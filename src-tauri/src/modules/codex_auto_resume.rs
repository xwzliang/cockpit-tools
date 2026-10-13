use crate::models::codex::CodexAccount;
use crate::models::ssh_server::SshServer;
use crate::modules::{config, logger, ssh_server};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::Utc;
use rusqlite::OpenFlags;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter};

const RETRY_STATE_FILE: &str = "codex_auto_resume_retries.json";
static RETRY_STATE: OnceLock<Mutex<HashMap<String, RecoveryRetry>>> = OnceLock::new();
static RECOVERY_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecoveryRetry {
    task: InterruptedCodexTask,
    attempts: u32,
    next_attempt_at: i64,
    first_attempt_at: i64,
}

fn retry_delay_seconds(attempts: u32) -> i64 {
    match attempts {
        0..=3 => 60,
        4..=6 => 300,
        _ => 600,
    }
}

fn retry_state() -> &'static Mutex<HashMap<String, RecoveryRetry>> {
    RETRY_STATE.get_or_init(|| {
        let retries = crate::modules::account::get_data_dir().ok()
            .and_then(|dir| std::fs::read_to_string(dir.join(RETRY_STATE_FILE)).ok())
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        Mutex::new(retries)
    })
}

fn save_retry_state(retries: &HashMap<String, RecoveryRetry>) {
    if let Ok(dir) = crate::modules::account::get_data_dir() {
        if let Ok(json) = serde_json::to_string_pretty(retries) {
            if let Err(err) = crate::modules::atomic_write::write_string_atomic(&dir.join(RETRY_STATE_FILE), &json) {
                logger::log_warn(&format!("[CodexAutoResume] 保存恢复重试状态失败: {}", err));
            }
        }
    }
}

