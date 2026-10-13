use crate::models::codex::CodexAccount;
use crate::models::ssh_server::{
    SshAntigravitySyncResult, SshAntigravitySyncStatus, SshAuthConfig, SshCodexSyncResult,
    SshCodexSyncStatus, SshServer, SshServerStore,
};
use crate::models::Account;
use crate::modules::{account, atomic_write, codex_account, logger};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;
use uuid::Uuid;

const SSH_SERVERS_FILE: &str = "ssh_servers.json";
const STORE_VERSION: &str = "1";
/// TCP/SSH 握手超时（传给 OpenSSH ConnectTimeout）
const CONNECTION_TIMEOUT_SECS: u64 = 12;
/// 测连整段命令墙钟超时
const TEST_COMMAND_TIMEOUT_SECS: u64 = 20;
/// 读写同步脚本墙钟超时
const SYNC_TIMEOUT_SECS: u64 = 45;
/// 远端 app-server reload 墙钟超时（略放宽；失败不阻断已校验写入）
const APP_SERVER_RELOAD_TIMEOUT_SECS: u64 = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshServerList {
    pub selected_server_id: Option<String>,
    pub servers: Vec<SshServer>,
}

/// Only enumerate aliases; OpenSSH itself resolves connection settings at use time.
pub fn list_ssh_config_hosts() -> Result<Vec<String>, String> {
    let home = dirs::home_dir().ok_or("Unable to locate the user home directory")?;
    let mut aliases = std::collections::BTreeSet::new();
    let mut visited = std::collections::HashSet::new();
    collect_ssh_config_hosts(&home.join(".ssh/config"), &home.join(".ssh"), &home, &mut visited, &mut aliases)?;
    #[cfg(unix)]
    collect_ssh_config_hosts(std::path::Path::new("/etc/ssh/ssh_config"), std::path::Path::new("/etc/ssh"), &home, &mut visited, &mut aliases)?;
    Ok(aliases.into_iter().collect())
}

fn ssh_config_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in line.chars() {
        if escaped { word.push(ch); escaped = false; continue; }
        if ch == '\\' && quote != Some('\'') { escaped = true; continue; }
        if let Some(current) = quote {
            if ch == current { quote = None; } else { word.push(ch); }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch == '#' {
            break;
        } else if ch.is_whitespace() {
            if !word.is_empty() { words.push(std::mem::take(&mut word)); }
        } else { word.push(ch); }
    }
    if !word.is_empty() { words.push(word); }
    words
}

