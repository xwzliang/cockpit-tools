use crate::models::ssh_server::SshServer;
use crate::modules::{config, logger, ssh_server};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter};

static RESUMED_TASKS_CACHE: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();

fn resumed_tasks_cache() -> &'static Mutex<HashMap<String, i64>> {
    RESUMED_TASKS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptedAntigravityTask {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub source: String, // "local" | "ssh"
    #[serde(default)]
    pub server_id: Option<String>,
    #[serde(default)]
    pub server_name: Option<String>,
    pub thread_id: String, // conversation_id
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
    pub status: String, // "usage_limited" | "blocked"
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
    #[serde(default)]
    pub is_excluded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeTaskResult {
    pub success: bool,
    pub message: String,
}

const RESUMED_TASKS_FILE: &str = "antigravity_auto_resumed_tasks.json";
const EXCLUDED_TASKS_FILE: &str = "antigravity_auto_resume_excluded_tasks.json";

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

fn resumed_tasks_file_path() -> Result<PathBuf, String> {
    Ok(crate::modules::account::get_data_dir()?.join(RESUMED_TASKS_FILE))
}

fn excluded_tasks_file_path() -> Result<PathBuf, String> {
    Ok(crate::modules::account::get_data_dir()?.join(EXCLUDED_TASKS_FILE))
}

pub fn load_excluded_task_ids() -> HashSet<String> {
    let path = match excluded_tasks_file_path() {
        Ok(p) => p,
        Err(_) => return HashSet::new(),
    };
    if !path.exists() {
        return HashSet::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => HashSet::new(),
    }
}

pub fn save_excluded_task_ids(ids: &HashSet<String>) -> Result<(), String> {
    let path = excluded_tasks_file_path()?;
    let content = serde_json::to_string_pretty(ids)
        .map_err(|e| format!("序列化排除任务失败: {}", e))?;
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
import os, sys, glob, json, sqlite3, time

def parse_iso_time(s):
    if not s: return int(time.time())
    try:
        from datetime import datetime
        s_clean = s.split('.')[0].replace('Z', '')
        if '+' in s_clean: s_clean = s_clean.split('+')[0]
        dt = datetime.fromisoformat(s_clean)
        return int(dt.timestamp())
    except:
        return int(time.time())

def check_task():
    home = os.path.expanduser('~')
    results = []
    db_candidates = [
        os.path.join(home, '.gemini', 'antigravity-cli', 'conversation_summaries.db'),
        os.path.join(home, '.gemini', 'antigravity', 'conversation_summaries.db'),
        os.path.join(home, '.gemini', 'conversation_summaries.db')
    ]
    seen_ids = set()

    for db_path in db_candidates:
        if not os.path.exists(db_path):
            continue
        root_dir = os.path.dirname(db_path)
        try:
            conn = sqlite3.connect(db_path)
            cursor = conn.cursor()
            cursor.execute('''
                SELECT conversation_id, title, preview, status, not_fully_idle, killed, last_modified_time, workspace_uris
                FROM conversation_summaries
                ORDER BY last_modified_time DESC
                LIMIT 50
            ''')
            rows = cursor.fetchall()
            for r in rows:
                cid, title, preview, c_status, not_fully_idle, killed, last_mod_str, ws_uris = r
                if not cid or cid in seen_ids:
                    continue
                
                transcript_path = os.path.join(root_dir, 'brain', cid, '.system_generated', 'logs', 'transcript.jsonl')
                last_step = None
                if os.path.exists(transcript_path):
                    try:
                        with open(transcript_path, 'r', encoding='utf-8', errors='ignore') as f:
                            lines = [l for l in f if l.strip()]
                            if lines:
                                last_step = json.loads(lines[-1])
                    except:
                        pass
                
                is_interrupted = False
                error_message = None
                is_quota_limited = False

                if last_step:
                    st = last_step.get('status')
                    err = last_step.get('error') or ''
                    content = str(last_step.get('content', ''))
                    
                    if st == 'ERROR' or err:
                        is_interrupted = True
                        error_message = err or content[:250]
                        combined = (str(err) + ' ' + content).lower()
                        if any(k in combined for k in ['quota', 'resource_exhausted', 'rate limit', 'rate_limit', 'usage limit', '429', 'capacity']):
                            is_quota_limited = True
                
                if not is_interrupted and (not_fully_idle or c_status == 'CASCADE_RUN_STATUS_RUNNING') and killed:
                    is_interrupted = True
                    error_message = 'Task was cancelled or interrupted during execution'
                
                if is_interrupted and is_quota_limited:
                    seen_ids.add(cid)
                    
                    cwd = None
                    if ws_uris:
                        try:
                            uris = json.loads(ws_uris)
                            if isinstance(uris, list) and uris:
                                cwd = uris[0].replace('file://', '')
                        except:
                            cwd = ws_uris.replace('file://', '')

                    display_title = title if (title and title.strip()) else (preview[:50] if preview else cid)
                    
                    results.append({
                        'id': f'ssh:{cid}',
                        'source': 'ssh',
                        'thread_id': cid,
                        'is_goal': False,
                        'title': display_title,
                        'objective': preview,
                        'cwd': cwd,
                        'model': None,
                        'status': 'usage_limited',
                        'error_message': error_message,
                        'resets_at': None,
                        'reset_window_minutes': None,
                        'updated_at': parse_iso_time(last_mod_str),
                        'can_resume': False
                    })
        except Exception:
            pass

    print('__ANTIGRAVITY_PROBE_JSON_START__')
    print(json.dumps(results))
    print('__ANTIGRAVITY_PROBE_JSON_END__')

if __name__ == '__main__':
    check_task()
"#;

fn parse_iso_datetime_to_ts(iso: &str) -> Option<i64> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(iso) {
        return Some(dt.timestamp());
    }
    // Try formats like 2026-10-05 06:00:00 or without Z
    let cleaned = iso.replace('Z', "+00:00");
    if let Ok(dt) = DateTime::parse_from_rfc3339(&cleaned) {
        return Some(dt.timestamp());
    }
    None
}

/// 获取当前 Antigravity 账号的配额重置时间戳（秒）
fn get_current_antigravity_quota_resets_at() -> (Option<i64>, bool) {
    let accounts = match crate::modules::account::list_accounts() {
        Ok(list) => list,
        Err(_) => return (None, true),
    };
    let current_id = match crate::modules::account::get_current_account_id() {
        Ok(Some(id)) => id,
        _ => return (None, true),
    };
    let current_acc = accounts.into_iter().find(|a| a.id == current_id);
    let Some(acc) = current_acc else {
        return (None, true);
    };
    let Some(ref quota) = acc.quota else {
        return (None, true);
    };

    if quota.is_forbidden {
        return (None, false);
    }

    let now_ts = Utc::now().timestamp();
    let mut earliest_reset: Option<i64> = None;
    let mut has_exhausted_model = false;

    for model in &quota.models {
        if model.percentage <= 0 {
            has_exhausted_model = true;
            if let Some(ts) = parse_iso_datetime_to_ts(&model.reset_time) {
                if ts > now_ts && (earliest_reset.is_none() || Some(ts) < earliest_reset) {
                    earliest_reset = Some(ts);
                }
            }
        }
    }

    let can_resume = !has_exhausted_model || earliest_reset.map(|r| now_ts >= r).unwrap_or(true);
    (earliest_reset, can_resume)
}

pub fn detect_local_tasks() -> Vec<InterruptedAntigravityTask> {
    let mut tasks = Vec::new();
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return tasks,
    };

    let (earliest_resets_at, current_can_resume) = get_current_antigravity_quota_resets_at();
    let now_ts = Utc::now().timestamp();
    let excluded_ids = load_excluded_task_ids();

    let db_paths = vec![
        home.join(".gemini").join("antigravity").join("conversation_summaries.db"),
        home.join(".gemini").join("antigravity-cli").join("conversation_summaries.db"),
    ];

    let mut seen_ids = HashSet::new();

    for db_path in db_paths {
        if !db_path.exists() {
            continue;
        }
        let root_dir = match db_path.parent() {
            Some(p) => p,
            None => continue,
        };

        let conn = match rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let query = "SELECT conversation_id, title, preview, status, not_fully_idle, killed, last_modified_time, workspace_uris FROM conversation_summaries ORDER BY last_modified_time DESC LIMIT 50";
        let mut stmt = match conn.prepare(query) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        });

        let Ok(mapped) = rows else { continue };

        for item in mapped.flatten() {
            let (cid, title_opt, preview_opt, status_opt, not_fully_idle, killed, last_mod_str, ws_uris) = item;
            if cid.trim().is_empty() || seen_ids.contains(&cid) {
                continue;
            }

            // Check transcript file
            let transcript_path = root_dir.join("brain").join(&cid).join(".system_generated").join("logs").join("transcript.jsonl");
            let mut last_step_status: Option<String> = None;
            let mut error_msg: Option<String> = None;
            let mut is_quota_limited = false;
            let mut last_step_created_at_ts: Option<i64> = None;

            if transcript_path.exists() {
                if let Ok(content) = std::fs::read(&transcript_path) {
                    let text = String::from_utf8_lossy(&content);
                    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
                    if let Some(last_line) = lines.last() {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(last_line) {
                            last_step_status = val.get("status").and_then(|s| s.as_str()).map(str::to_string);
                            let err_str = val.get("error").and_then(|e| e.as_str());
                            let content_str = val.get("content").and_then(|c| c.as_str()).unwrap_or("");
                            let created_at_str = val.get("created_at").and_then(|c| c.as_str());
                            if let Some(c_at) = created_at_str {
                                last_step_created_at_ts = parse_iso_datetime_to_ts(c_at);
                            }

                            if last_step_status.as_deref() == Some("ERROR") || err_str.is_some() {
                                error_msg = err_str.map(str::to_string).or_else(|| {
                                    if !content_str.is_empty() {
                                        Some(content_str[..content_str.len().min(250)].to_string())
                                    } else {
                                        None
                                    }
                                });
                                let combined = format!("{} {}", err_str.unwrap_or(""), content_str).to_lowercase();
                                if ["quota", "resource_exhausted", "rate limit", "rate_limit", "usage limit", "429", "capacity"]
                                    .iter()
                                    .any(|k| combined.contains(k))
                                {
                                    is_quota_limited = true;
                                }
                            }
                        }
                    }
                }
            }

            // If last step was modified in the last 20 seconds and finished cleanly (not ERROR), it is actively running in real-time
            if let Some(ts) = last_step_created_at_ts {
                if now_ts - ts < 20 && last_step_status.as_deref() != Some("ERROR") {
                    continue;
                }
            }

            let mut is_interrupted = false;
            let c_status = status_opt.as_deref().unwrap_or("");
            let not_idle = not_fully_idle.unwrap_or(0) == 1;
            let is_killed = killed.unwrap_or(0) == 1;

            if last_step_status.as_deref() == Some("ERROR") || error_msg.is_some() {
                is_interrupted = true;
            } else if (not_idle || c_status == "CASCADE_RUN_STATUS_RUNNING") && is_killed {
                is_interrupted = true;
                if error_msg.is_none() {
                    error_msg = Some("任务在执行过程中被终止中断".to_string());
                }
            }

            if is_interrupted && is_quota_limited {
                seen_ids.insert(cid.clone());

                let mut cwd = None;
                if let Some(ref ws) = ws_uris {
                    if let Ok(arr) = serde_json::from_str::<Vec<String>>(ws) {
                        if let Some(first) = arr.first() {
                            cwd = Some(first.replace("file://", ""));
                        }
                    } else {
                        cwd = Some(ws.replace("file://", ""));
                    }
                }

                let title = title_opt
                    .filter(|t| !t.trim().is_empty())
                    .or_else(|| preview_opt.as_ref().filter(|p| !p.trim().is_empty()).cloned())
                    .unwrap_or_else(|| cid.clone());

                let updated_at = last_mod_str
                    .as_deref()
                    .and_then(parse_iso_datetime_to_ts)
                    .unwrap_or(now_ts);

                let task_id = format!("local:{}", cid);
                let is_excluded = excluded_ids.contains(&task_id);

                tasks.push(InterruptedAntigravityTask {
                    id: task_id,
                    source: "local".to_string(),
                    server_id: None,
                    server_name: None,
                    thread_id: cid,
                    goal_id: None,
                    is_goal: false,
                    title,
                    objective: preview_opt,
                    cwd,
                    model: None,
                    status: "usage_limited".to_string(),
                    error_message: error_msg,
                    resets_at: earliest_resets_at,
                    reset_window_minutes: None,
                    updated_at,
                    can_resume: current_can_resume,
                    is_excluded,
                });
            }
        }
    }

    tasks
}