fn schedule_next_retry(retry: &mut RecoveryRetry, now_ts: i64) {
    retry.attempts = retry.attempts.saturating_add(1);
    if retry.first_attempt_at == 0 {
        retry.first_attempt_at = now_ts;
    }
    retry.next_attempt_at = now_ts + retry_delay_seconds(retry.attempts);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptedCodexTask {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub source: String, // "local" | "ssh"
    #[serde(default)]
    pub server_id: Option<String>,
    #[serde(default)]
    pub server_name: Option<String>,
    pub thread_id: String,
    #[serde(default)]
    pub goal_id: Option<String>,
    #[serde(default)]
    pub is_goal: bool,
    pub title: String,
    #[serde(default)]
    pub objective: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub status: String,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub resets_at: Option<i64>,
    #[serde(default)]
    pub reset_window_minutes: Option<i64>,
    #[serde(default)]
    pub reset_account_email: Option<String>,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub can_resume: bool,
    #[serde(default)]
    pub is_excluded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeTaskResult {
    pub success: bool,
    pub message: String,
}

const RESUMED_TASKS_FILE: &str = "codex_auto_resumed_tasks.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumedTaskRecord {
    pub id: String,
    pub source: String,
    pub server_id: Option<String>,
    pub server_name: Option<String>,
    pub thread_id: String,
    pub goal_id: Option<String>,
    pub is_goal: bool,
    pub title: String,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub resumed_at: i64,
    pub trigger_type: String, // "auto" | "manual"
    pub success: bool,
    pub message: String,
}

fn resumed_tasks_file_path() -> Result<std::path::PathBuf, String> {
    Ok(crate::modules::account::get_data_dir()?.join(RESUMED_TASKS_FILE))
}

pub fn load_resumed_task_records() -> Vec<ResumedTaskRecord> {
    let path = match resumed_tasks_file_path() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    if !path.exists() {
        return Vec::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub fn record_resumed_task(record: ResumedTaskRecord) {
    let mut records = load_resumed_task_records();
    records.insert(0, record);
    if records.len() > 100 {
        records.truncate(100);
    }
    if let Ok(path) = resumed_tasks_file_path() {
        if let Ok(content) = serde_json::to_string_pretty(&records) {
            let _ = crate::modules::atomic_write::write_string_atomic(&path, &content);
        }
    }
}

pub fn clear_resumed_task_records() -> Result<(), String> {
    let path = resumed_tasks_file_path()?;
    if path.exists() {
        let _ = crate::modules::atomic_write::write_string_atomic(&path, "[]");
    }
    Ok(())
}

const EXCLUDED_TASKS_FILE: &str = "codex_auto_resume_excluded_tasks.json";

fn excluded_tasks_file_path() -> Result<std::path::PathBuf, String> {
    Ok(crate::modules::account::get_data_dir()?.join(EXCLUDED_TASKS_FILE))
}

pub fn load_excluded_task_ids() -> std::collections::HashSet<String> {
    let path = match excluded_tasks_file_path() {
        Ok(p) => p,
        Err(_) => return std::collections::HashSet::new(),
    };
    if !path.exists() {
        return std::collections::HashSet::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => std::collections::HashSet::new(),
    }
}

pub fn save_excluded_task_ids(ids: &std::collections::HashSet<String>) -> Result<(), String> {
    let path = excluded_tasks_file_path()?;
    let content = serde_json::to_string_pretty(ids).map_err(|e| e.to_string())?;
    crate::modules::atomic_write::write_string_atomic(&path, &content)
}

pub fn set_task_excluded(task_id: &str, excluded: bool) -> Result<(), String> {
    let mut ids = load_excluded_task_ids();
    if excluded {
        ids.insert(task_id.to_string());
    } else {
        ids.remove(task_id);
    }
    save_excluded_task_ids(&ids)
}

const DISMISSED_TASKS_FILE: &str = "codex_auto_resume_dismissed_tasks.json";

fn dismissed_tasks_file_path() -> Result<std::path::PathBuf, String> {
    Ok(crate::modules::account::get_data_dir()?.join(DISMISSED_TASKS_FILE))
}

pub fn load_dismissed_task_ids() -> std::collections::HashSet<String> {
    let path = match dismissed_tasks_file_path() {
        Ok(p) => p,
        Err(_) => return std::collections::HashSet::new(),
    };
    if !path.exists() {
        return std::collections::HashSet::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => std::collections::HashSet::new(),
    }
}

pub fn save_dismissed_task_ids(ids: &std::collections::HashSet<String>) -> Result<(), String> {
    let path = dismissed_tasks_file_path()?;
    let content = serde_json::to_string_pretty(ids).map_err(|e| e.to_string())?;
    crate::modules::atomic_write::write_string_atomic(&path, &content)
}

pub fn delete_interrupted_task(task_id: &str) -> Result<(), String> {
    let mut ids = load_dismissed_task_ids();
    ids.insert(task_id.to_string());
    if let Some(pos) = task_id.rfind(':') {
        let raw_id = &task_id[pos + 1..];
        if !raw_id.is_empty() {
            ids.insert(raw_id.to_string());
        }
    }
    save_dismissed_task_ids(&ids)
}

const PYTHON_REMOTE_PROBE: &str = r#"
import os, sys, json, sqlite3, time
from pathlib import Path

def parse_rollout_tail(rollout_path):
    if not rollout_path or not os.path.exists(rollout_path):
        return None, None, None, False
    try:
        with open(rollout_path, "rb") as f:
            f.seek(0, 2)
            size = f.tell()
            f.seek(max(0, size - 65536))
            chunk = f.read().decode("utf-8", errors="ignore")
        
        lines = chunk.strip().splitlines()
        resets_at = None
        reset_window_minutes = None
        error_msg = None
        is_usage_limited = False
        has_task_complete = False
        
        for line in reversed(lines):
            line = line.strip()
            if not line:
                continue
            try:
                data = json.loads(line)
            except:
                continue
            payload = data.get("payload", {})
            p_type = payload.get("type")
            
            if p_type == "task_complete":
                if has_task_complete:
                    break
                has_task_complete = True
                
                err = payload.get("error")
                if err is None:
                    # Latest turn completed cleanly without error!
                    # Do not scan backwards into older turns for stale errors.
                    break
                
                if isinstance(err, dict):
                    if error_msg is None:
                        error_msg = err.get("message")
                    code = err.get("codex_error_info")
                    if code == "usage_limit_exceeded" or (error_msg and "usage limit" in error_msg.lower()):
                        is_usage_limited = True
            
            elif p_type == "task_started":
                # Reached the start of the latest turn. Do not look into older turns.
                break
                        
            elif p_type == "token_count":
                rl = payload.get("rate_limits", {})
                pri = rl.get("primary")
                if pri and isinstance(pri, dict):
                    if resets_at is None and pri.get("resets_at"):
                        resets_at = pri.get("resets_at")
                    if reset_window_minutes is None and pri.get("window_minutes"):
                        reset_window_minutes = pri.get("window_minutes")
                reached = rl.get("rate_limit_reached_type")
                if reached:
                    is_usage_limited = True
                        
            if (resets_at or is_usage_limited) and error_msg:
                break
                
        return resets_at, reset_window_minutes, error_msg, is_usage_limited
    except Exception as e:
        return None, None, str(e), False

def is_thread_in_progress(hist_db, thread_id):
    if not hist_db.exists():
        return False
    try:
        conn = sqlite3.connect(f"file:{hist_db}?mode=ro", uri=True)
        cur = conn.cursor()
        cur.execute("SELECT status FROM thread_turns WHERE thread_id = ? ORDER BY started_at DESC LIMIT 1", (thread_id,))
        row = cur.fetchone()
        conn.close()
        return bool(row and row[0] == "inProgress")
    except Exception:
        return False

def detect_interrupted_tasks():
    codex_home_env = os.environ.get("CODEX_HOME")
    codex_home = Path(codex_home_env) if codex_home_env else (Path.home() / ".codex")
    goals_db = codex_home / "goals_1.sqlite"
    state_db = codex_home / "state_5.sqlite"
    hist_db = codex_home / "thread_history_1.sqlite"
    
    tasks = []
    seen_threads = set()
    now_ts = int(time.time())
    
    # 1. Goals
    if goals_db.exists() and state_db.exists():
        try:
            conn_g = sqlite3.connect(f"file:{goals_db}?mode=ro", uri=True)
            conn_s = sqlite3.connect(f"file:{state_db}?mode=ro", uri=True)
            cur_g = conn_g.cursor()
            cur_s = conn_s.cursor()
            
            cur_g.execute("SELECT thread_id, goal_id, objective, status, updated_at_ms FROM thread_goals WHERE status IN ('usage_limited', 'blocked')")
            for row in cur_g.fetchall():
                thread_id, goal_id, objective, g_status, updated_at_ms = row
                seen_threads.add(thread_id)
                
                if is_thread_in_progress(hist_db, thread_id):
                    continue
                
                cur_s.execute("SELECT cwd, title, model, rollout_path FROM threads WHERE id = ?", (thread_id,))
                s_row = cur_s.fetchone()
                cwd = s_row[0] if s_row else None
                title = s_row[1] if s_row and s_row[1] else objective
                model = s_row[2] if s_row else None
                rollout_path = s_row[3] if s_row else None
                
                resets_at, reset_window_minutes, error_msg, is_limited = parse_rollout_tail(rollout_path)
                
                if not (g_status == "usage_limited" or is_limited):
                    continue
                
                tasks.append({
                    "id": f"ssh:{thread_id}",
                    "source": "ssh",
                    "thread_id": thread_id,
                    "goal_id": goal_id,
                    "is_goal": True,
                    "title": title,
                    "objective": objective,
                    "cwd": cwd,
                    "model": model,
                    "status": "usage_limited",
                    "error_message": error_msg,
                    "resets_at": resets_at,
                    "reset_window_minutes": reset_window_minutes,
                    "updated_at": int(updated_at_ms / 1000) if updated_at_ms else now_ts,
                    "can_resume": False
                })
            conn_g.close()
            conn_s.close()
        except Exception:
            pass

    # 2. Regular tasks
    if state_db.exists():
        try:
            conn_s = sqlite3.connect(f"file:{state_db}?mode=ro", uri=True)
            cur_s = conn_s.cursor()
            cur_s.execute("""
                SELECT id, cwd, title, model, rollout_path, updated_at
                FROM threads
                WHERE archived = 0
                  AND (thread_source IS NULL OR thread_source NOT IN ('guardian_review', 'subagent', 'automation'))
                  AND (source IS NULL OR source NOT LIKE '%"subagent"%')
                  AND (title IS NULL OR title != 'Guardian review')
                  AND updated_at > ?
                ORDER BY updated_at DESC LIMIT 50
            """, (now_ts - 7 * 86400,))
            for row in cur_s.fetchall():
                t_id, cwd, title, model, rollout_path, updated_at = row
                if t_id in seen_threads:
                    continue
                if is_thread_in_progress(hist_db, t_id):
                    continue
                resets_at, reset_window_minutes, error_msg, is_limited = parse_rollout_tail(rollout_path)
                if is_limited:
                    can_resume = (resets_at is not None) and (now_ts >= resets_at)
                    tasks.append({
                        "id": f"ssh:{t_id}",
                        "source": "ssh",
                        "thread_id": t_id,
                        "goal_id": None,
                        "is_goal": False,
                        "title": title or "Untitled Task",
                        "objective": None,
                        "cwd": cwd,
                        "model": model,
                        "status": "usage_limited",
                        "error_message": error_msg,
                        "resets_at": resets_at,
                        "reset_window_minutes": reset_window_minutes,
                        "updated_at": updated_at or now_ts,
                        "can_resume": can_resume
                    })
            conn_s.close()
        except Exception:
            pass
            
    return tasks

print("__CODEX_AUTO_RESUME_JSON_START__")
print(json.dumps(detect_interrupted_tasks(), ensure_ascii=False))
print("__CODEX_AUTO_RESUME_JSON_END__")
"#;

const PYTHON_RESUME_HELPER: &str = r#"
import socket, json, os, time, sqlite3, base64, sys, subprocess, struct, uuid
from pathlib import Path

def resume_desktop_owner(codex_home, thread_id, prompt):
    ipc_path = codex_home / "ipc" / "ipc.sock"
    if not ipc_path.exists():
        return None
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(12)
    owner_found = False
    try:
        sock.connect(str(ipc_path))
        client_id = "uninitialized"
        def read_exact(length):
            data = bytearray()
            while len(data) < length:
                chunk = sock.recv(length - len(data))
                if not chunk: raise EOFError("Desktop IPC closed")
                data.extend(chunk)
            return bytes(data)
        def send(message):
            data = json.dumps(message).encode("utf-8")
            sock.sendall(struct.pack("<I", len(data)) + data)
        def request(method, params, version=0, target=None):
            request_id = str(uuid.uuid4())
            message = {"type": "request", "requestId": request_id,
                       "sourceClientId": client_id, "method": method,
                       "params": params, "version": version, "timeoutMs": 10000}
            if target: message["targetClientId"] = target
            send(message)
            deadline = time.monotonic() + 12
            while time.monotonic() < deadline:
                sock.settimeout(max(0.1, deadline - time.monotonic()))
                length = struct.unpack("<I", read_exact(4))[0]
                if not 0 < length <= 16777216:
                    raise ValueError("Invalid desktop IPC frame length")
                reply = json.loads(read_exact(length))
                if reply.get("type") == "client-discovery-request":
                    send({"type": "client-discovery-response", "requestId": reply["requestId"],
                          "response": {"canHandle": False}})
                if reply.get("type") == "response" and reply.get("requestId") == request_id:
                    return reply
            raise TimeoutError("Desktop IPC request timed out")
        initialized = request("initialize", {"clientType": "cockpit-tools"})
        client_id = initialized["result"]["clientId"]
        owner = request("thread-owner-discovery", {"hostId": "local", "conversationId": thread_id}, 1)
        if owner.get("resultType") != "success":
            return None
        owner_found = True
        reply = request("thread-follower-start-turn", {
            "conversationId": thread_id,
            "turnStart": {"request": {"threadId": thread_id,
                "input": [{"type": "text", "text": prompt}]},
                "context": {"inheritThreadSettings": True}}
        }, 2, owner["handledByClientId"])
        if reply.get("resultType") != "success":
            return {"success": False, "mode": "desktop-ipc", "message": str(reply.get("error", "Desktop recovery rejected"))}
        turn = reply.get("result", {}).get("result", {}).get("turn", {})
        if not turn.get("id"):
            return {"success": False, "mode": "desktop-ipc", "message": "Desktop did not acknowledge a new turn"}
        return {"success": True, "mode": "desktop-ipc", "message": f"已通过桌面会话恢复任务: {thread_id}, turn={turn['id']}"}
    except Exception as error:
        if owner_found:
            # A turn may have been accepted before a timeout. Never launch a
            # second writer after contacting the existing desktop owner.
            return {"success": False, "mode": "desktop-ipc", "message": str(error)}
        return None
    finally:
        sock.close()

def resume_thread(thread_id, is_goal, prompt, cwd=None, model=None):
    codex_home_env = os.environ.get("CODEX_HOME")
    codex_home = Path(codex_home_env) if codex_home_env else (Path.home() / ".codex")
    goals_db = codex_home / "goals_1.sqlite"
    sock_path = codex_home / "app-server-control" / "app-server-control.sock"

    desktop_result = resume_desktop_owner(codex_home, thread_id, prompt)
    if desktop_result is not None:
        return desktop_result

    # 1. Update goals db if goal to make status active
    if is_goal and goals_db.exists():
        try:
            conn = sqlite3.connect(str(goals_db))
            cur = conn.cursor()
            cur.execute("UPDATE thread_goals SET status = 'active', updated_at_ms = ? WHERE thread_id = ?", (int(time.time()*1000), thread_id))
            conn.commit()
            conn.close()
        except Exception:
            pass

    # 2. Check app-server socket
    connected = False
    s = None
    if sock_path.exists():
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.settimeout(6.0)
            s.connect(str(sock_path))
            connected = True
        except Exception:
            connected = False
            if s:
                try: s.close()
                except: pass

    if connected:
        try:
            key = base64.b64encode(os.urandom(16)).decode('ascii')
            req = (
                f"GET / HTTP/1.1\r\n"
                f"Host: localhost\r\n"
                f"Upgrade: websocket\r\n"
                f"Connection: Upgrade\r\n"
                f"Sec-WebSocket-Key: {key}\r\n"
                f"Sec-WebSocket-Version: 13\r\n\r\n"
            )
            s.sendall(req.encode('ascii'))
            resp = s.recv(4096).decode('latin1')

            def send_ws(msg):
                data = msg.encode('utf-8')
                header = bytearray([0x81])
                length = len(data)
                mask_key = os.urandom(4)
                if length < 126:
                    header.append(0x80 | length)
                elif length < 65536:
                    header.append(0x80 | 126)
                    header.extend(length.to_bytes(2, 'big'))
                else:
                    header.append(0x80 | 127)
                    header.extend(length.to_bytes(8, 'big'))
                header.extend(mask_key)
                masked = bytearray(data[i] ^ mask_key[i % 4] for i in range(length))
                s.sendall(header + masked)

            def read_ws_msg():
                b1, b2 = s.recv(2)
                masked = bool(b2 & 0x80)
                length = b2 & 0x7F
                if length == 126:
                    length = int.from_bytes(s.recv(2), 'big')
                elif length == 127:
                    length = int.from_bytes(s.recv(8), 'big')
                mask_key = s.recv(4) if masked else b''
                buf = bytearray()
                remaining = length
                while remaining > 0:
                    chunk = s.recv(min(65536, remaining))
                    if not chunk: break
                    buf.extend(chunk)
                    remaining -= len(chunk)
                if masked:
                    for i in range(len(buf)):
                        buf[i] ^= mask_key[i % 4]
                return buf.decode('utf-8', errors='ignore')

            # Initialize
            send_ws(json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"cockpit-tools","version":"1.0"}}}))
            while True:
                msg = json.loads(read_ws_msg())
                if msg.get("id") == 1:
                    break

            # Resume thread
            send_ws(json.dumps({"jsonrpc":"2.0","id":2,"method":"thread/resume","params":{"threadId":thread_id}}))
            while True:
                msg = json.loads(read_ws_msg())
                if msg.get("id") == 2:
                    if "error" in msg:
                        err_obj = msg["error"]
                        err_msg = err_obj.get("message") if isinstance(err_obj, dict) else str(err_obj)
                        s.close()
                        return {"success": False, "message": f"Codex 会话恢复失败: {err_msg}"}
                    break

            # Start turn with input array: [{"type": "text", "text": prompt}]
            turn_params = {
                "threadId": thread_id,
                "input": [{"type": "text", "text": prompt}]
            }
            if cwd: turn_params["cwd"] = cwd
            if model: turn_params["model"] = model

            send_ws(json.dumps({"jsonrpc":"2.0","id":3,"method":"turn/start","params":turn_params}))
            while True:
                msg = json.loads(read_ws_msg())
                if msg.get("id") == 3:
                    if "error" in msg:
                        err_obj = msg["error"]
                        err_msg = err_obj.get("message") if isinstance(err_obj, dict) else str(err_obj)
                        s.close()
                        return {"success": False, "message": f"Codex 启动轮次失败: {err_msg}"}
                    break
            s.close()
            return {"success": True, "mode": "app-server", "message": f"已通过 Codex 守护进程成功恢复并启动任务: {thread_id}"}
        except Exception as e:
            if s:
                try: s.close()
                except: pass
            if connected:
                return {"success": False, "message": f"Codex 通信异常: {str(e)}"}

    # Fallback to CLI only if app-server socket is not connected
    import shutil
    candidates = [shutil.which("codex"), str(Path.home() / ".local/bin/codex"),
        "/opt/homebrew/bin/codex", "/usr/local/bin/codex"]
    for root in [Path.home() / "Applications", Path("/Applications")]:
        for name in ["ChatGPT.app", "Codex.app"]:
            resources = root / name / "Contents/Resources"
            candidates.extend([str(resources / "codex"), str(resources / "codex-cli/bin/codex")])
    codex_bin = next((path for path in candidates if path and os.path.isfile(path) and os.access(path, os.X_OK)), None)
    if not codex_bin:
        return {"success": False, "message": "No executable Codex CLI or desktop owner found"}
    target_cwd = cwd if (cwd and os.path.isdir(cwd)) else str(Path.home())
    log_file = codex_home / f"auto_resume_{thread_id}.log"
    cmd = [codex_bin, "exec", "resume", "--skip-git-repo-check"]
    if model: cmd.extend(["--model", model])
    cmd.extend([thread_id, prompt])
    with open(log_file, "a") as f:
        child = subprocess.Popen(cmd, cwd=target_cwd, stdout=f, stderr=f, stdin=subprocess.DEVNULL, start_new_session=True)
    time.sleep(1)
    if child.poll() is not None and child.returncode != 0:
        return {"success": False, "message": f"Codex exited with code {child.returncode}; see {log_file}"}
    return {"success": True, "mode": "cli", "message": f"已通过 Codex CLI 后台启动恢复任务: {thread_id}"}

