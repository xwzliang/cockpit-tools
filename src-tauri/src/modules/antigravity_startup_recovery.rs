//! Recover Electron's error page when the desktop server starts after its load timeout.
use std::sync::atomic::{AtomicU64, Ordering};
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn failed_startup(log: &str) -> Option<(u32, u16)> {
    let start = log.rfind("Starting app (")?;
    log[start..].lines().rev().find_map(|line| {
        if !line.contains("with error: ERR_TIMED_OUT") { return None; }
        let pid = line.split("(node:").nth(1)?.split(')').next()?.parse().ok()?;
        let url = line.split("Failed to load URL: ").nth(1)?.split_whitespace().next()?;
        // Only probe the app's HTTPS loopback root, never an arbitrary logged URL.
        let port = url.strip_prefix("https://127.0.0.1:")?.strip_suffix('/')?.parse::<u16>().ok()?;
        (port != 0).then_some((pid, port))
    })
}

pub(super) fn watch(log_path: std::path::PathBuf, offset: u64) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        use std::io::{Read, Seek, SeekFrom};
        let client = match reqwest::blocking::Client::builder()
            .no_proxy().redirect(reqwest::redirect::Policy::none())
            // Antigravity serves its loopback UI with a self-signed certificate.
            .danger_accept_invalid_certs(true).timeout(std::time::Duration::from_secs(2)).build() {
                Ok(client) => client, Err(_) => return,
            };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        while std::time::Instant::now() < deadline && GENERATION.load(Ordering::SeqCst) == generation {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let Ok(mut file) = std::fs::File::open(&log_path) else { continue; };
            let Ok(meta) = file.metadata() else { continue; };
            // If rotated/truncated, read the new file from its beginning.
            let from = if meta.len() < offset { 0 } else { offset };
            if file.seek(SeekFrom::Start(from)).is_err() { continue; }
            let mut log = String::new();
            if file.take(256 * 1024).read_to_string(&mut log).is_err() { continue; }
            let Some((pid, port)) = failed_startup(&log) else { continue; };
            if !super::process::is_pid_running(pid) { return; }
            let ready = client.get(format!("https://127.0.0.1:{port}/")).send()
                .map(|response| response.status().is_success()).unwrap_or(false);
            if !ready { continue; }
            if GENERATION.load(Ordering::SeqCst) != generation { return; }
            // Target the failed process and only reload an error page. Never send a
            // global keyboard shortcut or reload a window the user already recovered.
            let script = format!(r#"
with timeout of 5 seconds
 tell application "System Events"
  set targetProcess to first process whose unix id is {pid}
  if not (name of targetProcess starts with "Antigravity") then return "wrong-process"
  tell targetProcess
   repeat with w in windows
    repeat with element in entire contents of w
     try
      set pageURL to value of attribute "AXURL" of element
      if pageURL starts with "chrome-error://" then
       click menu item "Reload" of menu "View" of menu bar item "View" of menu bar 1
       return "reloaded"
      end if
     end try
    end repeat
   end repeat
  end tell
 end tell
end timeout
return "no-error-page"
"#);
            let result = crate::modules::process_timeout::output_with_timeout(
                std::process::Command::new("osascript").args(["-e", &script]),
                std::time::Duration::from_secs(7));
            match result {
                Ok(output) if output.status.success() => crate::modules::logger::log_info(&format!(
                    "[Antigravity Startup] server ready, recovery result: {}", String::from_utf8_lossy(&output.stdout).trim())),
                _ => crate::modules::logger::log_warn("[Antigravity Startup] server ready but window recovery failed; use View > Reload"),
            }
            return;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_timeout_requires_current_launch_and_loopback_root() {
        let failure = "(node:42) electron: Failed to load URL: https://127.0.0.1:62867/ with error: ERR_TIMED_OUT";
        assert_eq!(failed_startup(failure), None);
        assert_eq!(failed_startup(&format!("Starting app (v2)\n{failure}")), Some((42,62867)));
        assert_eq!(failed_startup(&format!("Starting app (v2)\n{failure}\nStarting app (v2)")), None);
        for url in ["https://example.com/", "https://127.0.0.1:123/path", "https://127.0.0.1:0/"] {
            assert_eq!(failed_startup(&format!("Starting app (v2)\n{}",failure.replace("https://127.0.0.1:62867/",url))),None);
        }
    }
}
