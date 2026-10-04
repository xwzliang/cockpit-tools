use crate::models::ssh_server::SshServer;
use crate::modules::{config, logger, ssh_server};
use chrono::Utc;
use rusqlite::OpenFlags;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter};

static RESUMED_TASKS_CACHE: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();

fn resumed_tasks_cache() -> &'static Mutex<HashMap<String, i64>> {
    RESUMED_TASKS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
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
    pub updated_at: i64,
    #[serde(default)]
    pub can_resume: bool,
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
                err = payload.get("error")
                if isinstance(err, dict):
                    if error_msg is None:
                        error_msg = err.get("message")
                    code = err.get("codex_error_info")
                    if code == "usage_limit_exceeded" or (error_msg and "usage limit" in error_msg.lower()):
                        is_usage_limited = True
                        
            if p_type == "token_count":
                rl = payload.get("rate_limits", {})
                pri = rl.get("primary")
                if pri and isinstance(pri, dict):
                    if resets_at is None and pri.get("resets_at"):
                        resets_at = pri.get("resets_at")
                    if reset_window_minutes is None and pri.get("window_minutes"):
                        reset_window_minutes = pri.get("window_minutes")
                        
            if (resets_at or is_usage_limited) and error_msg:
                break
                
        return resets_at, reset_window_minutes, error_msg, is_usage_limited
    except Exception as e:
        return None, None, str(e), False