try:
    raw_arg = sys.argv[1]
    try:
        raw_arg = base64.b64decode(raw_arg).decode("utf-8")
    except Exception:
        pass
    data = json.loads(raw_arg)
    res = resume_thread(
        thread_id=data["thread_id"],
        is_goal=data.get("is_goal", False),
        prompt=data.get("prompt", "Continue working toward the active thread goal."),
        cwd=data.get("cwd"),
        model=data.get("model")
    )
except Exception as e:
    res = {"success": False, "message": str(e)}

print("__CODEX_RESUME_RESULT_JSON_START__")
print(json.dumps(res, ensure_ascii=False))
print("__CODEX_RESUME_RESULT_JSON_END__")
"#;

fn parse_rollout_tail(path: &Path) -> (Option<i64>, Option<i64>, Option<String>, bool) {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (None, None, None, false),
    };
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let read_len = 65536.min(file_len);
    if read_len > 0 && file.seek(SeekFrom::End(-(read_len as i64))).is_err() {
        return (None, None, None, false);
    }
    let mut buf = vec![0u8; read_len as usize];
    if file.read_exact(&mut buf).is_err() {
        return (None, None, None, false);
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    lines.reverse();

    let mut resets_at = None;
    let mut reset_window_minutes = None;
    let mut error_msg = None;
    let mut is_usage_limited = false;
    let mut has_task_complete = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            let payload = val.get("payload");
            let p_type = payload.and_then(|p| p.get("type")).and_then(|t| t.as_str());

            if p_type == Some("task_complete") {
                if has_task_complete {
                    break;
                }
                has_task_complete = true;

                if let Some(err) = payload.and_then(|p| p.get("error")) {
                    if let Some(msg) = err.get("message").and_then(|m| m.as_str()) {
                        if error_msg.is_none() {
                            error_msg = Some(msg.to_string());
                        }
                    }
                    if let Some(code) = err.get("codex_error_info").and_then(|c| c.as_str()) {
                        if code == "usage_limit_exceeded"
                            || error_msg
                                .as_deref()
                                .map(|m| m.to_lowercase().contains("usage limit"))
                                .unwrap_or(false)
                        {
                            is_usage_limited = true;
                        }
                    }
                } else {
                    // Latest turn completed cleanly without error!
                    // Do not scan backwards into older turns for stale errors.
                    break;
                }
            } else if p_type == Some("task_started") {
                // Reached start of current turn; stop scanning older turns.
                break;
            } else if p_type == Some("token_count") {
                if let Some(rl) = payload.and_then(|p| p.get("rate_limits")) {
                    if let Some(pri) = rl.get("primary") {
                        if resets_at.is_none() {
                            resets_at = pri.get("resets_at").and_then(|r| r.as_i64());
                        }
                        if reset_window_minutes.is_none() {
                            reset_window_minutes =
                                pri.get("window_minutes").and_then(|w| w.as_i64());
                        }
                    }
                    if rl.get("rate_limit_reached_type").is_some() {
                        is_usage_limited = true;
                    }
                }
            }

            if (resets_at.is_some() || is_usage_limited) && error_msg.is_some() {
                break;
            }
        }
    }

    (resets_at, reset_window_minutes, error_msg, is_usage_limited)
}

fn is_thread_in_progress(hist_db: &Path, thread_id: &str) -> bool {
    if !hist_db.exists() {
        return false;
    }
    let conn = rusqlite::Connection::open_with_flags(
        hist_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    );
    if let Ok(conn) = conn {
        if let Ok(mut stmt) = conn.prepare("SELECT status FROM thread_turns WHERE thread_id = ? ORDER BY started_at DESC LIMIT 1") {
            if let Ok(status) = stmt.query_row([thread_id], |row| row.get::<_, String>(0)) {
                return status == "inProgress";
            }
        }
    }
    false
}

/// 获取当前 Codex 账号的 5 小时配额重置时间戳（秒）及是否允许恢复
pub fn get_current_codex_quota_resets_at() -> (Option<i64>, Option<i64>, bool) {
    let now_ts = Utc::now().timestamp();
    let account = match crate::modules::codex_account::get_current_account() {
        Some(acc) => acc,
        None => return (None, None, false),
    };
    let quota = match account.quota {
        Some(q) => q,
        None => return (None, None, true),
    };

    let hourly_reset = quota.hourly_reset_time;
    let hourly_window = quota.hourly_window_minutes.or(Some(300));
    let weekly_reset = quota.weekly_reset_time;

    let hourly_exhausted = if quota.hourly_percentage <= 0 {
        match hourly_reset {
            Some(reset_ts) => now_ts < reset_ts,
            None => true,
        }
    } else {
        false
    };

    let weekly_exhausted = if quota.weekly_percentage <= 0 {
        match weekly_reset {
            Some(reset_ts) => now_ts < reset_ts,
            None => true,
        }
    } else {
        false
    };

    let mut is_blocked = false;
    let mut target_reset_ts: Option<i64> = None;
    let mut target_window = hourly_window;

    if hourly_exhausted {
        is_blocked = true;
        target_reset_ts = hourly_reset;
    }

    if weekly_exhausted {
        is_blocked = true;
        target_reset_ts = match target_reset_ts {
            Some(cur) => match weekly_reset {
                Some(w_ts) => Some(cur.max(w_ts)),
                None => Some(cur),
            },
            None => {
                target_window = quota.weekly_window_minutes;
                weekly_reset
            }
        };
    }

    if is_blocked {
        (target_reset_ts, target_window, false)
    } else {
        (hourly_reset, hourly_window, true)
    }
}

/// 计算一组候选账号中最早可用的 5 小时配额重置时间戳、窗口时长、是否可恢复及对应账号邮箱
pub fn compute_earliest_quota_reset_from_accounts(
    now_ts: i64,
    current_account: Option<&CodexAccount>,
    accounts: &[CodexAccount],
    monitored_account_ids: Option<&std::collections::HashSet<String>>,
) -> (Option<i64>, Option<i64>, bool, Option<String>) {
    struct CandidateInfo {
        email: String,
        is_current: bool,
        can_resume: bool,
        hourly_reset_ts: Option<i64>,
        hourly_window: Option<i64>,
        is_weekly_blocked: bool,
        weekly_reset_ts: Option<i64>,
    }

    let mut candidates: Vec<CandidateInfo> = Vec::new();

    for acc in accounts {
        if acc.is_api_key_auth() || acc.is_agent_identity_auth() || acc.is_web_session_auth() {
            continue;
        }

        let is_curr = current_account.map(|c| c.id == acc.id).unwrap_or(false);
        if let Some(set) = monitored_account_ids {
            if !set.contains(&acc.id) && !is_curr {
                continue;
            }
        }

        let quota = match &acc.quota {
            Some(q) => q,
            None => {
                if is_curr {
                    return (None, None, true, Some(acc.email.clone()));
                }
                continue;
            }
        };

        let hourly_window = quota.hourly_window_minutes.or(Some(300));
        let hourly_reset = quota.hourly_reset_time;
        let weekly_reset = quota.weekly_reset_time;

        let hourly_exhausted = if quota.hourly_percentage <= 0 {
            match hourly_reset {
                Some(reset_ts) => now_ts < reset_ts,
                None => true,
            }
        } else {
            false
        };

        let weekly_exhausted = if quota.weekly_percentage <= 0 {
            match weekly_reset {
                Some(reset_ts) => now_ts < reset_ts,
                None => true,
            }
        } else {
            false
        };

        let can_resume = !hourly_exhausted && !weekly_exhausted;

        candidates.push(CandidateInfo {
            email: acc.email.clone(),
            is_current: is_curr,
            can_resume,
            hourly_reset_ts: hourly_reset,
            hourly_window,
            is_weekly_blocked: weekly_exhausted,
            weekly_reset_ts: weekly_reset,
        });
    }

    // 1. 若当前账号已可恢复，优先使用当前账号
    if let Some(curr) = candidates.iter().find(|c| c.is_current && c.can_resume) {
        return (curr.hourly_reset_ts, curr.hourly_window, true, Some(curr.email.clone()));
    }

    // 2. 若其它候选账号中有已恢复/有额度的，整体可立即恢复
    if let Some(ready) = candidates.iter().find(|c| c.can_resume) {
        return (ready.hourly_reset_ts, ready.hourly_window, true, Some(ready.email.clone()));
    }

    // 3. 所有账号均未到恢复状态：在周配额未受限的账号中，按 5 小时重置时间升序排列，取最早者
    let mut hourly_candidates: Vec<&CandidateInfo> = candidates
        .iter()
        .filter(|c| !c.is_weekly_blocked && c.hourly_reset_ts.is_some())
        .collect();

    if !hourly_candidates.is_empty() {
        hourly_candidates.sort_by(|a, b| {
            let a_ts = a.hourly_reset_ts.unwrap();
            let b_ts = b.hourly_reset_ts.unwrap();
            a_ts.cmp(&b_ts)
                .then_with(|| b.is_current.cmp(&a.is_current))
        });

        let earliest = hourly_candidates[0];
        return (
            earliest.hourly_reset_ts,
            earliest.hourly_window,
            false,
            Some(earliest.email.clone()),
        );
    }

    // 4. 若周配额均受限或无 5 小时重置时间，回退检查周重置时间
    let mut weekly_candidates: Vec<&CandidateInfo> = candidates
        .iter()
        .filter(|c| c.weekly_reset_ts.is_some())
        .collect();

    if !weekly_candidates.is_empty() {
        weekly_candidates.sort_by(|a, b| {
            let a_ts = a.weekly_reset_ts.unwrap();
            let b_ts = b.weekly_reset_ts.unwrap();
            a_ts.cmp(&b_ts)
                .then_with(|| b.is_current.cmp(&a.is_current))
        });

        let earliest = weekly_candidates[0];
        return (
            earliest.weekly_reset_ts,
            earliest.hourly_window,
            false,
            Some(earliest.email.clone()),
        );
    }

    // 5. 兜底回退
    if let Some(curr) = current_account {
        let (cur_reset, cur_window, cur_can) = get_current_codex_quota_resets_at();
        (cur_reset, cur_window, cur_can, Some(curr.email.clone()))
    } else {
        (None, None, false, None)
    }
}