pub async fn detect_remote_tasks(server: &SshServer) -> Result<Vec<InterruptedAntigravityTask>, String> {
    let output = ssh_server::run_ssh(
        server,
        15,
        &["python3", "-"],
        Some(PYTHON_REMOTE_PROBE.to_string()),
    )
    .await?;

    let start_tag = "__ANTIGRAVITY_PROBE_JSON_START__";
    let end_tag = "__ANTIGRAVITY_PROBE_JSON_END__";

    let (Some(s_idx), Some(e_idx)) = (output.find(start_tag), output.find(end_tag)) else {
        return Ok(Vec::new());
    };

    let json_str = output[s_idx + start_tag.len()..e_idx].trim();
    let mut tasks: Vec<InterruptedAntigravityTask> = serde_json::from_str(json_str)
        .map_err(|e| format!("解析 SSH 服务器【{}】Antigravity 任务返回失败: {}", server.name, e))?;

    let excluded_ids = load_excluded_task_ids();
    let (earliest_resets_at, current_can_resume) = get_current_antigravity_quota_resets_at();

    for task in &mut tasks {
        task.id = format!("ssh:{}:{}", server.id, task.thread_id);
        task.server_id = Some(server.id.clone());
        task.server_name = Some(server.name.clone());
        task.is_excluded = excluded_ids.contains(&task.id);
        if task.status == "usage_limited" {
            task.resets_at = earliest_resets_at;
            task.can_resume = current_can_resume;
        }
    }

    Ok(tasks)
}