def detect_interrupted_tasks():
    codex_home_env = os.environ.get("CODEX_HOME")
    codex_home = Path(codex_home_env) if codex_home_env else (Path.home() / ".codex")
    goals_db = codex_home / "goals_1.sqlite"
    state_db = codex_home / "state_5.sqlite"
    
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
            
            cur_g.execute("SELECT thread_id, goal_id, objective, status, updated_at_ms FROM thread_goals WHERE status = 'usage_limited'")
            for row in cur_g.fetchall():
                thread_id, goal_id, objective, status, updated_at_ms = row
                seen_threads.add(thread_id)
                
                cur_s.execute("SELECT cwd, title, model, rollout_path FROM threads WHERE id = ?", (thread_id,))
                s_row = cur_s.fetchone()
                cwd = s_row[0] if s_row else None
                title = s_row[1] if s_row and s_row[1] else objective
                model = s_row[2] if s_row else None
                rollout_path = s_row[3] if s_row else None
                
                resets_at, reset_window_minutes, error_msg, _ = parse_rollout_tail(rollout_path)
                can_resume = (resets_at is None) or (now_ts >= resets_at)
                
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
                    "status": status,
                    "error_message": error_msg,
                    "resets_at": resets_at,
                    "reset_window_minutes": reset_window_minutes,
                    "updated_at": int(updated_at_ms / 1000) if updated_at_ms else now_ts,
                    "can_resume": can_resume
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
                WHERE archived = 0 AND updated_at > ?
                ORDER BY updated_at DESC LIMIT 50
            """, (now_ts - 7 * 86400,))
            for row in cur_s.fetchall():
                t_id, cwd, title, model, rollout_path, updated_at = row
                if t_id in seen_threads:
                    continue
                resets_at, reset_window_minutes, error_msg, is_limited = parse_rollout_tail(rollout_path)
                if is_limited:
                    can_resume = (resets_at is None) or (now_ts >= resets_at)
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

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            let payload = val.get("payload");
            let p_type = payload.and_then(|p| p.get("type")).and_then(|t| t.as_str());

            if p_type == Some("task_complete") {
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
                }
            }

            if p_type == Some("token_count") {
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
                }
            }

            if (resets_at.is_some() || is_usage_limited) && error_msg.is_some() {
                break;
            }
        }
    }

    (resets_at, reset_window_minutes, error_msg, is_usage_limited)
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
    let now_ts = Utc::now().timestamp();
    let mut seen_threads = std::collections::HashSet::new();

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
                "SELECT thread_id, goal_id, objective, status, updated_at_ms FROM thread_goals WHERE status = 'usage_limited'",
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
                        let (thread_id, goal_id, objective, status, updated_at_ms) = item;
                        seen_threads.insert(thread_id.clone());

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

                        let (resets_at, reset_window_minutes, error_msg, _) =
                            if let Some(ref r_path) = rollout_path {
                                parse_rollout_tail(Path::new(r_path))
                            } else {
                                (None, None, None, false)
                            };

                        let can_resume = resets_at.map(|r| now_ts >= r).unwrap_or(true);
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
                            status,
                            error_message: error_msg,
                            resets_at,
                            reset_window_minutes,
                            updated_at,
                            can_resume,
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
                "SELECT id, cwd, title, model, rollout_path, updated_at FROM threads WHERE archived = 0 AND updated_at > ? ORDER BY updated_at DESC LIMIT 50",
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
                        let (resets_at, reset_window_minutes, error_msg, is_limited) =
                            if let Some(ref r_path) = rollout_path {
                                parse_rollout_tail(Path::new(r_path))
                            } else {
                                (None, None, None, false)
                            };

                        if is_limited {
                            let can_resume = resets_at.map(|r| now_ts >= r).unwrap_or(true);
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
                                resets_at,
                                reset_window_minutes,
                                updated_at,
                                can_resume,
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
        Some(PYTHON_REMOTE_PROBE.to_string()),
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

    for task in &mut raw_tasks {
        task.source = "ssh".to_string();
        task.server_id = Some(server.id.clone());
        task.server_name = Some(server.name.clone());
        task.id = format!("ssh:{}:{}", server.id, task.thread_id);
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
            "/goal resume".to_string()
        } else {
            "Continue where you left off and finish the task.".to_string()
        }
    });

    if task.source == "local" {
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
            "[CodexAutoResume] 成功启动本地任务恢复: thread_id={}",
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

        let safe_cwd = task.cwd.as_deref().unwrap_or("$HOME");
        // Escaped shell arguments
        let escaped_prompt = prompt.replace('\'', "'\\''");
        let remote_script = format!(
            "export PATH=\"$HOME/.local/bin:$HOME/.npm-global/bin:/usr/local/bin:$PATH\"\n\
             CODEX_BIN=$(which codex 2>/dev/null || echo \"$HOME/.local/bin/codex\")\n\
             cd \"{}\" 2>/dev/null || cd \"$HOME\"\n\
             nohup \"$CODEX_BIN\" exec resume --skip-git-repo-check \"{}\" '{}' > \"$HOME/.codex/auto_resume_{}.log\" 2>&1 &",
            safe_cwd, task.thread_id, escaped_prompt, task.thread_id
        );

        ssh_server::run_ssh(&server, 20, &["bash", "-c", &remote_script], None).await?;

        logger::log_info(&format!(
            "[CodexAutoResume] 成功在 SSH 服务器【{}】启动任务恢复: thread_id={}",
            server.name, task.thread_id
        ));

        Ok(ResumeTaskResult {
            success: true,
            message: format!(
                "已在服务器【{}】后台启动恢复任务: {}",
                server.name, task.thread_id
            ),
        })
    } else {
        Err(format!("未知任务来源: {}", task.source))
    }
}

pub async fn check_and_auto_resume(app: &AppHandle) {
    let cfg = config::get_user_config();
    if !cfg.codex_auto_resume_interrupted_tasks_enabled {
        return;
    }

    let tasks = detect_all_interrupted_tasks().await;
    let now_ts = Utc::now().timestamp();

    for task in tasks {
        if !task.can_resume {
            continue;
        }

        // Deduplication check: avoid triggering repeatedly within 30 minutes
        let should_trigger = {
            let mut cache = resumed_tasks_cache().lock().unwrap_or_else(|e| e.into_inner());
            match cache.get(&task.id) {
                Some(&last_trigger) if now_ts - last_trigger < 1800 => false,
                _ => {
                    cache.insert(task.id.clone(), now_ts);
                    true
                }
            }
        };

        if !should_trigger {
            continue;
        }

        logger::log_info(&format!(
            "[CodexAutoResume] 检测到配额恢复，正在自动恢复任务: id={}, title={}",
            task.id, task.title
        ));

        let res = resume_task(&task, None, "auto").await;
        match res {
            Ok(result) => {
                logger::log_info(&format!(
                    "[CodexAutoResume] 自动恢复成功: {} - {}",
                    task.id, result.message
                ));

                // Send OS notification
                let title = "Codex 任务已自动恢复";
                let display_title = if task.title.len() > 50 {
                    format!("{}...", &task.title[..47])
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
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_remote_detection_on_pve() {
        if let Ok(server_list) = ssh_server::list_servers() {
            if let Some(server) = server_list.servers.iter().find(|s| s.host.contains("pve-ubuntu")) {
                println!("Testing server: {} ({})", server.name, server.host);
                let tasks = detect_remote_tasks(server).await;
                println!("Detected tasks: {:?}", tasks);
                assert!(tasks.is_ok());
                let tasks = tasks.unwrap();
                assert!(!tasks.is_empty(), "Should detect at least 1 interrupted task");
                println!("Found task: {:?}", tasks[0]);
                assert_eq!(tasks[0].thread_id, "01a0f811-0c03-79d1-a5a8-d34fa3a564e7");
                assert_eq!(tasks[0].status, "usage_limited");
                assert!(tasks[0].is_goal);
                assert_eq!(tasks[0].resets_at, Some(1791138029));
            }
        }
    }
}

