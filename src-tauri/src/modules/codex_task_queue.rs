use crate::modules::{codex_auto_resume, logger, ssh_server};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::Utc;
use rusqlite::OpenFlags;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter};

const TASK_QUEUE_FILE: &str = "codex_task_queue.json";
const TASK_QUEUE_CONFIG_FILE: &str = "codex_task_queue_config.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    WaitingInterrupted,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::WaitingInterrupted => "waiting_interrupted",
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexQueuedTask {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub source: String, // "local" | "ssh"
    #[serde(default)]
    pub server_id: Option<String>,
    #[serde(default)]
    pub server_name: Option<String>,
    pub cwd: String,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub thread_title: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub status: TaskStatus,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub output_preview: Option<String>,
    #[serde(default)]
    pub assigned_thread_id: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    pub created_at: i64,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexQueueConfig {
    pub concurrency_mode: String, // "sequential" | "concurrent"
    pub max_concurrent_tasks: usize,
    pub auto_dispatch: bool,
}

impl Default for CodexQueueConfig {
    fn default() -> Self {
        Self {
            concurrency_mode: "sequential".to_string(),
            max_concurrent_tasks: 2,
            auto_dispatch: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexProjectItem {
    pub cwd: String,
    pub name: String,
    pub last_active: i64,
    pub thread_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexThreadSummary {
    pub id: String,
    pub title: String,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub updated_at: i64,
    pub has_interrupted_task: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexQueueState {
    pub tasks: Vec<CodexQueuedTask>,
    pub config: CodexQueueConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateQueuedTaskRequest {
    pub title: Option<String>,
    pub prompt: String,
    pub source: String, // "local" | "ssh"
    pub server_id: Option<String>,
    pub server_name: Option<String>,
    pub cwd: String,
    pub thread_id: Option<String>,
    pub thread_title: Option<String>,
    pub model: Option<String>,
}

static QUEUE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static DISPATCH_RUNNING: OnceLock<Mutex<bool>> = OnceLock::new();

fn queue_lock() -> &'static Mutex<()> {
    QUEUE_LOCK.get_or_init(|| Mutex::new(()))
}

fn dispatch_running_flag() -> &'static Mutex<bool> {
    DISPATCH_RUNNING.get_or_init(|| Mutex::new(false))
}

fn queue_file_path() -> Result<PathBuf, String> {
    Ok(crate::modules::account::get_data_dir()?.join(TASK_QUEUE_FILE))
}

fn config_file_path() -> Result<PathBuf, String> {
    Ok(crate::modules::account::get_data_dir()?.join(TASK_QUEUE_CONFIG_FILE))
}

pub fn load_queue_config() -> CodexQueueConfig {
    let path = match config_file_path() {
        Ok(p) => p,
        Err(_) => return CodexQueueConfig::default(),
    };
    if !path.exists() {
        return CodexQueueConfig::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => CodexQueueConfig::default(),
    }
}

pub fn save_queue_config(config: &CodexQueueConfig) -> Result<(), String> {
    let path = config_file_path()?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("序列化任务队列配置失败: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("写入任务队列配置失败: {}", e))?;
    Ok(())
}

pub fn load_tasks() -> Vec<CodexQueuedTask> {
    let path = match queue_file_path() {
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

pub fn save_tasks(tasks: &[CodexQueuedTask]) -> Result<(), String> {
    let path = queue_file_path()?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = serde_json::to_string_pretty(tasks)
        .map_err(|e| format!("序列化任务队列失败: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("写入任务队列失败: {}", e))?;
    Ok(())
}

pub fn get_queue_state() -> Result<CodexQueueState, String> {
    let _guard = queue_lock().lock().unwrap();
    let tasks = load_tasks();
    let config = load_queue_config();
    Ok(CodexQueueState { tasks, config })
}

pub fn add_task(req: CreateQueuedTaskRequest) -> Result<CodexQueuedTask, String> {
    let _guard = queue_lock().lock().unwrap();
    let mut tasks = load_tasks();
    let now = Utc::now().timestamp();
    let id = uuid::Uuid::new_v4().to_string();

    let title = req.title.unwrap_or_else(|| {
        let trimmed = req.prompt.trim();
        if trimmed.chars().count() > 40 {
            format!("{}...", trimmed.chars().take(40).collect::<String>())
        } else if !trimmed.is_empty() {
            trimmed.to_string()
        } else {
            "未命名任务".to_string()
        }
    });

    let task = CodexQueuedTask {
        id,
        title,
        prompt: req.prompt,
        source: req.source,
        server_id: req.server_id,
        server_name: req.server_name,
        cwd: req.cwd,
        thread_id: req.thread_id,
        thread_title: req.thread_title,
        model: req.model,
        status: TaskStatus::Pending,
        error_message: None,
        output_preview: None,
        assigned_thread_id: None,
        pid: None,
        created_at: now,
        started_at: None,
        completed_at: None,
    };

    tasks.push(task.clone());
    save_tasks(&tasks)?;
    logger::log_info(&format!(
        "[CodexTaskQueue] 添加排队任务: id={}, title={}, source={}",
        task.id, task.title, task.source
    ));
    Ok(task)
}

pub fn update_task(task: CodexQueuedTask) -> Result<CodexQueuedTask, String> {
    let _guard = queue_lock().lock().unwrap();
    let mut tasks = load_tasks();
    if let Some(idx) = tasks.iter().position(|t| t.id == task.id) {
        tasks[idx] = task.clone();
        save_tasks(&tasks)?;
        Ok(task)
    } else {
        Err(format!("未找到任务: {}", task.id))
    }
}

pub fn delete_task(task_id: &str) -> Result<(), String> {
    let _guard = queue_lock().lock().unwrap();
    let mut tasks = load_tasks();
    let prev_len = tasks.len();
    tasks.retain(|t| t.id != task_id);
    if tasks.len() == prev_len {
        return Err(format!("未找到任务: {}", task_id));
    }
    save_tasks(&tasks)?;
    logger::log_info(&format!("[CodexTaskQueue] 删除排队任务: id={}", task_id));
    Ok(())
}

pub fn reorder_queue(task_ids: Vec<String>) -> Result<(), String> {
    let _guard = queue_lock().lock().unwrap();
    let tasks = load_tasks();
    let mut map: std::collections::HashMap<String, CodexQueuedTask> =
        tasks.into_iter().map(|t| (t.id.clone(), t)).collect();

    let mut reordered = Vec::new();
    for id in &task_ids {
        if let Some(t) = map.remove(id) {
            reordered.push(t);
        }
    }
    // Append any unmentioned tasks at the end
    for (_, t) in map {
        reordered.push(t);
    }
    save_tasks(&reordered)?;
    Ok(())
}

pub fn retry_task(task_id: &str) -> Result<(), String> {
    let _guard = queue_lock().lock().unwrap();
    let mut tasks = load_tasks();
    if let Some(task) = tasks.iter_mut().find(|t| t.id == task_id) {
        task.status = TaskStatus::Pending;
        task.error_message = None;
        task.output_preview = None;
        task.started_at = None;
        task.completed_at = None;
        task.pid = None;
        save_tasks(&tasks)?;
        logger::log_info(&format!("[CodexTaskQueue] 重试任务: id={}", task_id));
        Ok(())
    } else {
        Err(format!("未找到任务: {}", task_id))
    }
}

pub fn clear_completed_tasks() -> Result<(), String> {
    let _guard = queue_lock().lock().unwrap();
    let mut tasks = load_tasks();
    tasks.retain(|t| t.status != TaskStatus::Completed && t.status != TaskStatus::Cancelled);
    save_tasks(&tasks)?;
    logger::log_info("[CodexTaskQueue] 清空已完成/已取消的任务");
    Ok(())
}

pub fn update_config(config: CodexQueueConfig) -> Result<(), String> {
    save_queue_config(&config)?;
    logger::log_info(&format!(
        "[CodexTaskQueue] 更新配置: mode={}, max_concurrent={}, auto_dispatch={}",
        config.concurrency_mode, config.max_concurrent_tasks, config.auto_dispatch
    ));
    Ok(())
}

// ----------------- Discovery: Projects & Threads -----------------

pub async fn list_projects(
    source: &str,
    server_id: Option<&str>,
) -> Result<Vec<CodexProjectItem>, String> {
    if source == "local" {
        list_local_projects()
    } else if source == "ssh" {
        let server_id = server_id.ok_or_else(|| "缺少 SSH 服务器 ID".to_string())?;
        list_ssh_projects(server_id).await
    } else {
        Err(format!("未知来源: {}", source))
    }
}

pub async fn list_threads(
    source: &str,
    server_id: Option<&str>,
    cwd: Option<&str>,
) -> Result<Vec<CodexThreadSummary>, String> {
    if source == "local" {
        list_local_threads(cwd).await
    } else if source == "ssh" {
        let server_id = server_id.ok_or_else(|| "缺少 SSH 服务器 ID".to_string())?;
        list_ssh_threads(server_id, cwd).await
    } else {
        Err(format!("未知来源: {}", source))
    }
}

fn list_local_projects() -> Result<Vec<CodexProjectItem>, String> {
    let codex_home = match std::env::var_os("CODEX_HOME") {
        Some(val) => PathBuf::from(val),
        None => match dirs::home_dir() {
            Some(home) => home.join(".codex"),
            None => return Ok(Vec::new()),
        },
    };
    let state_db = codex_home.join("state_5.sqlite");
    if !state_db.exists() {
        return Ok(Vec::new());
    }

    let conn = rusqlite::Connection::open_with_flags(
        &state_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("打开 local state_5.sqlite 失败: {}", e))?;

    let mut stmt = conn
        .prepare(
            "SELECT cwd, MAX(updated_at) as last_active, COUNT(id) as cnt \
             FROM threads \
             WHERE cwd IS NOT NULL AND cwd != '' AND archived = 0 \
             GROUP BY cwd \
             ORDER BY last_active DESC \
             LIMIT 50",
        )
        .map_err(|e| format!("准备查询 SQL 失败: {}", e))?;

    let rows = stmt
        .query_map([], |row| {
            let cwd: String = row.get(0)?;
            let last_active: i64 = row.get(1)?;
            let thread_count: usize = row.get(2)?;
            let name = Path::new(&cwd)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| cwd.clone());
            Ok(CodexProjectItem {
                cwd,
                name,
                last_active,
                thread_count,
            })
        })
        .map_err(|e| format!("执行项目查询失败: {}", e))?;

    let mut projects = Vec::new();
    for r in rows.flatten() {
        projects.push(r);
    }
    Ok(projects)
}

async fn list_local_threads(cwd: Option<&str>) -> Result<Vec<CodexThreadSummary>, String> {
    let codex_home = match std::env::var_os("CODEX_HOME") {
        Some(val) => PathBuf::from(val),
        None => match dirs::home_dir() {
            Some(home) => home.join(".codex"),
            None => return Ok(Vec::new()),
        },
    };
    let state_db = codex_home.join("state_5.sqlite");
    if !state_db.exists() {
        return Ok(Vec::new());
    }

    let interrupted = codex_auto_resume::detect_local_tasks();
    let interrupted_threads: HashSet<String> = interrupted.into_iter().map(|t| t.thread_id).collect();

    let conn = rusqlite::Connection::open_with_flags(
        &state_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("打开 local state_5.sqlite 失败: {}", e))?;

    let sql = if cwd.is_some() {
        "SELECT id, title, model, cwd, updated_at \
         FROM threads \
         WHERE archived = 0 AND cwd = ?1 \
         ORDER BY updated_at DESC \
         LIMIT 50"
    } else {
        "SELECT id, title, model, cwd, updated_at \
         FROM threads \
         WHERE archived = 0 \
         ORDER BY updated_at DESC \
         LIMIT 50"
    };

    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("准备查询会话 SQL 失败: {}", e))?;

    let mut threads = Vec::new();
    if let Some(target_cwd) = cwd {
        let rows = stmt
            .query_map([target_cwd], |row| {
                let id: String = row.get(0)?;
                let title: String = row.get(1)?;
                let model: Option<String> = row.get(2)?;
                let cwd: Option<String> = row.get(3)?;
                let updated_at: i64 = row.get(4)?;
                Ok((id, title, model, cwd, updated_at))
            })
            .map_err(|e| format!("查询会话列表失败: {}", e))?;

        for r in rows.flatten() {
            let has_interrupted = interrupted_threads.contains(&r.0);
            threads.push(CodexThreadSummary {
                id: r.0,
                title: if r.1.is_empty() { "未命名会话".to_string() } else { r.1 },
                model: r.2,
                cwd: r.3,
                updated_at: r.4,
                has_interrupted_task: has_interrupted,
            });
        }
    } else {
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let title: String = row.get(1)?;
                let model: Option<String> = row.get(2)?;
                let cwd: Option<String> = row.get(3)?;
                let updated_at: i64 = row.get(4)?;
                Ok((id, title, model, cwd, updated_at))
            })
            .map_err(|e| format!("查询会话列表失败: {}", e))?;

        for r in rows.flatten() {
            let has_interrupted = interrupted_threads.contains(&r.0);
            threads.push(CodexThreadSummary {
                id: r.0,
                title: if r.1.is_empty() { "未命名会话".to_string() } else { r.1 },
                model: r.2,
                cwd: r.3,
                updated_at: r.4,
                has_interrupted_task: has_interrupted,
            });
        }
    }

    Ok(threads)
}

const PYTHON_SSH_DISCOVERY: &str = r#"
import sys, json, os, sqlite3
from pathlib import Path

def handle():
    raw = sys.argv[1] if len(sys.argv) > 1 else "{}"
    params = json.loads(raw)
    action = params.get("action", "list_projects")
    
    codex_home_env = os.environ.get("CODEX_HOME")
    codex_home = Path(codex_home_env) if codex_home_env else (Path.home() / ".codex")
    state_db = codex_home / "state_5.sqlite"
    
    if not state_db.exists():
        return {"projects": [], "threads": []}
        
    conn = sqlite3.connect(f"file:{state_db}?mode=ro", uri=True)
    cur = conn.cursor()
    
    if action == "list_projects":
        cur.execute("""
            SELECT cwd, MAX(updated_at) as last_active, COUNT(id) as cnt
            FROM threads
            WHERE cwd IS NOT NULL AND cwd != '' AND archived = 0
            GROUP BY cwd
            ORDER BY last_active DESC
            LIMIT 50
        """)
        projects = []
        for row in cur.fetchall():
            cwd, last_active, cnt = row
            name = Path(cwd).name or cwd
            projects.append({
                "cwd": cwd,
                "name": name,
                "last_active": last_active,
                "thread_count": cnt
            })
        conn.close()
        return {"projects": projects}
        
    elif action == "list_threads":
        cwd = params.get("cwd")
        if cwd:
            cur.execute("""
                SELECT id, title, model, cwd, updated_at
                FROM threads
                WHERE archived = 0 AND cwd = ?
                ORDER BY updated_at DESC
                LIMIT 50
            """, (cwd,))
        else:
            cur.execute("""
                SELECT id, title, model, cwd, updated_at
                FROM threads
                WHERE archived = 0
                ORDER BY updated_at DESC
                LIMIT 50
            """)
        threads = []
        for row in cur.fetchall():
            t_id, title, model, t_cwd, updated_at = row
            threads.append({
                "id": t_id,
                "title": title or "未命名会话",
                "model": model,
                "cwd": t_cwd,
                "updated_at": updated_at or 0,
                "has_interrupted_task": False
            })
        conn.close()
        return {"threads": threads}
        
    conn.close()
    return {}

try:
    res = handle()
except Exception as e:
    res = {"error": str(e)}

print("__CODEX_QUEUE_JSON_START__")
print(json.dumps(res, ensure_ascii=False))
print("__CODEX_QUEUE_JSON_END__")
"#;

async fn list_ssh_projects(server_id: &str) -> Result<Vec<CodexProjectItem>, String> {
    let server_list = ssh_server::list_servers()?;
    let server = server_list
        .servers
        .into_iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| format!("未找到目标 SSH 服务器: {}", server_id))?;

    let payload = serde_json::json!({ "action": "list_projects" }).to_string();
    let output = ssh_server::run_ssh(
        &server,
        15,
        &["python3", "-", &payload],
        Some(PYTHON_SSH_DISCOVERY.to_string()),
    )
    .await?;

    let start_tag = "__CODEX_QUEUE_JSON_START__";
    let end_tag = "__CODEX_QUEUE_JSON_END__";
    let (s_idx, e_idx) = (
        output.find(start_tag).ok_or("未找到远端输出起始标记")? + start_tag.len(),
        output.find(end_tag).ok_or("未找到远端输出结束标记")?,
    );
    let json_str = output[s_idx..e_idx].trim();
    let val: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("解析远端项目 JSON 失败: {}", e))?;

    if let Some(err) = val.get("error").and_then(|v| v.as_str()) {
        return Err(format!("远端查询失败: {}", err));
    }

    let projects: Vec<CodexProjectItem> = val
        .get("projects")
        .cloned()
        .map(|v| serde_json::from_value(v).unwrap_or_default())
        .unwrap_or_default();

    Ok(projects)
}

async fn list_ssh_threads(
    server_id: &str,
    cwd: Option<&str>,
) -> Result<Vec<CodexThreadSummary>, String> {
    let server_list = ssh_server::list_servers()?;
    let server = server_list
        .servers
        .into_iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| format!("未找到目标 SSH 服务器: {}", server_id))?;

    let payload = serde_json::json!({
        "action": "list_threads",
        "cwd": cwd
    })
    .to_string();

    let output = ssh_server::run_ssh(
        &server,
        15,
        &["python3", "-", &payload],
        Some(PYTHON_SSH_DISCOVERY.to_string()),
    )
    .await?;

    let start_tag = "__CODEX_QUEUE_JSON_START__";
    let end_tag = "__CODEX_QUEUE_JSON_END__";
    let (s_idx, e_idx) = (
        output.find(start_tag).ok_or("未找到远端输出起始标记")? + start_tag.len(),
        output.find(end_tag).ok_or("未找到远端输出结束标记")?,
    );
    let json_str = output[s_idx..e_idx].trim();
    let val: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("解析远端会话 JSON 失败: {}", e))?;

    if let Some(err) = val.get("error").and_then(|v| v.as_str()) {
        return Err(format!("远端查询失败: {}", err));
    }

    let mut threads: Vec<CodexThreadSummary> = val
        .get("threads")
        .cloned()
        .map(|v| serde_json::from_value(v).unwrap_or_default())
        .unwrap_or_default();

    // Cross-reference with interrupted tasks on this server
    if let Ok(interrupted) = codex_auto_resume::detect_remote_tasks(&server).await {
        let interrupted_set: HashSet<String> = interrupted.into_iter().map(|t| t.thread_id).collect();
        for t in &mut threads {
            if interrupted_set.contains(&t.id) {
                t.has_interrupted_task = true;
            }
        }
    }

    Ok(threads)
}

// ----------------- Execution & Dispatcher Engine -----------------

const PYTHON_QUEUE_EXECUTE: &str = r#"
import socket, json, os, time, sqlite3, base64, sys, subprocess
from pathlib import Path

def execute():
    raw_arg = sys.argv[1] if len(sys.argv) > 1 else "{}"
    try:
        raw_arg = base64.b64decode(raw_arg).decode("utf-8")
    except Exception:
        pass
    data = json.loads(raw_arg)
    
    thread_id = data.get("thread_id")
    prompt = data.get("prompt", "")
    cwd = data.get("cwd")
    model = data.get("model")
    task_id = data.get("task_id", "default")
    
    codex_home_env = os.environ.get("CODEX_HOME")
    codex_home = Path(codex_home_env) if codex_home_env else (Path.home() / ".codex")
    sock_path = codex_home / "app-server-control" / "app-server-control.sock"
    target_cwd = cwd if (cwd and os.path.isdir(cwd)) else str(Path.home())
    
    # 1. Try App Server WS if socket exists
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
                data_b = msg.encode('utf-8')
                header = bytearray([0x81])
                length = len(data_b)
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
                masked = bytearray(data_b[i] ^ mask_key[i % 4] for i in range(length))
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
                rem = length
                while rem > 0:
                    chunk = s.recv(min(65536, rem))
                    if not chunk: break
                    buf.extend(chunk)
                    rem -= len(chunk)
                if masked:
                    for i in range(len(buf)):
                        buf[i] ^= mask_key[i % 4]
                return buf.decode('utf-8', errors='ignore')

            # Initialize
            send_ws(json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"cockpit-tools-queue","version":"1.0"}}}))
            while True:
                msg = json.loads(read_ws_msg())
                if msg.get("id") == 1:
                    break

            active_thread_id = thread_id
            if not active_thread_id:
                # Start new thread
                start_params = {"cwd": target_cwd}
                if model: start_params["model"] = model
                send_ws(json.dumps({"jsonrpc":"2.0","id":2,"method":"thread/start","params":start_params}))
                while True:
                    msg = json.loads(read_ws_msg())
                    if msg.get("id") == 2:
                        res_val = msg.get("result", {})
                        t_obj = res_val.get("thread", {})
                        active_thread_id = t_obj.get("id")
                        break
            else:
                # Resume existing thread
                send_ws(json.dumps({"jsonrpc":"2.0","id":2,"method":"thread/resume","params":{"threadId":active_thread_id}}))
                while True:
                    msg = json.loads(read_ws_msg())
                    if msg.get("id") == 2:
                        break

            if not active_thread_id:
                s.close()
                return {"success": False, "message": "无法创建或获取会话 ID"}

            # Start turn
            turn_params = {
                "threadId": active_thread_id,
                "input": [{"type": "text", "text": prompt}]
            }
            if cwd: turn_params["cwd"] = cwd
            if model: turn_params["model"] = model
            send_ws(json.dumps({"jsonrpc":"2.0","id":3,"method":"turn/start","params":turn_params}))
            while True:
                msg = json.loads(read_ws_msg())
                if msg.get("id") == 3:
                    break
            s.close()
            return {
                "success": True,
                "mode": "app-server",
                "thread_id": active_thread_id,
                "message": f"已通过 Codex 守护进程成功提交任务"
            }
        except Exception as e:
            if s:
                try: s.close()
                except: pass

    # 2. Fallback to CLI
    codex_bin = subprocess.getoutput("which codex").strip() or str(Path.home() / ".local/bin/codex")
    log_file = codex_home / f"task_queue_{task_id}.log"
    
    if thread_id:
        cmd = [codex_bin, "exec", "resume", "--skip-git-repo-check", thread_id, prompt]
    else:
        cmd = [codex_bin, "exec", "--skip-git-repo-check", "-C", target_cwd]
        if model:
            cmd.extend(["-c", f'model="{model}"'])
        cmd.append(prompt)
        
    with open(log_file, "a") as f:
        proc = subprocess.Popen(cmd, cwd=target_cwd, stdout=f, stderr=f, stdin=subprocess.DEVNULL, start_new_session=True)
        
    return {
        "success": True,
        "mode": "cli",
        "pid": proc.pid,
        "thread_id": thread_id,
        "message": f"已通过 Codex CLI 后台启动任务 (PID: {proc.pid})"
    }