fn collect_ssh_config_hosts(
    path: &std::path::Path, base: &std::path::Path, home: &std::path::Path,
    visited: &mut std::collections::HashSet<PathBuf>,
    aliases: &mut std::collections::BTreeSet<String>,
) -> Result<(), String> {
    if !path.is_file() { return Ok(()); }
    let canonical = std::fs::canonicalize(path).map_err(|err| err.to_string())?;
    if !visited.insert(canonical) { return Ok(()); }
    if visited.len() > 256 { return Err("SSH config includes too many files".to_string()); }
    let config = std::fs::read_to_string(path).map_err(|err| format!("Unable to read SSH config {}: {}", path.display(), err))?;
    for line in config.lines() {
        let line = line.trim();
        let Some(separator) = line.find(|ch: char| ch.is_whitespace() || ch == '=') else { continue; };
        let keyword = &line[..separator];
        let values = ssh_config_words(line[separator..].trim_start().trim_start_matches('=').trim_start());
        if keyword.eq_ignore_ascii_case("host") {
            for alias in values {
                if !alias.chars().any(|ch| matches!(ch, '*' | '?' | '!') || ch.is_whitespace())
                    && !alias.starts_with('-') && !contains_control_separator(&alias) {
                    aliases.insert(alias);
                }
            }
        } else if keyword.eq_ignore_ascii_case("include") {
            for pattern in values {
                let expanded = if let Some(suffix) = pattern.strip_prefix("~/") {
                    home.join(suffix)
                } else if std::path::Path::new(&pattern).is_absolute() {
                    PathBuf::from(pattern)
                } else { base.join(pattern) };
                let paths = glob::glob(&expanded.to_string_lossy()).map_err(|err| err.to_string())?;
                for included in paths {
                    let included = included.map_err(|err| err.to_string())?;
                    collect_ssh_config_hosts(&included, base, home, visited, aliases)?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn remote_python_with_home(script: &str, variable: &str, home: &str) -> String {
    let home = serde_json::to_string(home).expect("serialize remote home");
    format!("import os\nos.environ[{variable:?}] = os.path.expanduser({home})\n{script}")
}

fn now_timestamp() -> i64 {
    chrono::Utc::now().timestamp()
}

fn store_path() -> Result<PathBuf, String> {
    Ok(account::get_data_dir()?.join(SSH_SERVERS_FILE))
}

fn default_codex_home() -> String {
    "~/.codex".to_string()
}

fn default_antigravity_home() -> String {
    "~/.gemini".to_string()
}

fn contains_control_separator(value: &str) -> bool {
    value.contains('\n') || value.contains('\r') || value.contains('\0')
}

fn normalize_text(value: &str) -> String {
    value.trim().to_string()
}

fn sanitize_error(error: impl ToString) -> String {
    let mut value = error.to_string();
    for marker in [
        "OPENAI_API_KEY",
        "access_token",
        "refresh_token",
        "id_token",
    ] {
        value = redact_secret_values(&value, marker);
    }
    value
}

fn redact_secret_values(value: &str, marker: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(index) = remaining.find(marker) {
        let (before, matched_and_after) = remaining.split_at(index);
        output.push_str(before);
        output.push_str(marker);

        let after_marker = &matched_and_after[marker.len()..];
        let Some((delimiter_end, quote)) = secret_value_start(after_marker) else {
            remaining = after_marker;
            continue;
        };
        output.push_str(&after_marker[..delimiter_end]);

        let value_start = delimiter_end;
        let value_end = secret_value_end(&after_marker[value_start..], quote);
        output.push_str("[redacted]");
        remaining = &after_marker[value_start + value_end..];
    }
    output.push_str(remaining);
    output
}

fn secret_value_start(value: &str) -> Option<(usize, Option<char>)> {
    let mut chars = value.char_indices().peekable();
    let mut end = 0;
    while let Some((index, ch)) = chars.peek().copied() {
        if ch.is_whitespace() || ch == '"' || ch == '\'' {
            end = index + ch.len_utf8();
            chars.next();
        } else {
            break;
        }
    }
    let (_, delimiter) = chars.next()?;
    if delimiter != '=' && delimiter != ':' {
        return None;
    }
    end += delimiter.len_utf8();
    while let Some((index, ch)) = chars.peek().copied() {
        if ch.is_whitespace() {
            end = index + ch.len_utf8();
            chars.next();
        } else {
            break;
        }
    }
    if let Some((index, quote @ ('"' | '\''))) = chars.peek().copied() {
        return Some((index + quote.len_utf8(), Some(quote)));
    }
    Some((end, None))
}

fn secret_value_end(value: &str, quote: Option<char>) -> usize {
    match quote {
        Some(quote) => value.find(quote).unwrap_or(value.len()),
        None => value
            .find(|ch: char| ch.is_whitespace() || ch == ',' || ch == ';' || ch == '}')
            .unwrap_or(value.len()),
    }
}

fn validate_server(server: &SshServer) -> Result<(), String> {
    if server.name.trim().is_empty() {
        return Err("SSH server name is required".to_string());
    }
    for (label, value) in [
        ("host", server.host.as_str()),
        ("username", server.username.as_str()),
        ("codex_home", server.codex_home.as_str()),
        ("antigravity_home", server.antigravity_home.as_str()),
    ] {
        if value.trim().is_empty() && !(label == "username" && matches!(server.auth, SshAuthConfig::SshConfig)) {
            return Err(format!("SSH server {} is required", label));
        }
        if contains_control_separator(value) {
            return Err(format!(
                "SSH server {} contains unsupported characters",
                label
            ));
        }
    }
    if server.host.starts_with('-') || server.host.chars().any(char::is_whitespace) {
        return Err("SSH host must be a hostname or SSH config alias".to_string());
    }
    if server.port == 0 {
        return Err("SSH server port must be between 1 and 65535".to_string());
    }
    match &server.auth {
        SshAuthConfig::Agent | SshAuthConfig::SshConfig => {}
        SshAuthConfig::PrivateKeyFile { path } => {
            if path.trim().is_empty() {
                return Err("SSH private key path is required".to_string());
            }
            if contains_control_separator(path) {
                return Err("SSH private key path contains unsupported characters".to_string());
            }
        }
    }
    Ok(())
}

fn normalize_server(
    mut server: SshServer,
    existing: Option<&SshServer>,
) -> Result<SshServer, String> {
    let now = now_timestamp();
    if server.id.trim().is_empty() {
        server.id = Uuid::new_v4().to_string();
    } else {
        server.id = normalize_text(&server.id);
    }
    server.name = normalize_text(&server.name);
    server.host = normalize_text(&server.host);
    server.username = normalize_text(&server.username);
    server.codex_home = normalize_text(&server.codex_home);
    if server.codex_home.is_empty() {
        server.codex_home = default_codex_home();
    }
    server.antigravity_home = normalize_text(&server.antigravity_home);
    if server.antigravity_home.is_empty() {
        server.antigravity_home = default_antigravity_home();
    }
    if server.port == 0 {
        server.port = 22;
    }
    if server.created_at <= 0 {
        server.created_at = existing.map(|item| item.created_at).unwrap_or(now);
    }
    server.updated_at = now;
    if let Some(existing) = existing {
        if server.last_sync.is_none() {
            server.last_sync = existing.last_sync.clone();
        }
        if server.last_antigravity_sync.is_none() {
            server.last_antigravity_sync = existing.last_antigravity_sync.clone();
        }
    }
    validate_server(&server)?;
    Ok(server)
}

pub fn load_store() -> Result<SshServerStore, String> {
    let path = store_path()?;
    if !path.exists() {
        return Ok(SshServerStore::default());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read SSH servers store: {}", e))?;
    let mut store: SshServerStore = atomic_write::parse_json_with_auto_restore(&path, &content)
        .map_err(|e| format!("Failed to parse SSH servers store: {}", e))?;
    if store.version.trim().is_empty() {
        store.version = STORE_VERSION.to_string();
    }
    for server in &mut store.servers {
        if server.antigravity_home.trim().is_empty() {
            server.antigravity_home = default_antigravity_home();
        }
        if server.codex_home.trim().is_empty() {
            server.codex_home = default_codex_home();
        }
    }
    if let Some(selected_id) = store.selected_server_id.clone() {
        if !store.servers.iter().any(|server| server.id == selected_id) {
            store.selected_server_id = None;
        }
    }
    Ok(store)
}

fn save_store(store: &SshServerStore) -> Result<(), String> {
    let path = store_path()?;
    let content = serde_json::to_string_pretty(store)
        .map_err(|e| format!("Failed to serialize SSH servers store: {}", e))?;
    atomic_write::write_string_atomic(&path, &content)
}

pub fn list_servers() -> Result<SshServerList, String> {
    let store = load_store()?;
    Ok(SshServerList {
        selected_server_id: store.selected_server_id,
        servers: store.servers,
    })
}

pub fn upsert_server(server: SshServer) -> Result<SshServerList, String> {
    let mut store = load_store()?;
    store.version = STORE_VERSION.to_string();
    let existing_index = store.servers.iter().position(|item| item.id == server.id);
    let existing = existing_index.and_then(|index| store.servers.get(index));
    let server = normalize_server(server, existing)?;
    if let Some(index) = existing_index {
        store.servers[index] = server;
    } else {
        store.servers.push(server);
    }
    save_store(&store)?;
    list_servers()
}

pub fn delete_server(server_id: &str) -> Result<SshServerList, String> {
    let mut store = load_store()?;
    let server_id = server_id.trim();
    store.servers.retain(|server| server.id != server_id);
    if store.selected_server_id.as_deref() == Some(server_id) {
        store.selected_server_id = None;
    }
    save_store(&store)?;
    list_servers()
}

pub fn select_server(server_id: Option<String>) -> Result<SshServerList, String> {
    let mut store = load_store()?;
    let selected = server_id.and_then(|value| {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });
    if let Some(selected_id) = selected.as_deref() {
        if !store.servers.iter().any(|server| server.id == selected_id) {
            return Err(format!("SSH server not found: {}", selected_id));
        }
    }
    store.selected_server_id = selected;
    save_store(&store)?;
    list_servers()
}

fn selected_server_from_store(store: &SshServerStore) -> Option<SshServer> {
    let selected_id = store.selected_server_id.as_deref()?;
    store
        .servers
        .iter()
        .find(|server| server.id == selected_id)
        .cloned()
}

/// OpenSSH 参数：非交互、握手超时与私钥 IdentitiesOnly，避免 agent 里一堆 key 拖慢/超时。
fn build_ssh_args(server: &SshServer, connect_timeout_secs: u64) -> Vec<String> {
    let connect_timeout = connect_timeout_secs.clamp(3, 30);
    let mut args = vec![
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "NumberOfPasswordPrompts=0".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout={}", connect_timeout),
        "-o".to_string(),
        "ServerAliveInterval=5".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=2".to_string(),
    ];
    if !matches!(server.auth, SshAuthConfig::SshConfig) {
        args.extend(["-p".to_string(), server.port.to_string()]);
    }
    if let SshAuthConfig::PrivateKeyFile { path } = &server.auth {
        // 与手动 `ssh -o IdentitiesOnly=yes -i key` 对齐：只用指定私钥，不试 agent 其它身份
        args.push("-o".to_string());
        args.push("IdentitiesOnly=yes".to_string());
        args.push("-i".to_string());
        args.push(path.clone());
    }
    // Pass the original alias unchanged: OpenSSH resolves HostName, User, Port,
    // IdentityFile, Include, ProxyJump and other options from its config.
    args.push(if matches!(server.auth, SshAuthConfig::SshConfig) {
        server.host.clone()
    } else {
        format!("{}@{}", server.username, server.host)
    });
    args
}

pub(crate) async fn run_ssh(
    server: &SshServer,
    timeout_secs: u64,
    remote_args: &[&str],
    stdin_payload: Option<String>,
) -> Result<String, String> {
    // 握手超时与整段墙钟分开：ConnectTimeout 用连接上限，命令本身可更长
    let connect_timeout = CONNECTION_TIMEOUT_SECS.min(timeout_secs);
    let mut command = Command::new("ssh");
    command.args(build_ssh_args(server, connect_timeout));
    command.args(remote_args);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    if stdin_payload.is_some() {
        command.stdin(Stdio::piped());
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("ssh_binary_missing: {}", e))?;
    if let Some(payload) = stdin_payload {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ssh_connection_failed: stdin unavailable".to_string())?;
        stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| format!("ssh_connection_failed: {}", e))?;
        // 尽快关闭 stdin，避免远端 sh -s 一直等 EOF
        drop(stdin);
    }

    let output = timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
        .await
        .map_err(|_| "ssh_connection_failed: SSH command timed out".to_string())?
        .map_err(|e| format!("ssh_connection_failed: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let category = if stderr.to_ascii_lowercase().contains("permission denied") {
            "ssh_auth_failed"
        } else {
            "ssh_connection_failed"
        };
        return Err(format!(
            "{}: {}",
            category,
            sanitize_error(if stderr.is_empty() {
                format!("exit status {}", output.status)
            } else {
                stderr
            })
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub async fn test_connection(server_id: &str) -> Result<String, String> {
    let store = load_store()?;
    let server = store
        .servers
        .iter()
        .find(|server| server.id == server_id)
        .cloned()
        .ok_or_else(|| format!("SSH server not found: {}", server_id))?;
    let output = run_ssh(
        &server,
        TEST_COMMAND_TIMEOUT_SECS,
        &["printf", "cockpit-tools-ssh-ok"],
        None,
    )
    .await?;
    if output.trim() == "cockpit-tools-ssh-ok" {
        Ok(output)
    } else {
        Err("ssh_connection_failed: unexpected SSH test output".to_string())
    }
}

fn read_remote_config_script() -> &'static str {
    r#"set -eu
codex_home=$1
case "$codex_home" in
  "~") codex_home="$HOME" ;;
  "~/"*) codex_home="$HOME/${codex_home#??}" ;;
esac
target="$codex_home/config.toml"
if [ -f "$target" ]; then
  printf '__COCKPIT_EXISTS__\n'
  cat "$target"
elif [ -e "$target" ]; then
  printf 'config.toml is not a regular file\n' >&2
  exit 3
else
  printf '__COCKPIT_MISSING__\n'
fi
"#
}

async fn read_remote_config_toml(server: &SshServer) -> Result<Option<String>, String> {
    let script = read_remote_config_script();
    let output = run_ssh(
        server,
        SYNC_TIMEOUT_SECS,
        &["sh", "-s", "--", &server.codex_home],
        Some(script.to_string()),
    )
    .await
    .map_err(|e| format!("ssh_remote_read_failed: {}", sanitize_error(e)))?;
    if let Some(rest) = output.strip_prefix("__COCKPIT_EXISTS__\n") {
        return Ok(Some(rest.to_string()));
    }
    if output.trim() == "__COCKPIT_MISSING__" {
        return Ok(None);
    }
    Err("ssh_remote_read_failed: unexpected remote read response".to_string())
}

fn build_codex_upload_script(bundle: &codex_account::CodexAccountProjectionBundle) -> String {
    let mut payload = String::new();
    for file in &bundle.files {
        payload.push_str(&format!(
            "{}\t{:o}\t{}\t{}\n",
            file.relative_path,
            file.mode,
            file.sha256,
            STANDARD.encode(file.content.as_bytes())
        ));
    }
    format!(
        r#"set -eu
codex_home=$1
case "$codex_home" in
  "~") codex_home="$HOME" ;;
  "~/"*) codex_home="$HOME/${{codex_home#??}}" ;;
esac
mkdir -p "$codex_home"
chmod 700 "$codex_home" 2>/dev/null || true
tmp_dir="$codex_home/.cockpit-codex-sync.$$"
rm -rf "$tmp_dir"
mkdir -p "$tmp_dir"
cleanup() {{ rm -rf "$tmp_dir"; }}
trap cleanup EXIT INT TERM
cat <<'__COCKPIT_CODEX_PAYLOAD__' | while IFS='	' read -r rel mode expected encoded; do
{payload}__COCKPIT_CODEX_PAYLOAD__
  [ -n "$rel" ] || continue
  case "$rel" in
    auth.json|config.toml|.cockpit_codex_auth.json) ;;
    *) printf 'invalid relative path: %s\n' "$rel" >&2; exit 4 ;;
  esac
  tmp="$tmp_dir/$rel"
  target="$codex_home/$rel"
  if ! printf '%s' "$encoded" | base64 -d > "$tmp" 2>/dev/null; then
    printf '%s' "$encoded" | base64 -D > "$tmp"
  fi
  chmod "$mode" "$tmp" 2>/dev/null || true
  mv "$tmp" "$target"
  chmod "$mode" "$target" 2>/dev/null || true
  if command -v sha256sum >/dev/null 2>&1; then
    hash_output="$(sha256sum "$target")"
  else
    hash_output="$(shasum -a 256 "$target")"
  fi
  actual="$(printf '%s\n' "$hash_output" | awk '{{print $1}}')"
  if [ "$actual" != "$expected" ]; then
    printf 'hash mismatch for %s\n' "$rel" >&2
    exit 5
  fi
  printf '%s\t%s\n' "$rel" "$actual"
done
"#
    )
}

async fn upload_and_verify_bundle(
    server: &SshServer,
    bundle: &codex_account::CodexAccountProjectionBundle,
) -> Result<(), String> {
    let script = build_codex_upload_script(bundle);
    let output = run_ssh(
        server,
        SYNC_TIMEOUT_SECS,
        &["sh", "-s", "--", &server.codex_home],
        Some(script),
    )
    .await
    .map_err(|e| format!("ssh_remote_write_failed: {}", sanitize_error(e)))?;

    for file in &bundle.files {
        let verified = output
            .lines()
            .any(|line| line == format!("{}\t{}", file.relative_path, file.sha256));
        if !verified {
            return Err(format!(
                "ssh_remote_verify_failed: missing verification for {}",
                file.relative_path
            ));
        }
    }
    Ok(())
}

/// 远端刷新 Codex app-server：
/// 在 Linux 上执行深度进程终止（含 VS Code remote 进程和残留 host）与 daemon 重启，
/// 并对齐 setup_codex_account_restart.sh 的权限与 identity 状态写入；非 Linux 下执行兼容回退。
fn reload_app_server_script() -> &'static str {
    r#"set +e
os_platform="$(uname -s 2>/dev/null || echo Unknown)"
codex_home="$1"
case "$codex_home" in
  "~") codex_home="$HOME" ;;
  "~/"*) codex_home="$HOME/${codex_home#??}" ;;