/// 获取所有可用账号中最早的 5 小时配额重置时间戳（秒）、窗口时长、是否已有可用配额，以及最早重置的账号邮箱
pub fn get_earliest_codex_quota_resets_at() -> (Option<i64>, Option<i64>, bool, Option<String>) {
    let now_ts = Utc::now().timestamp();
    let current_account = crate::modules::codex_account::get_current_account();
    let accounts = crate::modules::codex_account::list_accounts();
    let cfg = config::get_user_config();

    let account_scope_mode = crate::modules::codex_account::normalize_auto_switch_account_scope_mode(
        &cfg.codex_auto_switch_account_scope_mode,
    );
    let monitored_account_ids: Option<std::collections::HashSet<String>> = if cfg.codex_auto_switch_enabled
        && account_scope_mode == "selected"
    {
        Some(
            crate::modules::codex_account::resolve_monitored_auto_switch_account_ids(
                &account_scope_mode,
                &cfg.codex_auto_switch_selected_account_ids,
                &accounts,
            )
            .into_iter()
            .collect(),
        )
    } else {
        None
    };

    compute_earliest_quota_reset_from_accounts(
        now_ts,
        current_account.as_ref(),
        &accounts,
        monitored_account_ids.as_ref(),
    )
}

pub fn detect_local_tasks() -> Vec<InterruptedCodexTask> {
    let mut tasks = Vec::new();
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return tasks,
    };
    let codex_home = home.join(".codex");
    let goals_db = codex_home.join("goals_1.sqlite");
    let state_db = codex_home.join("state_5.sqlite");
    let hist_db = codex_home.join("thread_history_1.sqlite");
    let now_ts = Utc::now().timestamp();
    let mut seen_threads = std::collections::HashSet::new();
    let (earliest_resets_at, earliest_window, earliest_can_resume, earliest_account_email) =
        get_earliest_codex_quota_resets_at();

    // 1. Check thread_goals in goals_1.sqlite
    if goals_db.exists() && state_db.exists() {
        let conn_g = rusqlite::Connection::open_with_flags(
            &goals_db,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        );
        let conn_s = rusqlite::Connection::open_with_flags(
            &state_db,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        );

        if let (Ok(conn_g), Ok(conn_s)) = (conn_g, conn_s) {
            let stmt = conn_g.prepare(
                "SELECT thread_id, goal_id, objective, status, updated_at_ms FROM thread_goals WHERE status IN ('usage_limited', 'blocked')",
            );
            if let Ok(mut stmt) = stmt {
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                    ))
                });
                if let Ok(rows) = rows {
                    for item in rows.flatten() {
                        let (thread_id, goal_id, objective, g_status, updated_at_ms) = item;
                        seen_threads.insert(thread_id.clone());

                        if is_thread_in_progress(&hist_db, &thread_id) {
                            continue;
                        }

                        let s_stmt = conn_s.prepare(
                            "SELECT cwd, title, model, rollout_path FROM threads WHERE id = ?",
                        );
                        let mut cwd = None;
                        let mut title = objective.clone();
                        let mut model = None;
                        let mut rollout_path = None;

                        if let Ok(mut s_stmt) = s_stmt {
                            let s_res = s_stmt.query_row([&thread_id], |row| {
                                Ok((
                                    row.get::<_, Option<String>>(0)?,
                                    row.get::<_, Option<String>>(1)?,
                                    row.get::<_, Option<String>>(2)?,
                                    row.get::<_, Option<String>>(3)?,
                                ))
                            });
                            if let Ok((c, t, m, r)) = s_res {
                                cwd = c;
                                if let Some(t_val) = t {
                                    if !t_val.trim().is_empty() {
                                        title = t_val;
                                    }
                                }
                                model = m;
                                rollout_path = r;
                            }
                        }

                        let (_resets_at, _reset_window_minutes, error_msg, is_limited) =
                            if let Some(ref r_path) = rollout_path {
                                parse_rollout_tail(Path::new(r_path))
                            } else {
                                (None, None, None, false)
                            };

                        if g_status != "usage_limited" && !is_limited {
                            continue;
                        }

                        let updated_at = updated_at_ms.map(|ms| ms / 1000).unwrap_or(now_ts);

                        tasks.push(InterruptedCodexTask {
                            id: format!("local:{}", thread_id),
                            source: "local".to_string(),
                            server_id: None,
                            server_name: None,
                            thread_id,
                            goal_id: Some(goal_id),
                            is_goal: true,
                            title,
                            objective: Some(objective),
                            cwd,
                            model,
                            status: "usage_limited".to_string(),
                            error_message: error_msg,
                            resets_at: earliest_resets_at,
                            reset_window_minutes: earliest_window,
                            reset_account_email: earliest_account_email.clone(),
                            updated_at,
                            can_resume: earliest_can_resume,
                            is_excluded: false,
                        });
                    }
                }
            }
        }
    }

    // 2. Check regular threads in state_5.sqlite
    if state_db.exists() {
        if let Ok(conn_s) = rusqlite::Connection::open_with_flags(
            &state_db,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            let cutoff = now_ts - 7 * 86400;
            if let Ok(mut stmt) = conn_s.prepare(
                "SELECT id, cwd, title, model, rollout_path, updated_at FROM threads WHERE archived = 0 AND (thread_source IS NULL OR thread_source NOT IN ('guardian_review', 'subagent', 'automation')) AND (source IS NULL OR source NOT LIKE '%\"subagent\"%') AND (title IS NULL OR title != 'Guardian review') AND updated_at > ? ORDER BY updated_at DESC LIMIT 50",
            ) {
                let rows = stmt.query_map([cutoff], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                    ))
                });
                if let Ok(rows) = rows {
                    for item in rows.flatten() {
                        let (t_id, cwd, title_opt, model, rollout_path, updated_at_opt) = item;
                        if seen_threads.contains(&t_id) {
                            continue;
                        }
                        if is_thread_in_progress(&hist_db, &t_id) {
                            continue;
                        }
                        let (_resets_at, _reset_window_minutes, error_msg, is_limited) =
                            if let Some(ref r_path) = rollout_path {
                                parse_rollout_tail(Path::new(r_path))
                            } else {
                                (None, None, None, false)
                            };

                        if is_limited {
                            let title = title_opt.unwrap_or_else(|| "未命名会话".to_string());
                            let updated_at = updated_at_opt.unwrap_or(now_ts);

                            tasks.push(InterruptedCodexTask {
                                id: format!("local:{}", t_id),
                                source: "local".to_string(),
                                server_id: None,
                                server_name: None,
                                thread_id: t_id,
                                goal_id: None,
                                is_goal: false,
                                title,
                                objective: None,
                                cwd,
                                model,
                                status: "usage_limited".to_string(),
                                error_message: error_msg,
                                resets_at: earliest_resets_at,
                                reset_window_minutes: earliest_window,
                                reset_account_email: earliest_account_email.clone(),
                                updated_at,
                                can_resume: earliest_can_resume,
                                is_excluded: false,
                            });
                        }
                    }
                }
            }
        }
    }

    tasks
}

pub async fn detect_remote_tasks(server: &SshServer) -> Result<Vec<InterruptedCodexTask>, String> {
    let output = ssh_server::run_ssh(
        server,
        25,
        &["python3", "-"],
        Some(ssh_server::remote_python_with_home(PYTHON_REMOTE_PROBE, "CODEX_HOME", &server.codex_home)),
    )
    .await?;

    let start_tag = "__CODEX_AUTO_RESUME_JSON_START__";
    let end_tag = "__CODEX_AUTO_RESUME_JSON_END__";

    let start_idx = output
        .find(start_tag)
        .ok_or_else(|| "未找到远端输出起始标记".to_string())?
        + start_tag.len();
    let end_idx = output[start_idx..]
        .find(end_tag)
        .ok_or_else(|| "未找到远端输出结束标记".to_string())?
        + start_idx;

    let json_text = output[start_idx..end_idx].trim();
    let mut raw_tasks: Vec<InterruptedCodexTask> = serde_json::from_str(json_text)
        .map_err(|e| format!("解析远端任务 JSON 失败: {}", e))?;

    let (earliest_resets_at, earliest_window, earliest_can_resume, earliest_account_email) =
        get_earliest_codex_quota_resets_at();
    for task in &mut raw_tasks {
        task.source = "ssh".to_string();
        task.server_id = Some(server.id.clone());
        task.server_name = Some(server.name.clone());
        task.id = format!("ssh:{}:{}", server.id, task.thread_id);
        task.resets_at = earliest_resets_at;
        task.reset_window_minutes = earliest_window;
        task.reset_account_email = earliest_account_email.clone();
        task.can_resume = earliest_can_resume;
    }

    Ok(raw_tasks)
}

pub async fn detect_all_interrupted_tasks() -> Vec<InterruptedCodexTask> {
    let mut all_tasks = detect_local_tasks();

    let cfg = config::get_user_config();
    if cfg.codex_auto_resume_sync_ssh {
        match ssh_server::list_servers() {
            Ok(server_list) => {
                for server in server_list.servers {
                    match detect_remote_tasks(&server).await {
                        Ok(mut remote_tasks) => {
                            all_tasks.append(&mut remote_tasks);
                        }
                        Err(err) => {
                            logger::log_warn(&format!(
                                "[CodexAutoResume] 探测 SSH 服务器【{}】中断任务失败: {}",
                                server.name, err
                            ));
                        }
                    }
                }
            }
            Err(e) => {
                logger::log_warn(&format!("[CodexAutoResume] list_servers 失败: {}", e));
            }
        }
    }

    let (earliest_resets_at, earliest_window, earliest_can_resume, earliest_account_email) =
        get_earliest_codex_quota_resets_at();
    let excluded = load_excluded_task_ids();
    let dismissed = load_dismissed_task_ids();

    all_tasks.retain(|t| !dismissed.contains(&t.id) && !dismissed.contains(&t.thread_id));

    for task in &mut all_tasks {
        task.resets_at = earliest_resets_at;
        task.reset_window_minutes = earliest_window;
        task.reset_account_email = earliest_account_email.clone();
        task.can_resume = earliest_can_resume;
        task.is_excluded = excluded.contains(&task.id) || excluded.contains(&task.thread_id);
    }

    all_tasks
}