try:
    res = execute()
except Exception as e:
    res = {"success": False, "message": str(e)}

print("__CODEX_QUEUE_EXEC_JSON_START__")
print(json.dumps(res, ensure_ascii=False))
print("__CODEX_QUEUE_EXEC_JSON_END__")
"#;

fn is_local_thread_busy(thread_id: &str) -> bool {
    let codex_home = match std::env::var_os("CODEX_HOME") {
        Some(val) => PathBuf::from(val),
        None => match dirs::home_dir() {
            Some(home) => home.join(".codex"),
            None => return false,
        },
    };
    let hist_db = codex_home.join("thread_history_1.sqlite");
    if !hist_db.exists() {
        return false;
    }
    let conn = rusqlite::Connection::open_with_flags(
        &hist_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    );
    if let Ok(conn) = conn {
        if let Ok(mut stmt) = conn.prepare(
            "SELECT status FROM thread_turns WHERE thread_id = ? ORDER BY started_at DESC LIMIT 1",
        ) {
            if let Ok(status) = stmt.query_row([thread_id], |row| row.get::<_, String>(0)) {
                return status == "inProgress";
            }
        }
    }
    false
}

fn check_local_turn_result(thread_id: &str) -> Option<(String, Option<String>)> {
    let codex_home = match std::env::var_os("CODEX_HOME") {
        Some(val) => PathBuf::from(val),
        None => match dirs::home_dir() {
            Some(home) => home.join(".codex"),
            None => return None,
        },
    };
    let hist_db = codex_home.join("thread_history_1.sqlite");
    if !hist_db.exists() {
        return None;
    }
    let conn = rusqlite::Connection::open_with_flags(
        &hist_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    );
    if let Ok(conn) = conn {
        if let Ok(mut stmt) = conn.prepare(
            "SELECT status, error_json FROM thread_turns WHERE thread_id = ? ORDER BY started_at DESC LIMIT 1",
        ) {
            if let Ok(res) = stmt.query_row([thread_id], |row| {
                let status: String = row.get(0)?;
                let err: Option<String> = row.get(1)?;
                Ok((status, err))
            }) {
                return Some(res);
            }
        }
    }
    None
}