esac
[ -n "$codex_home" ] || codex_home="$HOME/.codex"
export CODEX_HOME="$codex_home"

if [ "$os_platform" = "Darwin" ]; then
  for app in "$HOME/Applications/ChatGPT.app" /Applications/ChatGPT.app "$HOME/Applications/Codex.app" /Applications/Codex.app; do
    [ -d "$app" ] || continue
    name="$(basename "$app" .app)"
    pids="$(ps -u "$(id -u)" -o pid= -o comm= | awk -v executable="$app/Contents/MacOS/$name" '$2 == executable { print $1 }')"
    [ -n "$pids" ] || continue
    kill -TERM $pids || exit 1
    sleep 2
    open -a "$app" || exit 1
    printf 'desktop-relaunched\n'
    exit 0
  done
fi

if [ "$os_platform" = "Linux" ]; then
  chmod 600 "$codex_home/auth.json" 2>/dev/null || true
  chmod 600 "$codex_home/config.toml" 2>/dev/null || true

  if command -v python3 >/dev/null 2>&1; then
    python3 - "$codex_home" <<'PY_EOF'
import os, sys, signal, time, pathlib, json, hashlib, subprocess

uid = os.getuid()
codex_home = pathlib.Path(sys.argv[1]).expanduser()
auth_file = codex_home / "auth.json"

state_dir = pathlib.Path.home() / ".local" / "state" / "codex-auth-account-monitor"
if auth_file.exists():
    try:
        doc = json.loads(auth_file.read_text(encoding="utf-8"))
        ident = doc.get("tokens", {}).get("account_id") or doc.get("account_id")
        if not ident and "tokens" in doc:
            import base64
            tok = doc["tokens"].get("id_token") or doc["tokens"].get("access_token")
            if tok and "." in tok:
                parts = tok.split(".")
                if len(parts) == 3:
                    payload = json.loads(base64.urlsafe_b64decode(parts[1] + "=="))
                    ident = payload.get("account_id") or payload.get("chatgpt_account_id") or payload.get("sub")
        if ident:
            digest = hashlib.sha256(str(ident).encode("utf-8")).hexdigest()
            state_dir.mkdir(parents=True, exist_ok=True)
            state_file = state_dir / "identity.sha256"
            state_file.write_text(digest + "\n", encoding="ascii")
    except Exception:
        pass

terminated = []
self_pid = os.getpid()
proc = pathlib.Path("/proc")
if proc.exists():
    for p in proc.iterdir():
        if not p.name.isdigit():
            continue
        pid = int(p.name)
        if pid == self_pid:
            continue
        try:
            if p.stat().st_uid != uid:
                continue
            exe = os.readlink(p / "exe")
            cmdline = (p / "cmdline").read_bytes().replace(b"\0", b" ").decode("utf-8", "ignore")
            is_codex = False
            exe_name = pathlib.Path(exe).name
            if exe_name in ("codex", "codex-code-mode-host"):
                is_codex = True
            elif "codex app-server" in cmdline or "codex/packages/standalone" in exe or "openai.chatgpt" in exe:
                is_codex = True

            if is_codex:
                os.kill(pid, signal.SIGTERM)
                terminated.append(pid)
        except Exception:
            continue

    if terminated:
        time.sleep(1.0)
        for pid in terminated:
            try:
                os.kill(pid, 0)
                os.kill(pid, signal.SIGKILL)
            except Exception:
                pass

daemon_status = "none"
codex_bin = None
for candidate in [
    "codex",
    str(pathlib.Path.home() / ".local/bin/codex"),
    str(codex_home / "packages/standalone/current/bin/codex"),
]:
    try:
        res = subprocess.run(["which", candidate] if candidate == "codex" else ["test", "-x", candidate], capture_output=True)
        if res.returncode == 0:
            codex_bin = candidate
            break
    except Exception:
        pass

if codex_bin:
    try:
        p = subprocess.run([codex_bin, "app-server", "daemon", "restart"], timeout=5, capture_output=True, text=True)
        if p.returncode == 0:
            daemon_status = "restarted"
        else:
            p2 = subprocess.run([codex_bin, "app-server", "daemon", "start"], timeout=5, capture_output=True, text=True)
            daemon_status = "started" if p2.returncode == 0 else "failed"
    except Exception as e:
        daemon_status = f"error:{e}"

print(f"linux-codex-restarted: terminated={len(terminated)}, daemon={daemon_status}")
PY_EOF
    exit 0
  fi

  # Linux 但无 python3 时的纯 shell 回退
  pids="$(ps -u "$(id -u)" -o pid= -o args= 2>/dev/null | awk '