pub async fn detect_all_interrupted_tasks() -> Vec<InterruptedAntigravityTask> {
    let cfg = config::get_user_config();
    let local_tasks = detect_local_tasks();

    if !cfg.antigravity_auto_resume_sync_ssh {
        return local_tasks;
    }

    let servers = match ssh_server::list_servers() {
        Ok(list) => list.servers,
        Err(_) => return local_tasks,
    };

    let mut all_tasks = local_tasks;
    let mut handles = Vec::new();

    for server in servers {
        handles.push(tokio::spawn(async move {
            match detect_remote_tasks(&server).await {
                Ok(remote) => remote,
                Err(err) => {
                    logger::log_warn(&format!(
                        "[AntigravityAutoResume] 检测 SSH 服务器【{}】任务失败: {}",
                        server.name, err
                    ));
                    Vec::new()
                }
            }
        }));
    }

    for h in handles {
        if let Ok(res) = h.await {
            all_tasks.extend(res);
        }
    }

    let (earliest_resets_at, current_can_resume) = get_current_antigravity_quota_resets_at();
    let excluded_ids = load_excluded_task_ids();
    for task in &mut all_tasks {
        task.resets_at = earliest_resets_at;
        task.can_resume = current_can_resume;
        task.is_excluded = excluded_ids.contains(&task.id) || excluded_ids.contains(&task.thread_id);
    }

    all_tasks
}