pub async fn resume_task(
    task: &InterruptedCodexTask,
    custom_prompt: Option<String>,
    trigger_type: &str,
) -> Result<ResumeTaskResult, String> {
    let result = resume_task_internal(task, custom_prompt).await;
    let now_ts = Utc::now().timestamp();
    match &result {
        Ok(res) => {
            record_resumed_task(ResumedTaskRecord {
                id: task.id.clone(),
                source: task.source.clone(),
                server_id: task.server_id.clone(),
                server_name: task.server_name.clone(),
                thread_id: task.thread_id.clone(),
                goal_id: task.goal_id.clone(),
                is_goal: task.is_goal,
                title: task.title.clone(),
                cwd: task.cwd.clone(),
                model: task.model.clone(),
                resumed_at: now_ts,
                trigger_type: trigger_type.to_string(),
                success: true,
                message: res.message.clone(),
            });
        }
        Err(err) => {
            record_resumed_task(ResumedTaskRecord {
                id: task.id.clone(),
                source: task.source.clone(),
                server_id: task.server_id.clone(),
                server_name: task.server_name.clone(),
                thread_id: task.thread_id.clone(),
                goal_id: task.goal_id.clone(),
                is_goal: task.is_goal,
                title: task.title.clone(),
                cwd: task.cwd.clone(),
                model: task.model.clone(),
                resumed_at: now_ts,
                trigger_type: trigger_type.to_string(),
                success: false,
                message: err.clone(),
            });
        }
    }
    result
}

async fn resume_task_internal(
    task: &InterruptedCodexTask,
    custom_prompt: Option<String>,
) -> Result<ResumeTaskResult, String> {
    let prompt = custom_prompt.unwrap_or_else(|| {
        if task.is_goal {
            "Continue working toward the active thread goal.".to_string()
        } else {
            "Continue where you left off and finish the task.".to_string()
        }
    });

    let helper_json = serde_json::json!({
        "thread_id": task.thread_id,
        "is_goal": task.is_goal,
        "prompt": prompt,
        "cwd": task.cwd,
        "model": task.model
    })
    .to_string();
    let b64_json = BASE64_STANDARD.encode(helper_json.as_bytes());

    if task.source == "local" {
        // Attempt using python resume helper first (handles app-server socket & goals_1.sqlite cleanly)
        let child = std::process::Command::new("python3")
            .arg("-")
            .arg(&b64_json)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        if let Ok(mut c) = child {
            if let Some(mut stdin) = c.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(PYTHON_RESUME_HELPER.as_bytes());
            }
            if let Ok(output) = c.wait_with_output() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let start_tag = "__CODEX_RESUME_RESULT_JSON_START__";
                let end_tag = "__CODEX_RESUME_RESULT_JSON_END__";
                if let (Some(s_idx), Some(e_idx)) = (stdout.find(start_tag), stdout.find(end_tag)) {
                    let json_str = stdout[s_idx + start_tag.len()..e_idx].trim();
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                        if val.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                            let msg = val
                                .get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("恢复成功");
                            logger::log_info(&format!(
                                "[CodexAutoResume] 成功启动本地任务恢复: thread_id={}, 结果: {}",
                                task.thread_id, msg
                            ));
                            return Ok(ResumeTaskResult {
                                success: true,
                                message: msg.to_string(),
                            });
                        }
                        if val.get("mode").and_then(|v| v.as_str()) == Some("desktop-ipc") {
                            return Err(val.get("message").and_then(|v| v.as_str())
                                .unwrap_or("Desktop task recovery failed").to_string());
                        }
                    }
                }
            }
        }

        // Fallback to standard CLI spawn if python helper not present or errored
        let binary = crate::modules::codex_wakeup::resolve_binary()
            .map_err(|e| format!("未找到本地 Codex CLI: {}", e.message))?;

        let mut cmd = crate::modules::codex_wakeup::build_binary_command(&binary);
        cmd.arg("exec");
        cmd.arg("resume");
        cmd.arg("--skip-git-repo-check");
        cmd.arg(&task.thread_id);
        cmd.arg(&prompt);

        if let Some(ref cwd) = task.cwd {
            let p = Path::new(cwd);
            if p.is_dir() {
                cmd.current_dir(p);
            }
        }

        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());

        cmd.spawn()
            .map_err(|e| format!("启动本地恢复进程失败: {}", e))?;

        logger::log_info(&format!(
            "[CodexAutoResume] 成功通过 CLI 启动本地任务恢复: thread_id={}",
            task.thread_id
        ));

        Ok(ResumeTaskResult {
            success: true,
            message: format!("已在后台启动恢复本地任务: {}", task.thread_id),
        })
    } else if task.source == "ssh" {
        let server_id = task
            .server_id
            .as_deref()
            .ok_or_else(|| "缺少 SSH 服务器 ID".to_string())?;

        let server_list = ssh_server::list_servers()?;
        let server = server_list
            .servers
            .into_iter()
            .find(|s| s.id == server_id)
            .ok_or_else(|| format!("未找到目标 SSH 服务器: {}", server_id))?;

        let output = ssh_server::run_ssh(
            &server,
            25,
            &["python3", "-", &b64_json],
            Some(ssh_server::remote_python_with_home(PYTHON_RESUME_HELPER, "CODEX_HOME", &server.codex_home)),
        )
        .await?;

        let start_tag = "__CODEX_RESUME_RESULT_JSON_START__";
        let end_tag = "__CODEX_RESUME_RESULT_JSON_END__";

        if let (Some(s_idx), Some(e_idx)) = (output.find(start_tag), output.find(end_tag)) {
            let json_str = output[s_idx + start_tag.len()..e_idx].trim();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                if val.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                    let msg = val
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("恢复成功");
                    logger::log_info(&format!(
                        "[CodexAutoResume] 成功在 SSH 服务器【{}】恢复任务: thread_id={}, 结果: {}",
                        server.name, task.thread_id, msg
                    ));
                    return Ok(ResumeTaskResult {
                        success: true,
                        message: format!("已在服务器【{}】恢复任务: {}", server.name, msg),
                    });
                } else {
                    let err = val
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("远端 Python 执行失败");
                    return Err(err.to_string());
                }
            }
        }

        Err(format!("SSH server {} did not return a valid recovery result", server.name))
    } else {
        Err(format!("未知任务来源: {}", task.source))
    }
}

static LAST_EARLIER_RESET_SWITCH_TS: OnceLock<Mutex<i64>> = OnceLock::new();
static EARLIER_RESET_SWITCH_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
struct EarlierResetCandidate {
    account: CodexAccount,
    has_positive_quota: bool,
    hourly_reset_time: i64,
    hourly_percentage: i32,
    last_used: i64,
}

/// 当所有账号用尽 5 小时配额且任务处于受限中断中时，
/// 若其它账号的 5 小时重置时间早于当前账号且已到达，自动切换至该优先重置的账号以恢复任务。
pub async fn check_and_auto_switch_for_earlier_reset(app: &AppHandle) -> Result<bool, String> {
    let cfg = config::get_user_config();
    if !cfg.codex_auto_resume_interrupted_tasks_enabled {
        return Ok(false);
    }

    if EARLIER_RESET_SWITCH_IN_PROGRESS.swap(true, Ordering::SeqCst) {
        return Ok(false);
    }

    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            EARLIER_RESET_SWITCH_IN_PROGRESS.store(false, Ordering::SeqCst);
        }
    }
    let _guard = Guard;

    let now_ts = Utc::now().timestamp();
    {
        let last_ts_mutex = LAST_EARLIER_RESET_SWITCH_TS.get_or_init(|| Mutex::new(0));
        let last_ts = *last_ts_mutex.lock().unwrap_or_else(|e| e.into_inner());
        if now_ts - last_ts < 60 {
            return Ok(false);
        }
    }

    // 1. 检查是否存在待恢复的中断任务
    let tasks = detect_all_interrupted_tasks().await;
    let excluded = load_excluded_task_ids();
    let dismissed = load_dismissed_task_ids();
    let has_interrupted_tasks = tasks.iter().any(|t| !t.is_excluded)
        || retry_state().lock().unwrap_or_else(|e| e.into_inner()).values().any(|retry| {
            !excluded.contains(&retry.task.id) && !excluded.contains(&retry.task.thread_id)
                && !dismissed.contains(&retry.task.id) && !dismissed.contains(&retry.task.thread_id)
                && (retry.task.source == "local" || cfg.codex_auto_resume_sync_ssh)
        });
    if !has_interrupted_tasks {
        return Ok(false);
    }

    // 2. 检查当前账号状态：若当前账号已可恢复，则无需切换
    let current_account = match crate::modules::codex_account::get_current_account() {
        Some(acc) => acc,
        None => return Ok(false),
    };

    let (_resets_at, _window, current_can_resume) = get_current_codex_quota_resets_at();
    if current_can_resume {
        return Ok(false);
    }

    let current_reset_ts = current_account
        .quota
        .as_ref()
        .and_then(|q| q.hourly_reset_time)
        .unwrap_or(i64::MAX);

    // 3. 收集候选账号
    let accounts = crate::modules::codex_account::list_accounts();
    let account_scope_mode = crate::modules::codex_account::normalize_auto_switch_account_scope_mode(
        &cfg.codex_auto_switch_account_scope_mode,
    );
    let monitored_account_ids = if cfg.codex_auto_switch_enabled
        && account_scope_mode == "selected"
    {
        crate::modules::codex_account::resolve_monitored_auto_switch_account_ids(
            &account_scope_mode,
            &cfg.codex_auto_switch_selected_account_ids,
            &accounts,
        )
    } else {
        accounts.iter().map(|a| a.id.clone()).collect()
    };

    let mut candidates: Vec<EarlierResetCandidate> = Vec::new();

    for account in accounts {
        if account.id == current_account.id {
            continue;
        }
        if !monitored_account_ids.contains(&account.id) {
            continue;
        }
        if account.is_api_key_auth()
            || account.is_agent_identity_auth()
            || account.is_web_session_auth()
        {
            continue;
        }

        let quota = match &account.quota {
            Some(q) => q,
            None => continue,
        };

        // 检查周配额是否受限
        let weekly_blocked = if quota.weekly_percentage <= 0 {
            match quota.weekly_reset_time {
                Some(w_ts) => now_ts < w_ts,
                None => true,
            }
        } else {
            false
        };
        if weekly_blocked {
            continue;
        }

        let hourly_pct = quota.hourly_percentage;
        let hourly_reset = quota.hourly_reset_time;

        let has_positive = hourly_pct > 0;
        let is_eligible = if has_positive {
            true
        } else if let Some(reset_time) = hourly_reset {
            reset_time <= now_ts && reset_time < current_reset_ts
        } else {
            false
        };

        if !is_eligible {
            continue;
        }

        let effective_reset_ts = if has_positive {
            0
        } else {
            hourly_reset.unwrap_or(i64::MAX)
        };

        candidates.push(EarlierResetCandidate {
            account: account.clone(),
            has_positive_quota: has_positive,
            hourly_reset_time: effective_reset_ts,
            hourly_percentage: hourly_pct,
            last_used: account.last_used,
        });
    }

    if candidates.is_empty() {
        return Ok(false);
    }

    // 排序：正额度优先 -> 重置时间最早(升序) -> 额度百分比最高(降序) -> 最久未用(升序)
    candidates.sort_by(|a, b| {
        b.has_positive_quota
            .cmp(&a.has_positive_quota)
            .then_with(|| a.hourly_reset_time.cmp(&b.hourly_reset_time))
            .then_with(|| b.hourly_percentage.cmp(&a.hourly_percentage))
            .then_with(|| a.last_used.cmp(&b.last_used))
    });

    for cand in candidates {
        let is_ready = if cand.hourly_percentage > 0 {
            true
        } else {
            match crate::modules::codex_quota::refresh_account_quota(&cand.account.id).await {
                Ok(refreshed) => {
                    if refreshed.hourly_percentage > 0 {
                        true
                    } else if let Some(r_ts) = refreshed.hourly_reset_time {
                        now_ts >= r_ts
                    } else {
                        false
                    }
                }
                Err(err) => {
                    logger::log_warn(&format!(
                        "[CodexAutoResume] 刷新候选账号 {} 配额失败: {}",
                        cand.account.id, err
                    ));
                    now_ts >= cand.hourly_reset_time + 60
                }
            }
        };

        if !is_ready {
            continue;
        }

        logger::log_info(&format!(
            "[CodexAutoResume] 发现优先重置配额账号，执行自动切号: 当前账号={}, 候选账号={}(reset_ts={})",
            current_account.email, cand.account.email, cand.hourly_reset_time
        ));

        let switch_res = Box::pin(crate::commands::codex::switch_codex_account(
            app.clone(),
            cand.account.id.clone(),
            None,
            None,
            None,
        ))
        .await;

        match switch_res {
            Ok(switched) => {
                {
                    let last_ts_mutex = LAST_EARLIER_RESET_SWITCH_TS.get_or_init(|| Mutex::new(0));
                    *last_ts_mutex.lock().unwrap_or_else(|e| e.into_inner()) = Utc::now().timestamp();
                }

                logger::log_info(&format!(
                    "[CodexAutoResume] 优先重置切号成功: from={}, to={}",
                    current_account.email, switched.email
                ));

                let title = "Codex 账号已自动切换";
                let body = format!(
                    "原账号仍需等待配额重置，已自动切换至优先重置的账号【{}】以恢复中断任务",
                    switched.email
                );

                #[cfg(target_os = "macos")]
                {
                    let bundle_identifier = app.config().identifier.clone();
                    let mut notification = mac_notification_sys::Notification::new();
                    notification.title(title).message(&body);
                    let _ = mac_notification_sys::set_application(&bundle_identifier);
                    let _ = notification.send();
                }

                #[cfg(not(target_os = "macos"))]
                {
                    use tauri_plugin_notification::NotificationExt;
                    let _ = app
                        .notification()
                        .builder()
                        .title(title)
                        .body(&body)
                        .show();
                }

                let _ = app.emit(
                    "codex://account-switched-for-earlier-reset",
                    serde_json::json!({
                        "fromAccountId": current_account.id,
                        "fromAccountEmail": current_account.email,
                        "toAccountId": switched.id,
                        "toAccountEmail": switched.email,
                    }),
                );

                return Ok(true);
            }
            Err(err) => {
                logger::log_warn(&format!(
                    "[CodexAutoResume] 切换至优先重置账号 {} 失败: {}",
                    cand.account.id, err
                ));
            }
        }
    }

    Ok(false)
}