/codex app-server/ || /codex.*code_mode_host/ || /openai\.chatgpt/ || /packages\/standalone\/.*codex/ { print $1 }
' || true)"
  pids="$(printf '%s\n' "$pids" | tr -s '[:space:]' ' ' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
  if [ -n "$pids" ]; then
    kill -TERM $pids 2>/dev/null || true
    sleep 1
    for pid in $pids; do
      if kill -0 "$pid" 2>/dev/null; then
        kill -KILL "$pid" 2>/dev/null || true
      fi
    done
  fi

  codex_bin="codex"
  if ! command -v codex >/dev/null 2>&1; then
    if [ -x "$HOME/.local/bin/codex" ]; then
      codex_bin="$HOME/.local/bin/codex"
    elif [ -x "$codex_home/packages/standalone/current/bin/codex" ]; then
      codex_bin="$codex_home/packages/standalone/current/bin/codex"
    else
      codex_bin=""
    fi
  fi

  daemon_status="none"
  if [ -n "$codex_bin" ]; then
    if command -v timeout >/dev/null 2>&1; then
      if timeout 5 "$codex_bin" app-server daemon restart >/dev/null 2>&1; then
        daemon_status="restarted"
      elif timeout 5 "$codex_bin" app-server daemon start >/dev/null 2>&1; then
        daemon_status="started"
      else
        daemon_status="failed"
      fi
    else
      if "$codex_bin" app-server daemon restart >/dev/null 2>&1; then
        daemon_status="restarted"
      elif "$codex_bin" app-server daemon start >/dev/null 2>&1; then
        daemon_status="started"
      else
        daemon_status="failed"
      fi
    fi
  fi
  printf 'linux-codex-restarted: terminated=shell, daemon=%s\n' "$daemon_status"
  exit 0
fi

# 3) 非 Linux 平台（如 macOS/BSD）优先 daemon restart，限制 5s
if command -v codex >/dev/null 2>&1; then
  if command -v timeout >/dev/null 2>&1; then
    timeout 5 codex app-server daemon restart >/dev/null 2>&1
    rc=$?
  else
    codex app-server daemon restart >/dev/null 2>&1
    rc=$?
  fi
  if [ "${rc:-1}" -eq 0 ]; then
    printf 'daemon-restarted\n'
    exit 0
  fi
fi

# 4) 非 Linux 下尝试结束仍在跑的 app-server（没有则直接成功）
pids="$(ps -u "$(id -u)" -o pid= -o args= 2>/dev/null | awk '
/codex app-server --listen/ || /codex app-server proxy/ { print $1 }
' || true)"
pids="$(printf '%s\n' "$pids" | tr -s '[:space:]' ' ' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
if [ -z "$pids" ]; then
  printf 'no-app-server\n'
  exit 0
fi

kill -TERM $pids 2>/dev/null || true
sleep 1
for pid in $pids; do
  if kill -0 "$pid" 2>/dev/null; then
    kill -KILL "$pid" 2>/dev/null || true
  fi
done
printf 'app-server-terminated\n'
exit 0
"#
}

async fn reload_remote_codex_app_server(server: &SshServer) -> Result<(), String> {
    let output = run_ssh(
        server,
        APP_SERVER_RELOAD_TIMEOUT_SECS,
        &["sh", "-s", "--", &server.codex_home],
        Some(reload_app_server_script().to_string()),
    )
    .await
    .map_err(|e| format!("ssh_remote_app_server_reload_failed: {}", sanitize_error(e)))?;

    let recognized_status = output.lines().map(str::trim).find(|line| {
        matches!(
            *line,
            "daemon-restarted" | "app-server-terminated" | "no-app-server" | "desktop-relaunched"
        ) || line.starts_with("linux-codex-restarted")
    });

    if let Some(status) = recognized_status {
        logger::log_info(&format!(
            "[Codex SSH] 远端 app-server 刷新成功: server_id={}, status={}",
            server.id, status
        ));
        Ok(())
    } else {
        Err(format!(
            "ssh_remote_app_server_reload_failed: unexpected reload response: {}",
            sanitize_error(output.trim())
        ))
    }
}

fn result_from_status(server: &SshServer, status: SshCodexSyncStatus) -> SshCodexSyncResult {
    SshCodexSyncResult {
        server_id: server.id.clone(),
        server_name: server.name.clone(),
        account_id: status.account_id,
        account_email: status.account_email,
        token_generation: status.token_generation,
        bundle_hash: status.bundle_hash,
        verified: status.verified,
        error: status.error,
        synced_at: status.synced_at,
    }
}

fn persist_sync_status(
    server_id: &str,
    status: SshCodexSyncStatus,
) -> Result<SshCodexSyncResult, String> {
    let mut store = load_store()?;
    let index = store
        .servers
        .iter()
        .position(|server| server.id == server_id)
        .ok_or_else(|| format!("SSH server not found: {}", server_id))?;
    store.servers[index].last_sync = Some(status.clone());
    store.servers[index].updated_at = now_timestamp();
    let result = result_from_status(&store.servers[index], status);
    save_store(&store)?;
    Ok(result)
}

async fn sync_account_to_server(server: SshServer, account: &CodexAccount) -> SshCodexSyncResult {
    let synced_at = now_timestamp();
    let sync_attempt = async {
        validate_server(&server)?;
        let existing_config = read_remote_config_toml(&server).await?;
        let bundle =
            codex_account::build_projection_bundle_for_remote(account, existing_config.as_deref())
                .map_err(|e| format!("codex_bundle_failed: {}", sanitize_error(e)))?;
        // 鉴权落盘 + 校验是硬条件
        upload_and_verify_bundle(&server, &bundle).await?;
        // reload 是软条件：远端没 codex / 进程卡死 / 超时不应把已成功写入判失败
        if let Err(reload_error) = reload_remote_codex_app_server(&server).await {
            logger::log_warn(&format!(
                "[Codex SSH] 远端 app-server 刷新失败（鉴权已写入）: server_id={}, error={}",
                server.id, reload_error
            ));
        }
        Ok::<_, String>(bundle)
    }
    .await;

    let status = match sync_attempt {
        Ok(bundle) => SshCodexSyncStatus {
            account_id: bundle.account_id,
            account_email: bundle.account_email,
            token_generation: bundle.token_generation,
            bundle_hash: bundle.bundle_hash,
            synced_at,
            verified: true,
            error: None,
        },
        Err(error) => SshCodexSyncStatus {
            account_id: account.id.clone(),
            account_email: account.email.clone(),
            token_generation: account.token_generation,
            bundle_hash: String::new(),
            synced_at,
            verified: false,
            error: Some(sanitize_error(error)),
        },
    };

    match persist_sync_status(&server.id, status.clone()) {
        Ok(result) => result,
        Err(error) => {
            logger::log_warn(&format!(
                "[Codex SSH] 保存同步状态失败: server_id={}, error={}",
                server.id, error
            ));
            result_from_status(&server, status)
        }
    }
}

pub async fn sync_current_account_to_server(
    server_id: Option<String>,
) -> Result<SshCodexSyncResult, String> {
    let account = codex_account::get_current_account()
        .ok_or_else(|| "codex_bundle_failed: no current Codex account".to_string())?;
    let store = load_store()?;
    let server = if let Some(server_id) = server_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        store
            .servers
            .iter()
            .find(|server| server.id == server_id)
            .cloned()
            .ok_or_else(|| format!("SSH server not found: {}", server_id))?
    } else {
        selected_server_from_store(&store)
            .ok_or_else(|| "ssh_not_configured: no selected SSH server".to_string())?
    };
    Ok(sync_account_to_server(server, &account).await)
}