/// 查找本地正在监听的 language_server 端口 (HTTP)
fn find_local_ls_port() -> Option<u16> {
    let output = std::process::Command::new("lsof")
        .args(&["-iTCP", "-sTCP:LISTEN", "-n", "-P"])
        .output()
        .ok()?;

    let text = String::from_utf8_lossy(&output.stdout);
    let mut candidate_ports = Vec::new();

    for line in text.lines() {
        if line.contains("language_") || line.contains("language_server") {
            // Find port, e.g. 127.0.0.1:63744
            if let Some(idx) = line.find("127.0.0.1:") {
                let rest = &line[idx + 10..];
                if let Some(port_str) = rest.split_whitespace().next() {
                    if let Ok(p) = port_str.parse::<u16>() {
                        candidate_ports.push(p);
                    }
                }
            }
        }
    }

    let agentapi_bin = dirs::home_dir()?.join(".gemini").join("antigravity").join("bin").join("agentapi");
    if !agentapi_bin.exists() {
        return candidate_ports.first().copied();
    }

    // Test ports to find the one responding with valid metadata
    for port in candidate_ports {
        let mut cmd = std::process::Command::new(&agentapi_bin);
        cmd.arg("get-conversation-metadata");
        cmd.arg("00000000-0000-0000-0000-000000000000"); // dummy or check exit
        cmd.env("ANTIGRAVITY_LS_ADDRESS", format!("localhost:{}", port));
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Ok(out) = cmd.output() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            // If it connected (even if conversation not found, it won't say 'ANTIGRAVITY_LS_ADDRESS is not set')
            if !stdout.contains("ANTIGRAVITY_LS_ADDRESS is not set") && !stderr.contains("ANTIGRAVITY_LS_ADDRESS is not set") {
                return Some(port);
            }
        }
    }

    None
}

