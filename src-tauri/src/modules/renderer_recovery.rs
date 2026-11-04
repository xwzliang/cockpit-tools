//! Recover a main window whose renderer died while the backend stayed alive.
//!
//! In particular, WKWebView can retain a black window after WebContent crashes.
//! Showing/focusing that window does not restart its renderer. Probe from Rust
//! instead of relying on a JavaScript timer in the process that may have died.
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Runtime, State, WebviewWindow};
use tokio::sync::oneshot;

use crate::modules::logger;

const PROBE_INTERVAL: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const RELOAD_COOLDOWN: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct RendererProbe(Mutex<Option<(String, oneshot::Sender<()>)>>);

impl RendererProbe {
    fn begin(&self, token: String) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some((token, tx));
        rx
    }

    fn acknowledge(&self, token: &str) {
        let mut pending = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if pending.as_ref().is_some_and(|(expected, _)| expected == token) {
            if let Some((_, tx)) = pending.take() {
                let _ = tx.send(());
            }
        }
    }

    fn clear(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

#[tauri::command]
pub fn renderer_probe_ack<R: Runtime>(
    window: WebviewWindow<R>,
    probe: State<'_, RendererProbe>,
    token: String,
) {
    if window.label() == "main" {
        probe.acknowledge(&token);
    }
}

#[derive(Default)]
struct RecoveryPolicy {
    missed: u8,
    last_reload: Option<Instant>,
}

impl RecoveryPolicy {
    fn reset_probe_streak(&mut self) {
        self.missed = 0;
    }

    fn should_reload(&mut self, responsive: bool, now: Instant) -> bool {
        if responsive {
            self.reset_probe_streak();
            return false;
        }
        self.missed = self.missed.saturating_add(1);
        if self.missed < 2
            || self.last_reload.is_some_and(|last| now.duration_since(last) < RELOAD_COOLDOWN)
        {
            return false;
        }
        self.last_reload = Some(now);
        self.reset_probe_streak();
        true
    }
}

pub fn start<R: Runtime>(app: &AppHandle<R>) {
    if !app.manage(RendererProbe::default()) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut policy = RecoveryPolicy::default();
        let mut awaiting_recovery = false;
        // Allow initial asset loading before testing renderer responsiveness.
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            let Some(window) = app.get_webview_window("main") else {
                policy.reset_probe_streak();
                tokio::time::sleep(PROBE_INTERVAL).await;
                continue;
            };
            // Hidden/minimized WebViews may legitimately be suspended by the OS.
            if !window.is_visible().unwrap_or(false) || window.is_minimized().unwrap_or(true) {
                policy.reset_probe_streak();
                tokio::time::sleep(PROBE_INTERVAL).await;
                continue;
            }
            let token = uuid::Uuid::new_v4().to_string();
            let probe = app.state::<RendererProbe>();
            let reply = probe.begin(token.clone());
            let script = format!(
                "window.__TAURI_INTERNALS__.invoke('renderer_probe_ack', {{token: {}}}).catch(() => {{}})",
                serde_json::to_string(&token).expect("UUID is serializable")
            );
            // eval success only means the script was queued, not that it ran.
            let responsive = window.eval(script).is_ok()
                && matches!(tokio::time::timeout(PROBE_TIMEOUT, reply).await, Ok(Ok(())));
            probe.clear();
            if responsive && awaiting_recovery {
                logger::log_info("[RendererRecovery] Main renderer is responding again after reload");
                awaiting_recovery = false;
            }
            // A close/minimize during the probe must not trigger recovery.
            if !window.is_visible().unwrap_or(false) || window.is_minimized().unwrap_or(true) {
                policy.reset_probe_streak();
            } else if policy.should_reload(responsive, Instant::now()) {
                logger::log_warn("[RendererRecovery] Main renderer missed two probes; reloading WebView");
                awaiting_recovery = true;
                if let Err(error) = window.reload() {
                    logger::log_warn(&format!("[RendererRecovery] WebView reload failed: {error}"));
                }
            }
            tokio::time::sleep(PROBE_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_recovery_requires_two_consecutive_failures() {
        let now = Instant::now();
        let mut policy = RecoveryPolicy::default();
        assert!(!policy.should_reload(false, now));
        assert!(!policy.should_reload(true, now));
        assert!(!policy.should_reload(false, now));
        assert!(policy.should_reload(false, now));
    }

    #[test]
    fn renderer_recovery_respects_cooldown_even_after_a_success() {
        let now = Instant::now();
        let mut policy = RecoveryPolicy::default();
        assert!(!policy.should_reload(false, now));
        assert!(policy.should_reload(false, now));
        assert!(!policy.should_reload(true, now));
        assert!(!policy.should_reload(false, now + Duration::from_secs(20)));
        assert!(!policy.should_reload(false, now + Duration::from_secs(40)));
        assert!(policy.should_reload(false, now + RELOAD_COOLDOWN));
    }

    #[test]
    fn renderer_recovery_resets_failures_when_window_is_hidden_or_destroyed() {
        let now = Instant::now();
        let mut policy = RecoveryPolicy::default();
        assert!(!policy.should_reload(false, now));
        policy.reset_probe_streak();
        assert!(!policy.should_reload(false, now));
        assert!(policy.should_reload(false, now));
    }

    #[tokio::test]
    async fn renderer_recovery_ignores_late_acknowledgments() {
        let probe = RendererProbe::default();
        let expired = probe.begin("old".into());
        probe.clear();
        assert!(expired.await.is_err());
        let mut current = probe.begin("current".into());
        probe.acknowledge("old");
        assert!(matches!(current.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
        probe.acknowledge("current");
        assert!(current.await.is_ok());
    }
}