fn find_latest_created_thread(cwd: &str) -> Option<String> {
    let codex_home = match std::env::var_os("CODEX_HOME") {
        Some(val) => PathBuf::from(val),
        None => match dirs::home_dir() {
            Some(home) => home.join(".codex"),
            None => return None,
        },
    };
    let state_db = codex_home.join("state_5.sqlite");
    if !state_db.exists() {
        return None;
    }
    let conn = rusqlite::Connection::open_with_flags(
        &state_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    );
    if let Ok(conn) = conn {
        if let Ok(mut stmt) = conn.prepare(
            "SELECT id FROM threads WHERE cwd = ? ORDER BY created_at DESC LIMIT 1",
        ) {
            if let Ok(id) = stmt.query_row([cwd], |row| row.get::<_, String>(0)) {
                return Some(id);
            }
        }
    }
    None
}

async fn launch_task(task: &mut CodexQueuedTask) -> Result<(), String> {
    let payload = serde_json::json!({
        "thread_id": task.thread_id,
        "prompt": task.prompt,
        "cwd": task.cwd,
        "model": task.model,
        "task_id": task.id
    })
    .to_string();
    let b64_payload = BASE64_STANDARD.encode(payload.as_bytes());

    if task.source == "local" {
        let child = std::process::Command::new("python3")
            .arg("-")
            .arg(&b64_payload)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        if let Ok(mut c) = child {
            if let Some(mut stdin) = c.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(PYTHON_QUEUE_EXECUTE.as_bytes());
            }
            if let Ok(output) = c.wait_with_output() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let start_tag = "__CODEX_QUEUE_EXEC_JSON_START__";
                let end_tag = "__CODEX_QUEUE_EXEC_JSON_END__";
                if let (Some(s_idx), Some(e_idx)) = (stdout.find(start_tag), stdout.find(end_tag)) {
                    let json_str = stdout[s_idx + start_tag.len()..e_idx].trim();
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                        if val.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                            if let Some(tid) = val.get("thread_id").and_then(|v| v.as_str()) {
                                if !tid.is_empty() {
                                    task.assigned_thread_id = Some(tid.to_string());
                                }
                            }
                            if let Some(pid) = val.get("pid").and_then(|v| v.as_u64()) {
                                task.pid = Some(pid as u32);
                            }
                            task.status = TaskStatus::Running;
                            task.started_at = Some(Utc::now().timestamp());
                            logger::log_info(&format!(
                                "[CodexTaskQueue] 成功启动本地排队任务: id={}, thread_id={:?}",
                                task.id, task.assigned_thread_id
                            ));
                            return Ok(());
                        } else {
                            let msg = val
                                .get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("执行器返回未知错误");
                            return Err(msg.to_string());
                        }
                    }
                }
            }
        }

        // Direct CLI fallback if python helper fails
        let binary = crate::modules::codex_wakeup::resolve_binary()
            .map_err(|e| format!("未找到本地 Codex CLI: {}", e.message))?;

        let mut cmd = crate::modules::codex_wakeup::build_binary_command(&binary);
        cmd.arg("exec");
        if let Some(ref tid) = task.thread_id {
            cmd.arg("resume");
            cmd.arg("--skip-git-repo-check");
            cmd.arg(tid);
            cmd.arg(&task.prompt);
        } else {
            cmd.arg("--skip-git-repo-check");
            cmd.arg("-C");
            cmd.arg(&task.cwd);
            if let Some(ref m) = task.model {
                cmd.arg("-c");
                cmd.arg(format!(r#"model="{}""#, m));
            }
            cmd.arg(&task.prompt);
        }

        let p = Path::new(&task.cwd);
        if p.is_dir() {
            cmd.current_dir(p);
        }
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());

        let spawned = cmd
            .spawn()
            .map_err(|e| format!("启动本地 Codex CLI 失败: {}", e))?;

        task.pid = Some(spawned.id());
        task.status = TaskStatus::Running;
        task.started_at = Some(Utc::now().timestamp());
        logger::log_info(&format!(
            "[CodexTaskQueue] 通过 CLI 降级启动本地排队任务: id={}, pid={}",
            task.id,
            spawned.id()
        ));
        Ok(())
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
            &["python3", "-", &b64_payload],
            Some(PYTHON_QUEUE_EXECUTE.to_string()),
        )
        .await?;

        let start_tag = "__CODEX_QUEUE_EXEC_JSON_START__";
        let end_tag = "__CODEX_QUEUE_EXEC_JSON_END__";

        if let (Some(s_idx), Some(e_idx)) = (output.find(start_tag), output.find(end_tag)) {
            let json_str = output[s_idx + start_tag.len()..e_idx].trim();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                if val.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                    if let Some(tid) = val.get("thread_id").and_then(|v| v.as_str()) {
                        if !tid.is_empty() {
                            task.assigned_thread_id = Some(tid.to_string());
                        }
                    }
                    if let Some(pid) = val.get("pid").and_then(|v| v.as_u64()) {
                        task.pid = Some(pid as u32);
                    }
                    task.status = TaskStatus::Running;
                    task.started_at = Some(Utc::now().timestamp());
                    logger::log_info(&format!(
                        "[CodexTaskQueue] 成功在 SSH 服务器【{}】启动排队任务: id={}",
                        server.name, task.id
                    ));
                    return Ok(());
                } else {
                    let err = val
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("远端 Python 启动失败");
                    return Err(err.to_string());
                }
            }
        }

        // SSH CLI fallback script
        let safe_cwd = if task.cwd.is_empty() { "$HOME" } else { &task.cwd };
        let escaped_prompt = task.prompt.replace('\'', "'\\''");
        let remote_script = if let Some(ref tid) = task.thread_id {
            format!(
                "export PATH=\"$HOME/.local/bin:$HOME/.npm-global/bin:/usr/local/bin:$PATH\"\n\
                 CODEX_BIN=$(which codex 2>/dev/null || echo \"$HOME/.local/bin/codex\")\n\
                 cd \"{}\" 2>/dev/null || cd \"$HOME\"\n\
                 nohup \"$CODEX_BIN\" exec resume --skip-git-repo-check \"{}\" '{}' > \"$HOME/.codex/task_queue_{}.log\" 2>&1 & echo $!",
                safe_cwd, tid, escaped_prompt, task.id
            )
        } else {
            let model_arg = if let Some(ref m) = task.model {
                format!("-c 'model=\"{}\"'", m)
            } else {
                "".to_string()
            };
            format!(
                "export PATH=\"$HOME/.local/bin:$HOME/.npm-global/bin:/usr/local/bin:$PATH\"\n\
                 CODEX_BIN=$(which codex 2>/dev/null || echo \"$HOME/.local/bin/codex\")\n\
                 cd \"{}\" 2>/dev/null || cd \"$HOME\"\n\
                 nohup \"$CODEX_BIN\" exec --skip-git-repo-check {} -C \"{}\" '{}' > \"$HOME/.codex/task_queue_{}.log\" 2>&1 & echo $!",
                safe_cwd, model_arg, safe_cwd, escaped_prompt, task.id
            )
        };

        let out = ssh_server::run_ssh(&server, 20, &["bash", "-c", &remote_script], None).await?;
        let pid_str = out.trim();
        if let Ok(pid) = pid_str.parse::<u32>() {
            task.pid = Some(pid);
        }
        task.status = TaskStatus::Running;
        task.started_at = Some(Utc::now().timestamp());
        logger::log_info(&format!(
            "[CodexTaskQueue] 通过 CLI 降级在 SSH 服务器【{}】启动排队任务: id={}",
            server.name, task.id
        ));
        Ok(())
    } else {
        Err(format!("未知任务来源: {}", task.source))
    }
}