pub async fn check_and_auto_resume(app: &AppHandle) {
    let cfg = config::get_user_config();
    if !cfg.codex_auto_resume_interrupted_tasks_enabled {
        return;
    }

    let (_resets_at, _window, current_can_resume) = get_current_codex_quota_resets_at();
    if !current_can_resume {
        match check_and_auto_switch_for_earlier_reset(app).await {
            Ok(true) => {
                logger::log_info("[CodexAutoResume] 已自动切号至优先重置的账号，立即恢复中断任务");
            }
            Ok(false) => {}
            Err(e) => {
                logger::log_warn(&format!("[CodexAutoResume] 优先重置切号检查失败: {}", e));
            }
        }
    }

    // Re-read the active account after switching. Another account's available quota
    // must not cause a resume while the active credentials are still limited.
    if !get_current_codex_quota_resets_at().2 {
        return;
    }
    resume_available_interrupted_tasks(app).await;
}

#[derive(Debug, PartialEq)]
enum RecoveryProgress { Running, Finished, Retry }

// Launch acknowledgement is not completion. Watch the latest turn and CLI
// process so failures after launch remain eligible without duplicating live work.
const PYTHON_RECOVERY_PROGRESS: &str = r#"
import base64, json, os, sqlite3, subprocess, sys
from pathlib import Path
from datetime import datetime
args = json.loads(base64.b64decode(sys.argv[1]))
home = Path(os.environ.get("CODEX_HOME") or str(Path.home() / ".codex"))
thread = args["thread_id"]
since = args["since"]
result = "retry"
def timestamp(value):
    try:
        n = float(value)
        return n / 1000 if n > 100000000000 else n
    except (ValueError, TypeError):
        try: return datetime.fromisoformat(str(value).replace("Z", "+00:00")).timestamp()
        except Exception: return 0
try:
    db = sqlite3.connect(f"file:{home / 'thread_history_1.sqlite'}?mode=ro", uri=True)
    row = db.execute("SELECT status, started_at FROM thread_turns WHERE thread_id = ? ORDER BY started_at DESC LIMIT 1", (thread,)).fetchone()
    db.close()
    if row:
        if row[0] == "inProgress": result = "running"
        elif since and timestamp(row[1]) >= since and row[0] in ("completed", "cancelled", "canceled"):
            result = "finished"
except Exception: pass
try:
    db = sqlite3.connect(f"file:{home / 'state_5.sqlite'}?mode=ro", uri=True)
    row = db.execute("SELECT archived, rollout_path FROM threads WHERE id = ?", (thread,)).fetchone()
    db.close()
    if row and row[0]: result = "finished"
    elif row and row[1] and since:
        with open(row[1], "rb") as f:
            f.seek(0, 2)
            f.seek(max(0, f.tell() - 262144))
            lines = f.read().decode("utf-8", errors="ignore").splitlines()
        for line in reversed(lines):
            try: event = json.loads(line)
            except Exception: continue
            if timestamp(event.get("timestamp")) < since: break
            payload = event.get("payload", {})
            if event.get("type") == "event_msg":
                kind = payload.get("type")
                if kind in ("task_complete", "task_completed", "turn_aborted"):
                    result = "finished"
                    break
                if kind == "task_started":
                    result = "running"
                    break
                if kind == "error":
                    result = "retry"
                    break
except Exception: pass
try:
    # The CLI fallback may not write thread_history while it is executing.
    processes = subprocess.run(["ps", "-axo", "command="], capture_output=True, text=True, timeout=5).stdout
    if result != "finished" and any(thread in line and "codex" in line and "exec" in line and "resume" in line for line in processes.splitlines()):
        result = "running"
except Exception: pass
print("__RECOVERY_PROGRESS__" + result)
"#;

async fn recovery_progress(retry: &RecoveryRetry) -> RecoveryProgress {
    let encoded = BASE64_STANDARD.encode(serde_json::json!({
        "thread_id": retry.task.thread_id,
        "since": retry.first_attempt_at,
    }).to_string());
    let output = if retry.task.source == "local" {
        let script = PYTHON_RECOVERY_PROGRESS.to_string();
        tokio::task::spawn_blocking(move || {
            let mut child = std::process::Command::new("python3")
                .args(["-", &encoded]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
                .spawn().map_err(|err| err.to_string())?;
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                stdin.write_all(script.as_bytes()).map_err(|err| err.to_string())?;
            }
            child.wait_with_output().map(|out| String::from_utf8_lossy(&out.stdout).to_string()).map_err(|err| err.to_string())
        }).await.unwrap_or_else(|err| Err(err.to_string()))
    } else {
        let server = ssh_server::list_servers().ok().and_then(|list| list.servers.into_iter()
            .find(|server| Some(server.id.as_str()) == retry.task.server_id.as_deref()));
        match server {
            Some(server) => ssh_server::run_ssh(&server, 25, &["python3", "-", &encoded], Some(ssh_server::remote_python_with_home(PYTHON_RECOVERY_PROGRESS, "CODEX_HOME", &server.codex_home))).await,
            None => Err("Recovery server unavailable".to_string()),
        }
    };
    match output {
        Ok(text) if text.contains("__RECOVERY_PROGRESS__running") => RecoveryProgress::Running,
        Ok(text) if text.contains("__RECOVERY_PROGRESS__finished") => RecoveryProgress::Finished,
        _ => RecoveryProgress::Retry,
    }
}

async fn resume_available_interrupted_tasks(app: &AppHandle) {
    // Serialize the scheduler and delayed switch callback across detection and launch.
    if RECOVERY_IN_PROGRESS.swap(true, Ordering::SeqCst) {
        return;
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) { RECOVERY_IN_PROGRESS.store(false, Ordering::SeqCst); }
    }
    let _guard = Guard;
    let tasks = detect_all_interrupted_tasks().await;
    let now_ts = Utc::now().timestamp();
    let pending = {
        let mut retries = retry_state().lock().unwrap_or_else(|e| e.into_inner());
        for task in tasks {
            if !task.is_excluded {
                retries.entry(task.id.clone()).or_insert(RecoveryRetry {
                    task, attempts: 0, next_attempt_at: 0, first_attempt_at: 0,
                });
            }
        }
        save_retry_state(&retries);
        retries.values().cloned().collect::<Vec<_>>()
    };

    for retry in pending {
        let task = retry.task.clone();
        let cfg = config::get_user_config();
        if !cfg.codex_auto_resume_interrupted_tasks_enabled { return; }
        let excluded = load_excluded_task_ids();
        let dismissed = load_dismissed_task_ids();
        if excluded.contains(&task.id) || excluded.contains(&task.thread_id)
            || dismissed.contains(&task.id) || dismissed.contains(&task.thread_id) {
            let mut retries = retry_state().lock().unwrap_or_else(|e| e.into_inner());
            retries.remove(&task.id);
            save_retry_state(&retries);
            continue;
        }
        if task.source != "local" && !cfg.codex_auto_resume_sync_ssh { continue; }
        if now_ts < retry.next_attempt_at { continue; }
        match recovery_progress(&retry).await {
            RecoveryProgress::Finished => {
                let mut retries = retry_state().lock().unwrap_or_else(|e| e.into_inner());
                retries.remove(&task.id);
                save_retry_state(&retries);
                continue;
            }
            RecoveryProgress::Running => continue,
            RecoveryProgress::Retry => {}
        }
        // Re-check immediately before each launch; detection's pool-wide quota is
        // insufficient when the active account has run out during this batch.
        if !get_current_codex_quota_resets_at().2 { return; }
        {
            let mut retries = retry_state().lock().unwrap_or_else(|e| e.into_inner());
            if let Some(state) = retries.get_mut(&task.id) {
                schedule_next_retry(state, Utc::now().timestamp());
            }
            save_retry_state(&retries);
        }

        logger::log_info(&format!(
            "[CodexAutoResume] 检测到配额恢复，正在自动恢复任务: id={}, title={}",
            task.id, task.title
        ));

        let res = resume_task(&task, None, "auto").await;
        // Measure the delay from the end of the attempt, including slow failures.
        {
            let mut retries = retry_state().lock().unwrap_or_else(|e| e.into_inner());
            if let Some(state) = retries.get_mut(&task.id) {
                state.next_attempt_at = Utc::now().timestamp() + retry_delay_seconds(state.attempts);
            }
            save_retry_state(&retries);
        }
        match res {
            Ok(result) => {
                logger::log_info(&format!(
                    "[CodexAutoResume] 自动恢复成功: {} - {}",
                    task.id, result.message
                ));

                // Send OS notification
                let title = "Codex 任务已自动恢复";
                let display_title = if task.title.len() > 50 {
                    format!("{}...", task.title.chars().take(47).collect::<String>())
                } else {
                    task.title.clone()
                };
                let body = format!("已自动恢复执行：{}", display_title);

                #[cfg(target_os = "macos")]
                {
                    let bundle_identifier =
                        app.config().identifier.clone();
                    let mut notification = mac_notification_sys::Notification::new();
                    notification.title(title).message(&body);
                    let _ = mac_notification_sys::set_application(&bundle_identifier);
                    let _ = notification.send();
                }

                #[cfg(not(target_os = "macos"))]
                {
                    use tauri_plugin_notification::NotificationExt;
                    let _ = app
                        .notification()
                        .builder()
                        .title(title)
                        .body(&body)
                        .show();
                }

                let _ = app.emit("codex://task-auto-resumed", &task);
            }
            Err(err) => {
                logger::log_warn(&format!(
                    "[CodexAutoResume] 自动恢复任务失败: {} - {}",
                    task.id, err
                ));
                // Keep the recovery record: the next scheduler pass retries when due.
            }
        }
    }
}