async fn resume_local_task(thread_id: &str, prompt: &str) -> Result<ResumeTaskResult, String> {
    let home = dirs::home_dir().ok_or_else(|| "无法获取用户主目录".to_string())?;
    let agentapi_bin = home.join(".gemini").join("antigravity").join("bin").join("agentapi");

    // 1. Try agentapi with discovered language_server port
    if agentapi_bin.exists() {
        if let Some(port) = find_local_ls_port() {
            logger::log_info(&format!(
                "[AntigravityAutoResume] 找到本地 language_server 端口: {}, 正在向会话 {} 发送恢复消息",
                port, thread_id
            ));
            let mut cmd = std::process::Command::new(&agentapi_bin);
            cmd.arg("send-message");
            cmd.arg(thread_id);
            cmd.arg(prompt);
            cmd.env("ANTIGRAVITY_LS_ADDRESS", format!("localhost:{}", port));
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

            if let Ok(output) = cmd.output() {
                if output.status.success() {
                    return Ok(ResumeTaskResult {
                        success: true,
                        message: format!("已成功通过 Antigravity API 向会话 {} 触发恢复执行", thread_id),
                    });
                } else {
                    let err = String::from_utf8_lossy(&output.stderr);
                    logger::log_warn(&format!("[AntigravityAutoResume] agentapi 执行失败: {}", err));
                }
            }
        }
    }

    // 2. Try agy CLI if installed on local machine
    let mut agy_bin = home.join(".local").join("bin").join("agy");
    if !agy_bin.exists() {
        for candidate in ["/usr/local/bin/agy", "/opt/homebrew/bin/agy", "/usr/bin/agy"] {
            let p = PathBuf::from(candidate);
            if p.exists() {
                agy_bin = p;
                break;
            }
        }
    }

    if agy_bin.exists() {
        logger::log_info(&format!(
            "[AntigravityAutoResume] 通过本地 agy CLI 触发恢复会话: {}",
            thread_id
        ));
        let child = std::process::Command::new(&agy_bin)
            .args(&["--conversation", thread_id, "--dangerously-skip-permissions", "--print", prompt])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();

        if let Ok(_) = child {
            return Ok(ResumeTaskResult {
                success: true,
                message: format!("已在后台启动 agy CLI 会话恢复: {}", thread_id),
            });
        }
    }

    // 3. Fallback: activate / open Antigravity application window
    #[cfg(target_os = "macos")]
    {
        let url = format!("antigravity://cascade?id={}", thread_id);
        let _ = std::process::Command::new("open").arg(&url).spawn();
        let _ = std::process::Command::new("open").arg("-a").arg("Antigravity").spawn();

        return Ok(ResumeTaskResult {
            success: true,
            message: format!("已唤醒 Antigravity 应用并定位到会话: {}", thread_id),
        });
    }

    #[cfg(not(target_os = "macos"))]
    Err(format!("未找到可用的 Antigravity 恢复入口，请确保 Antigravity IDE 或 agy 运行中"))
}

async fn resume_remote_task(
    server: &SshServer,
    thread_id: &str,
    prompt: &str,
) -> Result<ResumeTaskResult, String> {
    let escaped_prompt = prompt.replace('\'', "'\\''");
    let remote_script = format!(
        "export PATH=\"$HOME/.local/bin:$HOME/bin:/usr/local/bin:$PATH\"\n\
        if command -v agy >/dev/null 2>&1; then\n\
            nohup agy --conversation '{cid}' --dangerously-skip-permissions --print '{prompt}' >/tmp/agy_resume_{cid}.log 2>&1 &\n\
            echo '__RESUME_SUCCESS__'\n\
        elif [ -x \"$HOME/.gemini/antigravity-cli/bin/agentapi\" ]; then\n\
            nohup \"$HOME/.gemini/antigravity-cli/bin/agentapi\" send-message '{cid}' '{prompt}' >/tmp/agy_resume_{cid}.log 2>&1 &\n\
            echo '__RESUME_SUCCESS__'\n\
        else\n\
            echo 'Error: Neither agy nor agentapi found on remote server'\n\
        fi",
        cid = thread_id,
        prompt = escaped_prompt
    );

    let output = ssh_server::run_ssh(server, 20, &["bash", "-c", &remote_script], None).await?;

    if output.contains("__RESUME_SUCCESS__") {
        Ok(ResumeTaskResult {
            success: true,
            message: format!("已在 SSH 服务器【{}】后台恢复任务: {}", server.name, thread_id),
        })
    } else {
        Err(format!("在服务器【{}】恢复失败: {}", server.name, output.trim()))
    }
}