pub async fn run_queue_dispatcher_step(app: Option<&AppHandle>) -> Result<(), String> {
    // Ensure only one dispatcher step executes at a time
    {
        let mut flag = dispatch_running_flag().lock().unwrap();
        if *flag {
            return Ok(());
        }
        *flag = true;
    }

    let result = run_queue_dispatcher_step_inner(app).await;

    {
        let mut flag = dispatch_running_flag().lock().unwrap();
        *flag = false;
    }

    result
}

async fn run_queue_dispatcher_step_inner(app: Option<&AppHandle>) -> Result<(), String> {
    let (mut tasks, config) = {
        let _guard = queue_lock().lock().unwrap();
        (load_tasks(), load_queue_config())
    };

    if !config.auto_dispatch {
        return Ok(());
    }

    let now_ts = Utc::now().timestamp();
    let mut updated = false;

    // 1. Check all currently RUNNING tasks for completion
    for task in tasks.iter_mut() {
        if task.status != TaskStatus::Running {
            continue;
        }

        let effective_thread_id = task
            .thread_id
            .as_deref()
            .or(task.assigned_thread_id.as_deref());

        if task.source == "local" {
            // Check if turn finished
            if let Some(t_id) = effective_thread_id {
                let is_busy = is_local_thread_busy(t_id);
                if !is_busy {
                    // Turn has concluded! Check result
                    if let Some((turn_status, error_json)) = check_local_turn_result(t_id) {
                        if turn_status == "completed" {
                            task.status = TaskStatus::Completed;
                            task.completed_at = Some(now_ts);
                            task.error_message = None;
                            updated = true;
                            logger::log_info(&format!(
                                "[CodexTaskQueue] 本地任务执行完成: id={}, thread_id={}",
                                task.id, t_id
                            ));
                        } else if turn_status == "failed" {
                            task.status = TaskStatus::Failed;
                            task.completed_at = Some(now_ts);
                            task.error_message = error_json.or(Some("任务执行失败".to_string()));
                            updated = true;
                        }
                    } else {
                        // Check if at least 15 seconds have elapsed and PID is no longer running
                        if let Some(started) = task.started_at {
                            if now_ts - started > 15 {
                                task.status = TaskStatus::Completed;
                                task.completed_at = Some(now_ts);
                                updated = true;
                            }
                        }
                    }
                }
            } else {
                // For new threads created locally where we didn't know thread_id upfront:
                if let Some(created_id) = find_latest_created_thread(&task.cwd) {
                    task.assigned_thread_id = Some(created_id.clone());
                    let is_busy = is_local_thread_busy(&created_id);
                    if !is_busy {
                        if let Some((turn_status, _)) = check_local_turn_result(&created_id) {
                            if turn_status == "completed" {
                                task.status = TaskStatus::Completed;
                                task.completed_at = Some(now_ts);
                                updated = true;
                            }
                        }
                    }
                }
            }
        } else if task.source == "ssh" {
            // For SSH, check elapsed time or remote status
            if let Some(started) = task.started_at {
                if let Some(ref server_id) = task.server_id {
                    if let Ok(server_list) = ssh_server::list_servers() {
                        if let Some(server) = server_list.servers.iter().find(|s| s.id == *server_id) {
                            // Check if interrupted tasks appeared on remote
                            if let Ok(interrupted) = codex_auto_resume::detect_remote_tasks(server).await {
                                if let Some(t_id) = effective_thread_id {
                                    if interrupted.iter().any(|it| it.thread_id == t_id) {
                                        task.status = TaskStatus::WaitingInterrupted;
                                        task.error_message = Some("远端任务受限中断，等待自动恢复".to_string());
                                        updated = true;
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }

                // If started over 25 seconds ago and PID check passes, mark completed
                if now_ts - started > 30 {
                    task.status = TaskStatus::Completed;
                    task.completed_at = Some(now_ts);
                    updated = true;
                }
            }
        }
    }

    // 2. Fetch all current interrupted tasks across local & SSH
    let interrupted_tasks = codex_auto_resume::detect_all_interrupted_tasks().await;

    // 3. Count currently running tasks
    let running_count = tasks.iter().filter(|t| t.status == TaskStatus::Running).count();
    let max_allowed = if config.concurrency_mode == "sequential" {
        1
    } else {
        config.max_concurrent_tasks.max(1)
    };

    // Set of threads currently running a task: (source, server_id, thread_id)
    let mut busy_threads: HashSet<(String, Option<String>, String)> = HashSet::new();
    for t in &tasks {
        if t.status == TaskStatus::Running {
            if let Some(tid) = t.thread_id.as_deref().or(t.assigned_thread_id.as_deref()) {
                busy_threads.insert((t.source.clone(), t.server_id.clone(), tid.to_string()));
            }
        }
    }

    let mut current_running = running_count;

    // 4. Iterate pending or waiting_interrupted tasks in queue order
    for idx in 0..tasks.len() {
        if current_running >= max_allowed {
            break;
        }

        let task_status = tasks[idx].status.clone();
        if task_status != TaskStatus::Pending && task_status != TaskStatus::WaitingInterrupted {
            continue;
        }

        let target_thread_id = tasks[idx]
            .thread_id
            .clone()
            .or_else(|| tasks[idx].assigned_thread_id.clone());

        // RULE 2: Same thread must execute SEQUENTIALLY
        if let Some(ref tid) = target_thread_id {
            let key = (tasks[idx].source.clone(), tasks[idx].server_id.clone(), tid.clone());
            if busy_threads.contains(&key) {
                // Another task is already executing in this same thread -> hold this task!
                continue;
            }
            if tasks[idx].source == "local" && is_local_thread_busy(tid) {
                // Local thread is actively generating or in progress -> wait!
                continue;
            }
        }

        // RULE 1: If thread has an INTERRUPTED task:
        // Must FIRST resume the interrupted task, let it finish, THEN submit the queued task!
        if let Some(ref tid) = target_thread_id {
            let matching_interrupted = interrupted_tasks.iter().find(|it| {
                it.thread_id == *tid
                    && it.source == tasks[idx].source
                    && it.server_id == tasks[idx].server_id
            });

            if let Some(interrupted) = matching_interrupted {
                // Mark queued task as waiting for interrupted task to resume and finish
                if tasks[idx].status != TaskStatus::WaitingInterrupted {
                    tasks[idx].status = TaskStatus::WaitingInterrupted;
                    tasks[idx].error_message =
                        Some("当前会话存在中断任务，等待其中断恢复并执行完成...".to_string());
                    updated = true;
                }

                // If interrupted task can be resumed now (quota available / time reached):
                if interrupted.can_resume {
                    logger::log_info(&format!(
                        "[CodexTaskQueue] 排队任务依赖的中断任务已满足恢复条件，优先启动恢复: thread_id={}",
                        tid
                    ));
                    let _ = codex_auto_resume::resume_task(
                        interrupted,
                        None,
                        "queue_priority_auto",
                    )
                    .await;
                }

                // Must wait for this interrupted task to finish!
                continue;
            } else {
                // If previously was waiting_interrupted, but interrupted task is no longer present:
                if tasks[idx].status == TaskStatus::WaitingInterrupted {
                    tasks[idx].status = TaskStatus::Pending;
                    tasks[idx].error_message = None;
                    updated = true;
                }
            }
        }

        // Check quota availability for local execution: if quota is hard-blocked and cannot resume, wait
        if tasks[idx].source == "local" {
            let (_, _, can_resume) = codex_auto_resume::get_current_codex_quota_resets_at();
            if !can_resume {
                logger::log_info(&format!(
                    "[CodexTaskQueue] 本地账号当前用量受限，排队任务暂缓提交: id={}",
                    tasks[idx].id
                ));
                continue;
            }
        }

        // Eligible to launch!
        let mut task_to_launch = tasks[idx].clone();
        match launch_task(&mut task_to_launch).await {
            Ok(()) => {
                tasks[idx] = task_to_launch;
                if let Some(tid) = tasks[idx].thread_id.as_deref().or(tasks[idx].assigned_thread_id.as_deref()) {
                    busy_threads.insert((tasks[idx].source.clone(), tasks[idx].server_id.clone(), tid.to_string()));
                }
                current_running += 1;
                updated = true;
            }
            Err(e) => {
                logger::log_warn(&format!(
                    "[CodexTaskQueue] 启动排队任务失败: id={}, error={}",
                    tasks[idx].id, e
                ));
                tasks[idx].status = TaskStatus::Failed;
                tasks[idx].error_message = Some(e);
                tasks[idx].completed_at = Some(now_ts);
                updated = true;
            }
        }
    }

    if updated {
        let _guard = queue_lock().lock().unwrap();
        save_tasks(&tasks)?;
        if let Some(app_handle) = app {
            let _ = app_handle.emit("codex://task-queue-updated", ());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_status_as_str() {
        assert_eq!(TaskStatus::Pending.as_str(), "pending");
        assert_eq!(TaskStatus::WaitingInterrupted.as_str(), "waiting_interrupted");
        assert_eq!(TaskStatus::Running.as_str(), "running");
        assert_eq!(TaskStatus::Completed.as_str(), "completed");
        assert_eq!(TaskStatus::Failed.as_str(), "failed");
        assert_eq!(TaskStatus::Cancelled.as_str(), "cancelled");
    }

    #[test]
    fn test_queue_config_default() {
        let config = CodexQueueConfig::default();
        assert_eq!(config.concurrency_mode, "sequential");
        assert_eq!(config.max_concurrent_tasks, 2);
        assert!(config.auto_dispatch);
    }

    #[test]
    fn test_add_and_reorder_tasks() {
        let req1 = CreateQueuedTaskRequest {
            title: Some("Task 1".to_string()),
            prompt: "Prompt 1".to_string(),
            source: "local".to_string(),
            server_id: None,
            server_name: None,
            cwd: "/tmp/project1".to_string(),
            thread_id: Some("thread-1".to_string()),
            thread_title: Some("Thread 1".to_string()),
            model: None,
        };
        let req2 = CreateQueuedTaskRequest {
            title: Some("Task 2".to_string()),
            prompt: "Prompt 2".to_string(),
            source: "local".to_string(),
            server_id: None,
            server_name: None,
            cwd: "/tmp/project1".to_string(),
            thread_id: Some("thread-1".to_string()),
            thread_title: Some("Thread 1".to_string()),
            model: None,
        };

        let t1 = add_task(req1).unwrap();
        let t2 = add_task(req2).unwrap();

        assert_eq!(t1.title, "Task 1");
        assert_eq!(t2.title, "Task 2");
        assert_eq!(t1.status, TaskStatus::Pending);

        // Reorder
        let reorder_res = reorder_queue(vec![t2.id.clone(), t1.id.clone()]);
        assert!(reorder_res.is_ok());

        let tasks = load_tasks();
        let pos1 = tasks.iter().position(|t| t.id == t1.id).unwrap();
        let pos2 = tasks.iter().position(|t| t.id == t2.id).unwrap();
        assert!(pos2 < pos1);

        // Delete test tasks
        let _ = delete_task(&t1.id);
        let _ = delete_task(&t2.id);
    }

    #[test]
    fn test_retry_and_clear_completed() {
        let req = CreateQueuedTaskRequest {
            title: Some("Task to fail".to_string()),
            prompt: "Prompt".to_string(),
            source: "local".to_string(),
            server_id: None,
            server_name: None,
            cwd: "/tmp/test".to_string(),
            thread_id: None,
            thread_title: None,
            model: None,
        };
        let mut t = add_task(req).unwrap();
        t.status = TaskStatus::Failed;
        t.error_message = Some("Simulated error".to_string());
        let _ = update_task(t.clone());

        assert_eq!(load_tasks().iter().find(|x| x.id == t.id).unwrap().status, TaskStatus::Failed);

        let _ = retry_task(&t.id);
        let retried = load_tasks().into_iter().find(|x| x.id == t.id).unwrap();
        assert_eq!(retried.status, TaskStatus::Pending);
        assert!(retried.error_message.is_none());

        // Mark completed and clear
        let mut retried_completed = retried.clone();
        retried_completed.status = TaskStatus::Completed;
        let _ = update_task(retried_completed);
        let _ = clear_completed_tasks();
        assert!(!load_tasks().iter().any(|x| x.id == t.id));
    }
}