pub async fn resume_checked_tasks_on_account_switch(app: &AppHandle) {
    let cfg = config::get_user_config();
    if !cfg.codex_auto_resume_interrupted_tasks_enabled {
        logger::log_info("[CodexAutoResume] 切号后检查：自动恢复功能未开启，跳过");
        return;
    }

    let (_resets_at, _window, can_resume) = get_current_codex_quota_resets_at();
    if !can_resume {
        logger::log_info("[CodexAutoResume] 切号后检查：当前账号配额仍处于受限状态，跳过立即恢复");
        return;
    }

    logger::log_info("[CodexAutoResume] 切号后检测到配额可用，开始检查并立即恢复勾选的中断任务...");
    // Share the scheduler's serialized recovery loop and retry policy. The delayed switch
    // callback can overlap the scheduler that initiated the account switch.
    resume_available_interrupted_tasks(app).await;
}

static CONSUMED_EXPIRING_CREDITS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn consumed_expiring_credits() -> &'static Mutex<HashSet<String>> {
    CONSUMED_EXPIRING_CREDITS.get_or_init(|| Mutex::new(HashSet::new()))
}

pub async fn check_and_auto_apply_expiring_resets(app: &AppHandle) {
    let cfg = config::get_user_config();
    if !cfg.codex_auto_apply_reset_before_expiration_enabled {
        return;
    }

    let accounts = crate::modules::codex_account::list_accounts();
    let now_ts = Utc::now().timestamp();

    for account in accounts {
        if account.is_api_key_auth() {
            continue;
        }

        let quota = match &account.quota {
            Some(q) => q,
            None => continue,
        };

        let reset_credits = &quota.reset_credits;

        // Find earliest available reset credit expiry
        let mut earliest_expires_at: Option<i64> = None;
        let mut has_available = false;

        for credit in reset_credits {
            let status = credit.status.as_deref().unwrap_or("available").to_lowercase();
            if status == "redeemed" || status == "used" || status == "consumed" || status == "expired" {
                continue;
            }
            if let Some(exp) = credit.expires_at {
                if exp <= now_ts {
                    continue;
                }
                has_available = true;
                if earliest_expires_at.is_none() || Some(exp) < earliest_expires_at {
                    earliest_expires_at = Some(exp);
                }
            } else {
                has_available = true;
            }
        }

        if !has_available {
            continue;
        }

        if earliest_expires_at.is_none() {
            if let Some(next_exp) = quota.reset_credits_next_expires_at {
                if next_exp > now_ts {
                    earliest_expires_at = Some(next_exp);
                }
            }
        }

        let Some(expires_at) = earliest_expires_at else {
            continue;
        };

        // 10 minutes (600s) before expiration
        if now_ts < expires_at - 600 || now_ts >= expires_at {
            continue;
        }

        let credit_dedup_key = format!("{}:{}", account.id, expires_at);
        {
            let mut consumed = consumed_expiring_credits().lock().unwrap_or_else(|e| e.into_inner());
            if consumed.contains(&credit_dedup_key) {
                continue;
            }
            consumed.insert(credit_dedup_key.clone());
        }

        logger::log_info(&format!(
            "[CodexAutoReset] 账号 {} 的重置额度将在 10 分钟内到期 (expires_at={}, now={})，触发自动使用以防过期浪费",
            account.id, expires_at, now_ts
        ));

        let account_id = account.id.clone();
        let account_email = account.email.clone();
        let app_handle = app.clone();

        tauri::async_runtime::spawn(async move {
            let res = crate::modules::codex_quota::consume_reset_credit(&account_id).await;
            match res {
                Ok(_) => {
                    logger::log_info(&format!(
                        "[CodexAutoReset] 成功为账号 {} 自动使用即将到期的重置额度",
                        account_id
                    ));

                    let title = "Codex 重置额度已自动使用";
                    let body = format!(
                        "账号 {} 的重置额度即将在 10 分钟内到期，已自动使用以防过期浪费。",
                        account_email
                    );

                    #[cfg(target_os = "macos")]
                    {
                        let bundle_identifier = app_handle.config().identifier.clone();
                        let mut notification = mac_notification_sys::Notification::new();
                        notification.title(title).message(&body);
                        let _ = mac_notification_sys::set_application(&bundle_identifier);
                        let _ = notification.send();
                    }

                    #[cfg(not(target_os = "macos"))]
                    {
                        use tauri_plugin_notification::NotificationExt;
                        let _ = app_handle
                            .notification()
                            .builder()
                            .title(title)
                            .body(&body)
                            .show();
                    }

                    let _ = app_handle.emit(
                        "codex://reset-credit-auto-consumed",
                        serde_json::json!({
                            "accountId": account_id,
                            "accountEmail": account_email,
                            "reason": "expiring",
                            "expiresAt": expires_at,
                        }),
                    );

                    let _ = crate::modules::codex_quota::refresh_account_quota(&account_id).await;
                    let _ = crate::modules::tray::update_tray_menu(&app_handle);
                }
                Err(err) => {
                    logger::log_warn(&format!(
                        "[CodexAutoReset] 为账号 {} 自动使用重置额度失败: {}",
                        account_id, err
                    ));
                    let mut consumed = consumed_expiring_credits().lock().unwrap_or_else(|e| e.into_inner());
                    consumed.remove(&credit_dedup_key);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_resume_regression_macos_bundled_cli_and_model_preservation() {
        use std::io::Write;
        let definitions = PYTHON_RESUME_HELPER.split("\ntry:\n    raw_arg").next().unwrap();
        let test = r#"
import tempfile, shutil
with tempfile.TemporaryDirectory() as root:
    os.environ['CODEX_HOME'] = root
    resume_desktop_owner = lambda *args: None
    shutil.which = lambda name: None
    time.sleep = lambda seconds: None
    os.access = lambda *args: True
    captured = []
    class Child:
        returncode = 5
        def poll(self): return self.returncode
    subprocess.Popen = lambda cmd, **kwargs: (captured.append(cmd) or Child())
    for binary in ['/Applications/ChatGPT.app/Contents/Resources/codex',
                   '/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex']:
        os.path.isfile = lambda path: path == binary
        result = resume_thread('fixture', False, 'Continue', root, 'saved-model')
        assert not result['success'], result
        assert captured[-1][0] == binary
        assert captured[-1][-4:] == ['--model', 'saved-model', 'fixture', 'Continue']
"#;
        let mut child = std::process::Command::new("python3").arg("-")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(format!("{}\n{}", definitions, test).as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }

    #[test]
    fn auto_resume_regression_backoff_continues_without_a_retry_limit() {
        assert_eq!(retry_delay_seconds(1), 60);
        assert_eq!(retry_delay_seconds(3), 60);
        assert_eq!(retry_delay_seconds(4), 300);
        assert_eq!(retry_delay_seconds(6), 300);
        assert_eq!(retry_delay_seconds(7), 600);
        assert_eq!(retry_delay_seconds(u32::MAX), 600);
    }

    #[test]
    fn auto_resume_regression_retry_state_survives_restart() {
        let task: InterruptedCodexTask = serde_json::from_value(serde_json::json!({
            "thread_id": "thread", "title": "Task", "status": "usage_limited"
        })).unwrap();
        let mut retry = RecoveryRetry { task, attempts: 0, next_attempt_at: 0, first_attempt_at: 0 };
        schedule_next_retry(&mut retry, 2000);
        assert_eq!(retry.next_attempt_at, 2060);
        for now in [2060, 2120, 2180] { schedule_next_retry(&mut retry, now); }
        assert_eq!(retry.next_attempt_at, 2480);
        let json = serde_json::to_string(&retry).unwrap();
        let restored: RecoveryRetry = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.attempts, 4);
        assert_eq!(restored.first_attempt_at, 2000);
        assert_eq!(restored.next_attempt_at, 2480);
    }

    #[test]
    fn auto_resume_regression_desktop_owner_recovery_uses_existing_writer() {
        let definitions = PYTHON_RESUME_HELPER.split("def resume_thread(").next().unwrap();
        let test = r#"
import tempfile, threading
for accepted in (True, False):
    with tempfile.TemporaryDirectory() as root:
        home = Path(root)
        (home / 'ipc').mkdir()
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(str(home / 'ipc' / 'ipc.sock'))
        server.listen(1)
        requests = []
        def serve():
            peer, _ = server.accept()
            def exact(n):
                data = b''
                while len(data) < n:
                    chunk = peer.recv(n - len(data))
                    if not chunk: raise EOFError()
                    data += chunk
                return data
            for method in ('initialize', 'thread-owner-discovery', 'thread-follower-start-turn'):
                message = json.loads(exact(struct.unpack('<I', exact(4))[0]))
                requests.append(message)
                assert message['method'] == method
                result = {'clientId': 'client'} if method == 'initialize' else {}
                if method == 'thread-follower-start-turn':
                    assert message['version'] == 2
                    assert message['targetClientId'] == 'owner'
                    assert message['params']['turnStart']['request'] == {
                        'threadId': 'thread', 'input': [{'type': 'text', 'text': 'Continue'}]}
                    assert message['params']['turnStart']['context']['inheritThreadSettings']
                    result = {'result': {'turn': {'id': 'new-turn', 'status': 'inProgress'}}}
                reply = {'type': 'response', 'requestId': message['requestId'],
                    'resultType': 'success', 'handledByClientId': 'owner', 'result': result}
                if method == 'thread-follower-start-turn' and not accepted:
                    reply.update(resultType='error', error='service unavailable')
                data = json.dumps(reply).encode()
                # Deliberately fragment the frame header and body.
                frame = struct.pack('<I', len(data)) + data
                peer.sendall(frame[:2]); peer.sendall(frame[2:7]); peer.sendall(frame[7:])
            peer.close()
        worker = threading.Thread(target=serve, daemon=True)
        worker.start()
        result = resume_desktop_owner(home, 'thread', 'Continue')
        worker.join(timeout=5)
        server.close()
        assert not worker.is_alive()
        assert len(requests) == 3
        assert result['success'] == accepted, result
        assert result['mode'] == 'desktop-ipc'
        if not accepted: assert result['message'] == 'service unavailable'
"#;
        let mut child = std::process::Command::new("python3").arg("-")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(format!("{}\n{}", definitions, test).as_bytes()).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }

    #[tokio::test]
    async fn auto_resume_regression_observes_execution_instead_of_launch_ack() {
        let dir = std::env::temp_dir().join(format!("cockpit-recovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let thread_id = format!("recovery-test-{}", uuid::Uuid::new_v4());
        let conn = rusqlite::Connection::open(dir.join("thread_history_1.sqlite")).unwrap();
        conn.execute_batch("CREATE TABLE thread_turns (thread_id TEXT, status TEXT, started_at INTEGER);").unwrap();
        let now = Utc::now().timestamp();
        conn.execute("INSERT INTO thread_turns VALUES (?, 'completed', ?)", rusqlite::params![thread_id, (now - 100) * 1000]).unwrap();
        let encoded = BASE64_STANDARD.encode(serde_json::json!({"thread_id": thread_id, "since": now}).to_string());
        let state = rusqlite::Connection::open(dir.join("state_5.sqlite")).unwrap();
        state.execute_batch("CREATE TABLE threads (id TEXT, archived INTEGER, rollout_path TEXT);").unwrap();
        let rollout = dir.join("rollout.jsonl");
        state.execute("INSERT INTO threads VALUES (?, 0, ?)", rusqlite::params![thread_id, rollout.to_str().unwrap()]).unwrap();
        for (status, started_at, rollout_kind, expected) in [
            ("completed", (now - 100) * 1000, "", "retry"),
            ("inProgress", now * 1000, "", "running"),
            ("failed", now * 1000, "", "retry"),
            ("completed", now * 1000, "", "finished"),
            ("cancelled", now * 1000, "", "finished"),
            // New desktop versions no longer update the older history database.
            ("interrupted", (now - 100) * 1000, "task_started", "running"),
            ("inProgress", (now - 100) * 1000, "task_complete", "finished"),
            ("inProgress", (now - 100) * 1000, "error", "retry"),
        ] {
            let line = if rollout_kind.is_empty() { String::new() } else {
                serde_json::json!({"timestamp": Utc::now().to_rfc3339(),
                    "type": "event_msg", "payload": {"type": rollout_kind}}).to_string()
            };
            std::fs::write(&rollout, line).unwrap();
            conn.execute("UPDATE thread_turns SET status = ?, started_at = ?", rusqlite::params![status, started_at]).unwrap();
            let mut child = std::process::Command::new("python3")
                .args(["-", &encoded]).env("CODEX_HOME", &dir)
                .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
            {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(PYTHON_RECOVERY_PROGRESS.as_bytes()).unwrap();
            }
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), format!("__RECOVERY_PROGRESS__{}", expected));
        }
        drop(conn);
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn test_remote_detection_on_pve() {
        if let Ok(server_list) = ssh_server::list_servers() {
            if let Some(server) = server_list.servers.iter().find(|s| s.host.contains("pve-ubuntu")) {
                println!("Testing server: {} ({})", server.name, server.host);
                let tasks = detect_remote_tasks(server).await;
                println!("Detected tasks: {:?}", tasks);
                assert!(tasks.is_ok());
                let tasks = tasks.unwrap();
                println!("Found tasks count: {}", tasks.len());
            }
        }
    }

    #[test]
    fn test_task_exclusion_logic() {
        let test_task_id = "test:exclude:123";
        let _ = set_task_excluded(test_task_id, false);
        assert!(!load_excluded_task_ids().contains(test_task_id));
        set_task_excluded(test_task_id, true).unwrap();
        assert!(load_excluded_task_ids().contains(test_task_id));
        set_task_excluded(test_task_id, false).unwrap();
        assert!(!load_excluded_task_ids().contains(test_task_id));
    }

    #[test]
    fn test_task_delete_dismiss_logic() {
        let test_task_id = "test:delete:999";
        let _ = delete_interrupted_task(test_task_id);
        assert!(load_dismissed_task_ids().contains(test_task_id));
    }

    #[test]
    fn test_python_resume_helper_base64_payload() {
        use std::io::Write;
        let test_payload = serde_json::json!({
            "thread_id": "test-uuid-non-existent",
            "is_goal": false,
            "prompt": "Continue working /goal test 'quotes' and {braces}, commas.",
            "cwd": "/tmp",
            "model": "gpt-6-astra"
        });
        let b64 = BASE64_STANDARD.encode(test_payload.to_string().as_bytes());

        let mut child = std::process::Command::new("python3")
            .arg("-")
            .arg(&b64)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn python3");

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(PYTHON_RESUME_HELPER.as_bytes());
        }

        let output = child.wait_with_output().expect("Failed to read python output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("__CODEX_RESUME_RESULT_JSON_START__"));
        assert!(stdout.contains("__CODEX_RESUME_RESULT_JSON_END__"));
        assert!(!stdout.contains("Expecting value: line 1 column 1"));
    }

    #[test]
    fn test_earlier_reset_candidate_sorting() {
        let make_account = |id: &str, email: &str| -> CodexAccount {
            serde_json::from_value(serde_json::json!({
                "id": id,
                "email": email,
                "user_id": null,
                "plan_type": "plus",
                "account_id": null,
                "organization_id": null,
                "tags": null,
                "created_at": 100,
                "last_used": 100,
                "tokens": {
                    "access_token": "token",
                    "refresh_token": "refresh",
                    "id_token": "id_token"
                }
            }))
            .unwrap()
        };

        let mut candidates = vec![
            EarlierResetCandidate {
                account: make_account("acc_late", "late@example.com"),
                has_positive_quota: false,
                hourly_reset_time: 18000,
                hourly_percentage: 0,
                last_used: 100,
            },
            EarlierResetCandidate {
                account: make_account("acc_early", "early@example.com"),
                has_positive_quota: false,
                hourly_reset_time: 17000,
                hourly_percentage: 0,
                last_used: 200,
            },
            EarlierResetCandidate {
                account: make_account("acc_positive", "positive@example.com"),
                has_positive_quota: true,
                hourly_reset_time: 0,
                hourly_percentage: 80,
                last_used: 300,
            },
        ];

        candidates.sort_by(|a, b| {
            b.has_positive_quota
                .cmp(&a.has_positive_quota)
                .then_with(|| a.hourly_reset_time.cmp(&b.hourly_reset_time))
                .then_with(|| b.hourly_percentage.cmp(&a.hourly_percentage))
                .then_with(|| a.last_used.cmp(&b.last_used))
        });

        assert_eq!(candidates[0].account.id, "acc_positive");
        assert_eq!(candidates[1].account.id, "acc_early");
        assert_eq!(candidates[2].account.id, "acc_late");
    }

    #[test]
    fn test_compute_earliest_quota_reset_from_accounts() {
        let make_account_with_quota = |id: &str, email: &str, hourly_pct: i32, hourly_reset: Option<i64>, weekly_pct: i32, weekly_reset: Option<i64>| -> CodexAccount {
            serde_json::from_value(serde_json::json!({
                "id": id,
                "email": email,
                "user_id": null,
                "plan_type": "plus",
                "account_id": null,
                "organization_id": null,
                "tags": null,
                "created_at": 100,
                "last_used": 100,
                "tokens": {
                    "access_token": "token",
                    "refresh_token": "refresh",
                    "id_token": "id_token"
                },
                "quota": {
                    "hourly_percentage": hourly_pct,
                    "hourly_reset_time": hourly_reset,
                    "hourly_window_minutes": 300,
                    "weekly_percentage": weekly_pct,
                    "weekly_reset_time": weekly_reset
                }
            }))
            .unwrap()
        };

        let now_ts = 10000;
        let acc_current = make_account_with_quota("acc_curr", "curr@example.com", 0, Some(20000), 100, Some(50000));
        let acc_earlier = make_account_with_quota("acc_early", "early@example.com", 0, Some(14000), 100, Some(50000));
        let acc_later = make_account_with_quota("acc_late", "late@example.com", 0, Some(25000), 100, Some(50000));

        let accounts = vec![acc_current.clone(), acc_earlier.clone(), acc_later.clone()];

        // Case 1: All accounts exhausted -> pick acc_earlier with earliest 5h reset
        let (resets_at, window, can_resume, email) = compute_earliest_quota_reset_from_accounts(
            now_ts,
            Some(&acc_current),
            &accounts,
            None,
        );
        assert_eq!(resets_at, Some(14000));
        assert_eq!(window, Some(300));
        assert_eq!(can_resume, false);
        assert_eq!(email, Some("early@example.com".to_string()));

        // Case 2: acc_earlier is weekly blocked -> skip it, choose acc_current (20000 < 25000)
        let acc_earlier_blocked = make_account_with_quota("acc_early", "early@example.com", 0, Some(14000), 0, Some(60000));
        let accounts2 = vec![acc_current.clone(), acc_earlier_blocked, acc_later.clone()];
        let (resets_at2, _, can_resume2, email2) = compute_earliest_quota_reset_from_accounts(
            now_ts,
            Some(&acc_current),
            &accounts2,
            None,
        );
        assert_eq!(resets_at2, Some(20000));
        assert_eq!(can_resume2, false);
        assert_eq!(email2, Some("curr@example.com".to_string()));

        // Case 3: One account has positive quota -> can_resume = true
        let acc_ready = make_account_with_quota("acc_ready", "ready@example.com", 50, Some(18000), 100, Some(60000));
        let accounts3 = vec![acc_current.clone(), acc_ready.clone()];
        let (_, _, can_resume3, email3) = compute_earliest_quota_reset_from_accounts(
            now_ts,
            Some(&acc_current),
            &accounts3,
            None,
        );
        assert_eq!(can_resume3, true);
        assert_eq!(email3, Some("ready@example.com".to_string()));

        // Case 4: One account reached reset time (now_ts >= 14000) -> can_resume = true
        let (resets_at4, _, can_resume4, email4) = compute_earliest_quota_reset_from_accounts(
            15000,
            Some(&acc_current),
            &accounts,
            None,
        );
        assert_eq!(can_resume4, true);
        assert_eq!(resets_at4, Some(14000));
        assert_eq!(email4, Some("early@example.com".to_string()));
    }
}