pub async fn sync_selected_server_after_codex_switch(
    account: &CodexAccount,
) -> Option<SshCodexSyncResult> {
    let store = match load_store() {
        Ok(store) => store,
        Err(error) => {
            logger::log_warn(&format!("[Codex SSH] 读取 SSH 服务器配置失败: {}", error));
            return None;
        }
    };
    let Some(server) = selected_server_from_store(&store) else {
        return None;
    };
    if !server.sync_on_codex_switch {
        return None;
    }
    Some(sync_account_to_server(server, account).await)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityAccountProjectionBundle {
    pub account_id: String,
    pub account_email: String,
    pub files: Vec<AntigravityProjectionFile>,
    pub bundle_hash: String,
    pub keyring_payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityProjectionFile {
    pub relative_path: String,
    pub mode: u32,
    pub sha256: String,
    pub content: String,
}

fn sha256_hex_bytes(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content);
    format!("{:x}", hasher.finalize())
}

fn build_antigravity_bundle_hash(files: &[AntigravityProjectionFile]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for file in files {
        hasher.update(file.relative_path.as_bytes());
        hasher.update(b"\0");
        hasher.update(file.sha256.as_bytes());
        hasher.update(b"\0");
    }
    format!("{:x}", hasher.finalize())
}

pub fn build_antigravity_projection_bundle(
    account: &Account,
) -> Result<AntigravityAccountProjectionBundle, String> {
    let expiry = chrono::DateTime::from_timestamp(account.token.expiry_timestamp, 0)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);

    let token_json = serde_json::json!({
        "token": {
            "access_token": account.token.access_token,
            "token_type": "Bearer",
            "refresh_token": account.token.refresh_token,
            "expiry": expiry,
        },
        "auth_method": "consumer",
        "id_token": account.token.id_token,
    });
    let token_content = serde_json::to_string_pretty(&token_json)
        .map_err(|e| format!("序列化 Antigravity Token 失败: {}", e))?;

    let keyring_json = serde_json::json!({
        "token": {
            "access_token": account.token.access_token,
            "token_type": "Bearer",
            "refresh_token": account.token.refresh_token,
            "expiry": expiry,
        },
        "auth_method": "consumer",
    });
    let keyring_payload = serde_json::to_string(&keyring_json)
        .map_err(|e| format!("序列化 Antigravity Keyring 凭据失败: {}", e))?;

    let marker_json = serde_json::json!({
        "account_id": account.id,
        "account_email": account.email,
        "synced_at": now_timestamp(),
        "source": "cockpit-tools",
    });
    let marker_content = serde_json::to_string_pretty(&marker_json)
        .map_err(|e| format!("序列化 Antigravity 投影标记失败: {}", e))?;

    let files = vec![
        AntigravityProjectionFile {
            relative_path: "jetski-standalone-oauth-token".to_string(),
            mode: 0o600,
            sha256: sha256_hex_bytes(token_content.as_bytes()),
            content: token_content,
        },
        AntigravityProjectionFile {
            relative_path: ".cockpit_antigravity_auth.json".to_string(),
            mode: 0o600,
            sha256: sha256_hex_bytes(marker_content.as_bytes()),
            content: marker_content,
        },
    ];

    let bundle_hash = build_antigravity_bundle_hash(&files);

    Ok(AntigravityAccountProjectionBundle {
        account_id: account.id.clone(),
        account_email: account.email.clone(),
        files,
        bundle_hash,
        keyring_payload,
    })
}

const MACOS_KEYCHAIN_GUI_SCRIPT: &str = r#"
if [ "$(uname -s)" = "Darwin" ]; then
  if ! security add-generic-password -U -s gemini -a antigravity -w 'go-keyring-base64:__PAYLOAD__' -A >/dev/null 2>&1; then
    python3 - '__PAYLOAD__' <<'KEYCHAIN_GUI_PY'
import os,sys,time,tempfile,pathlib,plistlib,subprocess,shutil,uuid
with tempfile.TemporaryDirectory(prefix='cockpit-keychain-') as root:
    folder=pathlib.Path(root)
    label='com.cockpit.antigravity-auth.' + str(uuid.uuid4())
    runner=folder/'runner.py'
    runner.write_text('''import subprocess,sys,pathlib,json
args=sys.argv[2:]
result=subprocess.run(args,capture_output=True,text=True)
if result.returncode and "-U" in args:
    deleted=subprocess.run([args[0],"delete-generic-password","-s","gemini","-a","antigravity"],capture_output=True,text=True)
    if deleted.returncode==0:
        args.remove("-U")
        result=subprocess.run(args,capture_output=True,text=True)
pathlib.Path(sys.argv[1]).write_text(str(result.returncode))
sys.exit(result.returncode)
''')
    runner.chmod(0o700)
    result=folder/'result'
    program=[sys.executable,str(runner),str(result),shutil.which('security') or '/usr/bin/security',
        'add-generic-password','-U','-s','gemini','-a','antigravity','-w','go-keyring-base64:'+sys.argv[1],'-A']
    plist=folder/'job.plist'
    plist.write_bytes(plistlib.dumps(dict(Label=label,ProgramArguments=program,RunAtLoad=True,
        StandardOutPath=str(folder/'out'),StandardErrorPath=str(folder/'err'))))
    plist.chmod(0o600)
    domain='gui/'+str(os.getuid())
    boot=subprocess.run(['launchctl','bootstrap',domain,str(plist)],capture_output=True,text=True)
    if boot.returncode:
        raise SystemExit('macos_keychain_gui_unavailable: log in to the remote Mac desktop before syncing')
    try:
        deadline=time.monotonic()+15
        while not result.exists() and time.monotonic()<deadline: time.sleep(0.1)
        if not result.exists() or result.read_text().strip()!='0':
            raise SystemExit('macos_keychain_gui_write_failed: GUI credential writer failed or timed out; check Keychain access permissions in the remote desktop session')
    finally:
        subprocess.run(['launchctl','bootout',domain+'/'+label],capture_output=True)
KEYCHAIN_GUI_PY
  fi
fi
"#;

fn build_antigravity_upload_script(bundle: &AntigravityAccountProjectionBundle) -> String {
    let mut payload = String::new();
    for file in &bundle.files {
        payload.push_str(&format!(
            "{}\t{:o}\t{}\t{}\n",
            file.relative_path,
            file.mode,
            file.sha256,
            STANDARD.encode(file.content.as_bytes())
        ));
    }
    let keyring_payload_b64 = STANDARD.encode(bundle.keyring_payload.as_bytes());

    let script = format!(
        r#"set -eu
antigravity_home=$1
case "$antigravity_home" in
  "~") antigravity_home="$HOME" ;;
  "~/"*) antigravity_home="$HOME/${{antigravity_home#??}}" ;;
esac
mkdir -p "$antigravity_home"
chmod 700 "$antigravity_home" 2>/dev/null || true
tmp_dir="$antigravity_home/.cockpit-antigravity-sync.$$"
rm -rf "$tmp_dir"
mkdir -p "$tmp_dir"
cleanup() {{ rm -rf "$tmp_dir"; }}
trap cleanup EXIT INT TERM
cat <<'__COCKPIT_ANTIGRAVITY_PAYLOAD__' | while IFS='	' read -r rel mode expected encoded; do
{payload}__COCKPIT_ANTIGRAVITY_PAYLOAD__
  [ -n "$rel" ] || continue
  case "$rel" in
    jetski-standalone-oauth-token|.cockpit_antigravity_auth.json) ;;
    *) printf 'invalid relative path: %s\n' "$rel" >&2; exit 4 ;;
  esac
  tmp="$tmp_dir/$rel"
  target="$antigravity_home/$rel"
  if ! printf '%s' "$encoded" | base64 -d > "$tmp" 2>/dev/null; then
    printf '%s' "$encoded" | base64 -D > "$tmp"
  fi
  chmod "$mode" "$tmp" 2>/dev/null || true
  mv "$tmp" "$target"
  chmod "$mode" "$target" 2>/dev/null || true
  if command -v sha256sum >/dev/null 2>&1; then
    hash_output="$(sha256sum "$target")"
  else
    hash_output="$(shasum -a 256 "$target")"
  fi
  actual="$(printf '%s\n' "$hash_output" | awk '{{print $1}}')"
  if [ "$actual" != "$expected" ]; then
    printf 'hash mismatch for %s\n' "$rel" >&2
    exit 5
  fi
  printf '%s\t%s\n' "$rel" "$actual"
done

__COCKPIT_MACOS_KEYCHAIN__

# Try updating Linux Secret Service (GNOME Keyring / secret-tool / python dbus) if active user bus exists
USER_BUS="/run/user/$(id -u)/bus"
if [ -S "$USER_BUS" ]; then
  export DBUS_SESSION_BUS_ADDRESS="unix:path=$USER_BUS"
  payload_content=$(printf '%s' '{keyring_payload_b64}' | (base64 -d 2>/dev/null || base64 -D 2>/dev/null || true))
  if command -v secret-tool >/dev/null 2>&1; then
    printf '%s' "$payload_content" | secret-tool store --label=gemini service gemini username antigravity 2>/dev/null || true
  elif command -v python3 >/dev/null 2>&1; then
    printf '%s' "$payload_content" | python3 -c '
import os, sys
try:
    import dbus
    uid = os.getuid()
    bus_path = f"/run/user/{{uid}}/bus"
    if not os.path.exists(bus_path):
        sys.exit(0)
    bus = dbus.bus.BusConnection(f"unix:path={{bus_path}}")
    ss = bus.get_object("org.freedesktop.secrets", "/org/freedesktop/secrets")
    ss_iface = dbus.Interface(ss, "org.freedesktop.Secret.Service")
    session_path = ss_iface.OpenSession("plain", dbus.String("", variant_level=1))[1]
    raw_payload = sys.stdin.read().strip().encode("utf-8")
    default_coll = bus.get_object("org.freedesktop.secrets", "/org/freedesktop/secrets/aliases/default")
    coll_iface = dbus.Interface(default_coll, "org.freedesktop.Secret.Collection")
    props = {{
        "org.freedesktop.Secret.Item.Label": dbus.String("gemini", variant_level=1),
        "org.freedesktop.Secret.Item.Attributes": dbus.Dictionary({{
            "service": "gemini",
            "username": "antigravity",
            "xdg:schema": "org.freedesktop.Secret.Generic",
        }}, signature="ss", variant_level=1),
    }}
    secret_struct = (
        dbus.ObjectPath(session_path),
        dbus.Array([], signature="y"),
        dbus.Array(list(raw_payload), signature="y"),
        "text/plain; charset=utf-8",
    )
    coll_iface.CreateItem(props, secret_struct, True)