pub async fn resume_task(
    task: &InterruptedAntigravityTask,
    custom_prompt: Option<String>,
    trigger_type: &str,
) -> Result<ResumeTaskResult, String> {
    let prompt = custom_prompt.unwrap_or_else(|| "Continue where you left off and finish the task.".to_string());

    let res = if task.source == "local" {
        resume_local_task(&task.thread_id, &prompt).await
    } else if task.source == "ssh" {
        let server_id = task
            .server_id
            .as_deref()
            .ok_or_else(|| "SSH 任务缺少 server_id".to_string())?;

        let servers = ssh_server::list_servers()
            .map_err(|e| format!("加载 SSH 服务器列表失败: {}", e))?
            .servers;

        let server = servers
            .iter()
            .find(|s| s.id == server_id)
            .ok_or_else(|| format!("未找到目标 SSH 服务器: {}", server_id))?;

        resume_remote_task(server, &task.thread_id, &prompt).await
    } else {
        Err(format!("未知任务来源: {}", task.source))
    };

    let record = ResumedTaskRecord {
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
        resumed_at: Utc::now().timestamp(),
        trigger_type: trigger_type.to_string(),
        success: res.is_ok(),
        message: match &res {
            Ok(r) => r.message.clone(),
            Err(e) => e.clone(),
        },
    };

    record_resumed_task(record);
    res
}

pub async fn check_and_auto_resume(app: &AppHandle) {
    let cfg = config::get_user_config();
    if !cfg.antigravity_auto_resume_interrupted_tasks_enabled {
        return;
    }

    let tasks = detect_all_interrupted_tasks().await;
    let now_ts = Utc::now().timestamp();

    for task in tasks {
        if !task.can_resume || task.is_excluded {
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
            "[AntigravityAutoResume] 检测到任务恢复条件满足，正在自动恢复任务: id={}, title={}",
            task.id, task.title
        ));

        let res = resume_task(&task, None, "auto").await;
        match res {
            Ok(result) => {
                logger::log_info(&format!(
                    "[AntigravityAutoResume] 自动恢复成功: {} - {}",
                    task.id, result.message
                ));

                let title = "Antigravity 任务已自动恢复";
                let display_title = if task.title.len() > 50 {
                    format!("{}...", &task.title[..47])
                } else {
                    task.title.clone()
                };
                let body = format!("已自动恢复执行：{}", display_title);

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

                let _ = app.emit("antigravity://task-auto-resumed", &task);
            }
            Err(err) => {
                logger::log_warn(&format!(
                    "[AntigravityAutoResume] 自动恢复任务失败: {} - {}",
                    task.id, err
                ));
                let mut cache = resumed_tasks_cache().lock().unwrap_or_else(|e| e.into_inner());
                cache.insert(task.id.clone(), now_ts - 1800 + 60);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_exclusion_logic() {
        let test_task_id = "test:antigravity:exclude:123";
        let _ = set_task_excluded(test_task_id, false);
        assert!(!load_excluded_task_ids().contains(test_task_id));
        set_task_excluded(test_task_id, true).unwrap();
        assert!(load_excluded_task_ids().contains(test_task_id));
        set_task_excluded(test_task_id, false).unwrap();
        assert!(!load_excluded_task_ids().contains(test_task_id));
    }

    #[tokio::test]
    async fn test_remote_detection_on_pve() {
        if let Ok(server_list) = ssh_server::list_servers() {
            if let Some(server) = server_list.servers.iter().find(|s| s.host.contains("pve-ubuntu")) {
                println!("Testing Antigravity on server: {} ({})", server.name, server.host);
                let tasks = detect_remote_tasks(server).await;
                println!("Detected remote Antigravity tasks: {:?}", tasks);
                assert!(tasks.is_ok());
                let tasks = tasks.unwrap();
                println!("Found tasks count: {}", tasks.len());
            }
        }
    }
}