except Exception:
    sys.exit(0)
' 2>/dev/null || true
  fi
fi

# Ensure agy CLI wrapper is in place so SSH terminals can access keyring auth
for agy_dir in "$HOME/.local/bin" "/usr/local/bin"; do
  if [ -f "$agy_dir/agy" ] && file "$agy_dir/agy" 2>/dev/null | grep -q 'ELF'; then
    mv "$agy_dir/agy" "$agy_dir/agy.real"
    cat > "$agy_dir/agy" << 'EOF_AGY'
#!/usr/bin/env sh
if [ -z "${{DBUS_SESSION_BUS_ADDRESS:-}}" ] && [ -S "/run/user/$(id -u)/bus" ]; then
  export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u)/bus"
fi
DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REAL_BIN="$DIR/agy.real"
if [ ! -x "$REAL_BIN" ]; then
  REAL_BIN="$(which agy.real 2>/dev/null || true)"
fi
if [ -x "$REAL_BIN" ]; then
  exec env -u SSH_CONNECTION -u SSH_CLIENT -u SSH_TTY "$REAL_BIN" "$@"
fi
exec "$0.real" "$@"
EOF_AGY
    chmod +x "$agy_dir/agy"
  fi
done
"#
    );
    script.replace("__COCKPIT_MACOS_KEYCHAIN__", &MACOS_KEYCHAIN_GUI_SCRIPT.replace("__PAYLOAD__", &keyring_payload_b64))
}

async fn upload_and_verify_antigravity_bundle(
    server: &SshServer,
    bundle: &AntigravityAccountProjectionBundle,
) -> Result<(), String> {
    let script = build_antigravity_upload_script(bundle);
    let output = run_ssh(
        server,
        SYNC_TIMEOUT_SECS,
        &["sh", "-s", "--", &server.antigravity_home],
        Some(script),
    )
    .await
    .map_err(|e| format!("ssh_remote_write_failed: {}", sanitize_error(e)))?;

    for file in &bundle.files {
        let verified = output
            .lines()
            .any(|line| line == format!("{}\t{}", file.relative_path, file.sha256));
        if !verified {
            return Err(format!(
                "ssh_remote_verify_failed: missing verification for {}",
                file.relative_path
            ));
        }
    }
    Ok(())
}

fn result_from_antigravity_status(
    server: &SshServer,
    status: SshAntigravitySyncStatus,
) -> SshAntigravitySyncResult {
    SshAntigravitySyncResult {
        server_id: server.id.clone(),
        server_name: server.name.clone(),
        account_id: status.account_id,
        account_email: status.account_email,
        bundle_hash: status.bundle_hash,
        verified: status.verified,
        error: status.error,
        synced_at: status.synced_at,
    }
}

fn persist_antigravity_sync_status(
    server_id: &str,
    status: SshAntigravitySyncStatus,
) -> Result<SshAntigravitySyncResult, String> {
    let mut store = load_store()?;
    let index = store
        .servers
        .iter()
        .position(|server| server.id == server_id)
        .ok_or_else(|| format!("SSH server not found: {}", server_id))?;
    store.servers[index].last_antigravity_sync = Some(status.clone());
    store.servers[index].updated_at = now_timestamp();
    let result = result_from_antigravity_status(&store.servers[index], status);
    save_store(&store)?;
    Ok(result)
}

fn reload_antigravity_script() -> &'static str {
    r#"set -eu
if [ "$(uname -s)" = "Darwin" ]; then
  for app in "$HOME/Applications/Antigravity.app" /Applications/Antigravity.app; do
    [ -d "$app" ] || continue
    pids="$(ps -u "$(id -u)" -o pid= -o comm= | awk -v executable="$app/Contents/MacOS/Antigravity" '$2 == executable { print $1 }')"
    [ -n "$pids" ] || continue
    kill -TERM $pids || exit 1
    sleep 2
    open -a "$app" || exit 1
    printf 'desktop-relaunched\n'
    exit 0
  done
  printf 'no-antigravity-process\n'
  exit 0
fi

main_pid=""
main_exe=""
main_display=""
main_xauth=""

for p in $(pgrep -u "$(id -u)" -f antigravity 2>/dev/null || true); do
  [ -d "/proc/$p" ] || continue
  exe=$(readlink -f "/proc/$p/exe" 2>/dev/null || true)
  cmd=$(tr '\0' ' ' < "/proc/$p/cmdline" 2>/dev/null || true)
  case "$exe" in
    */antigravity)
      case "$cmd" in
        *--type=*) ;;
        *)
          main_pid="$p"
          main_exe="$exe"
          main_display=$(tr '\0' '\n' < "/proc/$p/environ" 2>/dev/null | grep '^DISPLAY=' | head -n 1 || true)
          main_xauth=$(tr '\0' '\n' < "/proc/$p/environ" 2>/dev/null | grep '^XAUTHORITY=' | head -n 1 || true)
          break
          ;;
      esac
      ;;
  esac
done

all_pids=$(pgrep -u "$(id -u)" -f "antigravity|language_server" 2>/dev/null || true)
if [ -z "$all_pids" ] && [ -z "$main_pid" ]; then
  printf 'no-antigravity-process\n'
  exit 0
fi

for p in $all_pids; do
  kill -TERM "$p" 2>/dev/null || true
done
sleep 2

for p in $all_pids; do
  if kill -0 "$p" 2>/dev/null; then
    kill -KILL "$p" 2>/dev/null || true
  fi
done
sleep 1

if [ -n "$main_exe" ] && [ -x "$main_exe" ]; then
  [ -n "$main_display" ] && eval "export $main_display"
  [ -n "$main_xauth" ] && eval "export $main_xauth"
  "$main_exe" </dev/null >/dev/null 2>&1 &
  printf 'desktop-relaunched\n'
  exit 0
fi

printf 'processes-terminated\n'
exit 0
"#
}

async fn reload_remote_antigravity_server(server: &SshServer) -> Result<(), String> {
    let output = run_ssh(
        server,
        APP_SERVER_RELOAD_TIMEOUT_SECS,
        &["sh", "-s"],
        Some(reload_antigravity_script().to_string()),
    )
    .await
    .map_err(|e| format!("ssh_remote_antigravity_reload_failed: {}", sanitize_error(e)))?;

    let status = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if matches!(
        status,
        "desktop-relaunched" | "processes-terminated" | "no-antigravity-process"
    ) {
        Ok(())
    } else {
        Err(format!(
            "ssh_remote_antigravity_reload_failed: unexpected reload response: {}",
            sanitize_error(status)
        ))
    }
}

async fn sync_antigravity_account_to_server(
    server: SshServer,
    account: &Account,
) -> SshAntigravitySyncResult {
    let synced_at = now_timestamp();
    let sync_attempt = async {
        validate_server(&server)?;
        let bundle = build_antigravity_projection_bundle(account)
            .map_err(|e| format!("antigravity_bundle_failed: {}", sanitize_error(e)))?;
        upload_and_verify_antigravity_bundle(&server, &bundle).await?;
        if let Err(reload_error) = reload_remote_antigravity_server(&server).await {
            logger::log_warn(&format!(
                "[Antigravity SSH] 远端进程刷新失败（鉴权已写入）: server_id={}, error={}",
                server.id, reload_error
            ));
        }
        Ok::<_, String>(bundle)
    }
    .await;

    let status = match sync_attempt {
        Ok(bundle) => SshAntigravitySyncStatus {
            account_id: bundle.account_id,
            account_email: bundle.account_email,
            bundle_hash: bundle.bundle_hash,
            synced_at,
            verified: true,
            error: None,
        },
        Err(error) => SshAntigravitySyncStatus {
            account_id: account.id.clone(),
            account_email: account.email.clone(),
            bundle_hash: String::new(),
            synced_at,
            verified: false,
            error: Some(sanitize_error(error)),
        },
    };

    match persist_antigravity_sync_status(&server.id, status.clone()) {
        Ok(result) => result,
        Err(error) => {
            logger::log_warn(&format!(
                "[Antigravity SSH] 保存同步状态失败: server_id={}, error={}",
                server.id, error
            ));
            result_from_antigravity_status(&server, status)
        }
    }
}

pub async fn sync_current_antigravity_account_to_server(
    server_id: Option<String>,
) -> Result<SshAntigravitySyncResult, String> {
    let account = account::get_current_account()?
        .ok_or_else(|| "antigravity_sync_failed: no current Antigravity account".to_string())?;
    let store = load_store()?;
    let server = if let Some(server_id) = server_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        store
            .servers
            .iter()
            .find(|server| server.id == server_id)
            .cloned()
            .ok_or_else(|| format!("SSH server not found: {}", server_id))?
    } else {
        selected_server_from_store(&store)
            .ok_or_else(|| "ssh_not_configured: no selected SSH server".to_string())?
    };
    Ok(sync_antigravity_account_to_server(server, &account).await)
}

pub async fn sync_selected_server_after_antigravity_switch(
    account: &Account,
) -> Option<SshAntigravitySyncResult> {
    let store = match load_store() {
        Ok(store) => store,
        Err(error) => {
            logger::log_warn(&format!("[Antigravity SSH] 读取 SSH 服务器配置失败: {}", error));
            return None;
        }
    };
    let server = selected_server_from_store(&store)?;
    if !server.sync_on_antigravity_switch {
        return None;
    }
    Some(sync_antigravity_account_to_server(server, account).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct StoreBackup {
        path: PathBuf,
        original: Option<Vec<u8>>,
    }

    impl StoreBackup {
        fn capture() -> Self {
            let path = store_path().expect("resolve ssh server store path");
            let original = std::fs::read(&path).ok();
            Self { path, original }
        }
    }

    impl Drop for StoreBackup {
        fn drop(&mut self) {
            if let Some(original) = self.original.as_ref() {
                if let Some(parent) = self.path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&self.path, original);
            } else if self.path.exists() {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    fn valid_server() -> SshServer {
        SshServer {
            id: "server-1".to_string(),
            name: "Dev".to_string(),
            host: "example.com".to_string(),
            port: 22,
            username: "alice".to_string(),
            codex_home: "~/.codex".to_string(),
            antigravity_home: "~/.gemini".to_string(),
            auth: SshAuthConfig::Agent,
            sync_on_codex_switch: true,
            sync_on_antigravity_switch: false,
            created_at: 1,
            updated_at: 1,
            last_sync: None,
            last_antigravity_sync: None,
        }
    }

    #[test]
    fn macos_keychain_generated_gui_writer_is_valid_python() {
        use std::io::Write;
        let helper = MACOS_KEYCHAIN_GUI_SCRIPT.split("<<'KEYCHAIN_GUI_PY'\n").nth(1).unwrap()
            .split("\nKEYCHAIN_GUI_PY").next().unwrap();
        let validation = r#"
import ast,sys
source=sys.stdin.read()
tree=ast.parse(source)
writers=[node.value.args[0].value for node in ast.walk(tree)
    if isinstance(node,ast.Expr) and isinstance(node.value,ast.Call)
    and isinstance(node.value.func,ast.Attribute) and node.value.func.attr=='write_text'
    and node.value.args and isinstance(node.value.args[0],ast.Constant)
    and isinstance(node.value.args[0].value,str) and 'import subprocess' in node.value.args[0].value]
assert len(writers)==1
compile(writers[0], 'generated-keychain-writer.py', 'exec')
"#;
        let mut child = std::process::Command::new("python3").args(["-c", validation])
            .stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(helper.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }

    #[test]
    fn macos_keychain_falls_back_to_the_logged_in_gui_session() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("cockpit-keychain-gui-{}", Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let fixtures = [
            ("uname", "#!/bin/sh\necho Darwin\n"),
            ("security", "#!/bin/sh\n[ \"$COCKPIT_GUI_TEST\" = 1 ] || exit 1\n[ \"$2\" != -U ] || exit 1\nprintf '%s\\n' \"$@\" > \"$HOME/gui-keychain-call\"\n"),
            ("launchctl", "#!/usr/bin/python3\nimport sys,plistlib,subprocess,os\nif sys.argv[1]=='bootstrap':\n p=plistlib.load(open(sys.argv[-1],'rb'))\n sys.exit(subprocess.run(p['ProgramArguments'],env=dict(os.environ,COCKPIT_GUI_TEST='1')).returncode)\n")
        ];
        for (name, text) in fixtures {
            let path = dir.join("bin").join(name);
            std::fs::write(&path, text).unwrap();
            #[cfg(unix)] {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let script = MACOS_KEYCHAIN_GUI_SCRIPT.replace("__PAYLOAD__", "ZHVtbXk=");
        let mut child = std::process::Command::new("sh").arg("-s")
            .env("HOME", &dir).env("PATH", format!("{}:/usr/bin:/bin", dir.join("bin").display()))
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(std::fs::read_to_string(dir.join("gui-keychain-call")).unwrap().contains("go-keyring-base64:ZHVtbXk="));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn macos_sync_scripts_expand_homes_verify_hashes_and_write_keychain() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("cockpit-macos-sync-{}", Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/uname"), "#!/bin/sh\necho Darwin\n").unwrap();
        std::fs::write(dir.join("bin/security"), "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOME/keychain-call\"\n").unwrap();
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            for name in ["uname", "security"] {
                std::fs::set_permissions(dir.join("bin").join(name), std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let content = "dummy credential";
        let hash = sha256_hex_bytes(content.as_bytes());
        let codex: codex_account::CodexAccountProjectionBundle = serde_json::from_value(serde_json::json!({
            "account_id":"test", "account_email":"test@example.com", "token_generation":1,
            "bundle_hash":"test", "files":[{"relative_path":"auth.json", "mode":384, "sha256":hash, "content":content}]
        })).unwrap();
        let ag: AntigravityAccountProjectionBundle = serde_json::from_value(serde_json::json!({
            "account_id":"test", "account_email":"test@example.com", "bundle_hash":"test", "keyring_payload":"dummy keyring",
            "files":[{"relative_path":"jetski-standalone-oauth-token", "mode":384, "sha256":hash, "content":content}]
        })).unwrap();
        let scripts = [("codex", build_codex_upload_script(&codex), "~/.codex", "auth.json"),
            ("antigravity", build_antigravity_upload_script(&ag), "~/.gemini", "jetski-standalone-oauth-token")];
        for (name, script, home, file) in scripts {
            if let Ok(export) = std::env::var("COCKPIT_SSH_FIXTURE_EXPORT_DIR") {
                std::fs::create_dir_all(&export).unwrap();
                std::fs::write(std::path::Path::new(&export).join(format!("{}.sh", name)), &script).unwrap();
            }
            let mut child = std::process::Command::new("sh").args(["-s", "--", home])
                .env("HOME", &dir).env("PATH", format!("{}:/usr/bin:/bin", dir.join("bin").display()))
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            assert_eq!(std::fs::read_to_string(dir.join(&home[2..]).join(file)).unwrap(), content);
            assert!(String::from_utf8_lossy(&output.stdout).contains(&hash));
        }
        let keychain = std::fs::read_to_string(dir.join("keychain-call")).unwrap();
        assert!(keychain.contains("-s\ngemini\n-a\nantigravity"));
        assert!(keychain.contains("go-keyring-base64:"));
        assert!(!dir.join("~").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ssh_config_dropdown_reads_includes_quotes_and_filters_patterns() {
        let dir = std::env::temp_dir().join(format!("cockpit-ssh-config-{}", Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("conf.d")).unwrap();
        std::fs::write(dir.join("config"), "Host macbookair daily !excluded *.local ?wild\nInclude conf.d/*.conf\nhost=quoted other # comment\n").unwrap();
        std::fs::write(dir.join("conf.d/remote.conf"), "Host \"pve-ubuntu\" daily\nInclude config\n").unwrap();
        let mut aliases = std::collections::BTreeSet::new();
        collect_ssh_config_hosts(&dir.join("config"), &dir, &dir, &mut std::collections::HashSet::new(), &mut aliases).unwrap();
        assert_eq!(aliases.into_iter().collect::<Vec<_>>(), ["daily", "macbookair", "other", "pve-ubuntu", "quoted"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ssh_config_alias_preserves_openssh_connection_settings() {
        let mut server = valid_server();
        server.auth = SshAuthConfig::SshConfig;
        server.host = "my-dev-host".to_string();
        server.username.clear();
        assert!(validate_server(&server).is_ok());
        let args = build_ssh_args(&server, 12);
        assert_eq!(args.last().unwrap(), "my-dev-host");
        for override_arg in ["-p", "-l", "-i", "IdentitiesOnly=yes", "-F"] {
            assert!(!args.iter().any(|arg| arg == override_arg));
        }
        assert!(args.iter().any(|arg| arg == "BatchMode=yes"));
        server.host = "-oProxyCommand=command".to_string();
        assert!(validate_server(&server).is_err());
    }

    #[test]
    fn validation_rejects_empty_host() {
        let mut server = valid_server();
        server.host.clear();
        assert!(validate_server(&server).is_err());
    }

    #[test]
    fn validation_rejects_private_key_without_path() {
        let mut server = valid_server();
        server.auth = SshAuthConfig::PrivateKeyFile {
            path: String::new(),
        };
        assert!(validate_server(&server).is_err());
    }

    #[test]
    fn ssh_args_include_batch_mode_without_disabling_host_key_checks() {
        let server = valid_server();
        let args = build_ssh_args(&server, 10);
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.contains(&"ConnectTimeout=10".to_string()));
        assert!(!args
            .iter()
            .any(|arg| arg.contains("StrictHostKeyChecking=no")));
        // agent 模式不强制 IdentitiesOnly
        assert!(!args.iter().any(|arg| arg == "IdentitiesOnly=yes"));
    }

    #[test]
    fn ssh_args_use_identities_only_for_private_key() {
        let mut server = valid_server();
        server.auth = SshAuthConfig::PrivateKeyFile {
            path: "/tmp/id_test".to_string(),
        };
        let args = build_ssh_args(&server, 12);
        assert!(args
            .windows(2)
            .any(|w| w[0] == "-o" && w[1] == "IdentitiesOnly=yes"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "-i" && w[1] == "/tmp/id_test"));
    }

    #[test]
    fn app_server_reload_script_restarts_or_terminates_codex_app_server() {
        let script = reload_app_server_script();
        assert!(script.contains("codex app-server daemon restart"));
        assert!(script.contains("timeout 5 codex app-server daemon restart"));
        assert!(script.contains("codex app-server --listen"));
        assert!(script.contains("codex app-server proxy"));
        assert!(script.contains("no-app-server"));
        assert!(!script.contains("pkill"));
        assert!(script.contains("os_platform="));
        assert!(script.contains("linux-codex-restarted"));
        assert!(script.contains("codex-auth-account-monitor"));
    }

    #[test]
    fn sanitize_error_redacts_secret_values() {
        let error = r#"access_token=abc123 refresh_token: 'def456' {"id_token":"ghi789","OPENAI_API_KEY":"sk-test"}"#;
        let sanitized = sanitize_error(error);
        assert!(sanitized.contains("access_token=[redacted]"));
        assert!(sanitized.contains("refresh_token: '[redacted]'"));
        assert!(sanitized.contains(r#""id_token":"[redacted]""#));
        assert!(sanitized.contains(r#""OPENAI_API_KEY":"[redacted]""#));
        assert!(!sanitized.contains("abc123"));
        assert!(!sanitized.contains("def456"));
        assert!(!sanitized.contains("ghi789"));
        assert!(!sanitized.contains("sk-test"));
    }

    #[test]
    fn validation_rejects_empty_antigravity_home() {
        let mut server = valid_server();
        server.antigravity_home.clear();
        assert!(validate_server(&server).is_err());
    }

    #[test]
    fn build_antigravity_bundle_generates_valid_files() {
        use crate::models::token::TokenData;

        let token = TokenData::new(
            "test_access_token".to_string(),
            "test_refresh_token".to_string(),
            3600,
            Some("user@example.com".to_string()),
            None,
            None,
        );
        let account = Account::new(
            "test_id".to_string(),
            "user@example.com".to_string(),
            token,
        );

        let bundle = build_antigravity_projection_bundle(&account).unwrap();
        assert_eq!(bundle.account_id, "test_id");
        assert_eq!(bundle.account_email, "user@example.com");
        assert_eq!(bundle.files.len(), 2);

        let oauth_file = bundle
            .files
            .iter()
            .find(|f| f.relative_path == "jetski-standalone-oauth-token")
            .unwrap();
        assert_eq!(oauth_file.mode, 0o600);
        assert!(oauth_file.content.contains("test_access_token"));
        assert!(oauth_file.content.contains("test_refresh_token"));
        assert!(oauth_file.content.contains("consumer"));

        let marker_file = bundle
            .files
            .iter()
            .find(|f| f.relative_path == ".cockpit_antigravity_auth.json")
            .unwrap();
        assert_eq!(marker_file.mode, 0o600);
        assert!(marker_file.content.contains("cockpit-tools"));
    }

    #[test]
    fn antigravity_reload_script_contains_process_detection_and_relaunch() {
        let script = reload_antigravity_script();
        assert!(script.contains("antigravity"));
        assert!(script.contains("language_server"));
        assert!(script.contains("desktop-relaunched"));
        assert!(script.contains("processes-terminated"));
        assert!(script.contains("no-antigravity-process"));
        assert!(script.contains("DISPLAY="));
        assert!(script.contains("XAUTHORITY="));
    }

    #[tokio::test]
    #[ignore]
    async fn live_ssh_own_syncs_current_codex_account() {
        if std::env::var("COCKPIT_LIVE_SSH_OWN_SYNC").ok().as_deref() != Some("1") {
            eprintln!("set COCKPIT_LIVE_SSH_OWN_SYNC=1 to run the live own SSH sync test");
            return;
        }

        let current = codex_account::get_current_account()
            .expect("a current Codex account is required for live SSH sync");
        let _backup = StoreBackup::capture();
        let now = now_timestamp();
        let server = SshServer {
            id: "live-ssh-own".to_string(),
            name: "own".to_string(),
            host: "own".to_string(),
            port: 22,
            username: "ubuntu".to_string(),
            codex_home: "~/.codex".to_string(),
            antigravity_home: "~/.gemini".to_string(),
            auth: SshAuthConfig::Agent,
            sync_on_codex_switch: true,
            sync_on_antigravity_switch: false,
            created_at: now,
            updated_at: now,
            last_sync: None,
            last_antigravity_sync: None,
        };
        let store = SshServerStore {
            version: STORE_VERSION.to_string(),
            selected_server_id: Some(server.id.clone()),
            servers: vec![server.clone()],
        };
        save_store(&store).expect("write live SSH server store");

        test_connection(&server.id)
            .await
            .expect("live SSH connection test should pass");
        let result = sync_current_account_to_server(Some(server.id.clone()))
            .await
            .expect("live SSH sync should return a result");

        assert!(
            result.verified,
            "live SSH sync should verify remote hashes: {:?}",
            result.error
        );
        assert_eq!(result.account_id, current.id);
        assert_eq!(result.account_email, current.email);
        assert_eq!(result.token_generation, current.token_generation);
    }

    #[tokio::test]
    #[ignore]
    async fn live_ssh_server_syncs_current_antigravity_account() {
        let home = dirs::home_dir().expect("get home dir");
        let real_cockpit_dir = home.join(".antigravity_cockpit");
        std::env::set_var("COCKPIT_TOOLS_DATA_DIR", &real_cockpit_dir);

        let current = account::get_current_account()
            .expect("read current antigravity account")
            .expect("a current Antigravity account is required for live SSH sync");
        let store = load_store().expect("load store");
        let server = store
            .servers
            .into_iter()
            .find(|s| s.name == "server" || s.host == "server" || s.host == "pve-ubuntu.local")
            .expect("ssh server named 'server' not found");

        test_connection(&server.id)
            .await
            .expect("ssh connection test should pass");

        let result = sync_current_antigravity_account_to_server(Some(server.id.clone()))
            .await
            .expect("sync current antigravity account to server");

        assert!(
            result.verified,
            "live SSH sync should verify remote hashes: {:?}",
            result.error
        );
        assert_eq!(result.account_id, current.id);
        assert_eq!(result.account_email, current.email);
    }

    #[tokio::test]
    #[ignore]
    async fn live_ssh_server_syncs_current_codex_account_real_store() {
        let home = dirs::home_dir().expect("get home dir");
        let real_cockpit_dir = home.join(".antigravity_cockpit");
        std::env::set_var("COCKPIT_TOOLS_DATA_DIR", &real_cockpit_dir);

        let current = codex_account::get_current_account()
            .expect("read current codex account");
        let store = load_store().expect("load store");
        let server = store
            .servers
            .into_iter()
            .find(|s| s.name == "server" || s.host == "server" || s.host == "pve-ubuntu.local")
            .expect("ssh server named 'server' not found");

        test_connection(&server.id)
            .await
            .expect("ssh connection test should pass");

        let result = sync_current_account_to_server(Some(server.id.clone()))
            .await
            .expect("sync current codex account to server");

        assert!(
            result.verified,
            "live SSH sync should verify remote hashes: {:?}",
            result.error
        );
        assert_eq!(result.account_id, current.id);
        assert_eq!(result.account_email, current.email);
    }
}
