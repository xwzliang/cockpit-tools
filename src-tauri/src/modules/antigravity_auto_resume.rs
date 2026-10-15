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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_agent_id: Option<String>,
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

const DISMISSED_TASKS_FILE: &str = "antigravity_auto_resume_dismissed_tasks.json";

fn dismissed_tasks_file_path() -> Result<PathBuf, String> {
    Ok(crate::modules::account::get_data_dir()?.join(DISMISSED_TASKS_FILE))
}

pub fn load_dismissed_task_ids() -> HashSet<String> {
    let path = match dismissed_tasks_file_path() {
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

pub fn save_dismissed_task_ids(ids: &HashSet<String>) -> Result<(), String> {
    let path = dismissed_tasks_file_path()?;
    let content = serde_json::to_string_pretty(ids)
        .map_err(|e| format!("序列化已忽略任务失败: {}", e))?;
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
import os, sys, glob, json, sqlite3, time, tempfile, shutil

def parse_iso_time(s):
    if not s: return 0
    try:
        from datetime import datetime
        dt = datetime.fromisoformat(str(s).replace('Z', '+00:00'))
        return int(dt.timestamp())
    except:
        return 0

def unfinished_background_turn(steps):
    # DONE describes one step, not the whole agent turn. Tool results and
    # ephemeral snapshots still require a subsequent planner response.
    pending = False
    for step in steps:
        kind = step.get('type')
        if kind == 'USER_INPUT' or step.get('source') == 'USER':
            pending = False
        elif kind == 'PLANNER_RESPONSE':
            if step.get('tool_calls'):
                pending = True
            elif str(step.get('content') or '').strip():
                pending = False
    return pending

def restart_interrupted_background(row, parent, steps, switch_at, now):
    updated = parse_iso_time(row.get('last_modified_time'))
    if not switch_at or not (0 <= now - switch_at <= 7 * 86400):
        return False
    if not (switch_at - 120 <= updated <= switch_at + 120) or now - updated < 20:
        return False
    # Never revive an explicitly stopped agent or one still marked running.
    if row.get('killed') or row.get('not_fully_idle') or row.get('status') != 'CASCADE_RUN_STATUS_IDLE':
        return False
    if not parent or parent.get('killed') or parent.get('not_fully_idle') or parent.get('status') != 'CASCADE_RUN_STATUS_IDLE':
        return False
    # A newer parent turn may have handled the failure or finished the task.
    if parse_iso_time(parent.get('last_modified_time')) > updated:
        return False
    event_at = parse_iso_time(steps[-1].get('created_at')) if steps else 0
    if not (switch_at - 120 <= event_at <= switch_at + 120):
        return False
    return unfinished_background_turn(steps)

def check_task():
    home = os.path.expanduser(os.environ.get('ANTIGRAVITY_HOME', '~/.gemini'))
    results = []
    db_candidates = [
        os.path.join(home, 'antigravity-cli', 'conversation_summaries.db'),
        os.path.join(home, 'antigravity', 'conversation_summaries.db'),
        os.path.join(home, 'conversation_summaries.db')
    ]
    seen_ids = set()

    for db_path in db_candidates:
        if not os.path.exists(db_path):
            continue
        root_dir = os.path.dirname(db_path)
        try:
            # SQLite may need writable WAL sidecars even for a read-only query.
            # Use a private snapshot so SSH never modifies the application's DB.
            snapshot = tempfile.TemporaryDirectory(prefix='cockpit-ag-index-')
            snapshot_path = os.path.join(snapshot.name, 'index.db')
            shutil.copy2(db_path, snapshot_path)
            if os.path.exists(db_path + '-wal'):
                shutil.copy2(db_path + '-wal', snapshot_path + '-wal')
            conn = sqlite3.connect(snapshot_path)
            conn.row_factory = sqlite3.Row
            rows = [dict(r) for r in conn.execute(
                'SELECT * FROM conversation_summaries ORDER BY last_modified_time DESC')]
            by_id = {r['conversation_id']: r for r in rows}
            latest_children = {}
            for row in rows:
                parent_id = row.get('parent_conversation_id')
                if parent_id:
                    latest_children.setdefault(parent_id, row['conversation_id'])
            switch_at = int(os.environ.get('ANTIGRAVITY_SWITCH_AT', '0'))
            for r in rows:
                cid = r['conversation_id']
                title, preview = r.get('title'), r.get('preview')
                c_status, not_fully_idle, killed = r.get('status'), r.get('not_fully_idle'), r.get('killed')
                last_mod_str, ws_uris = r.get('last_modified_time'), r.get('workspace_uris')
                # Ignore old cached transcript errors from previous installations.
                if not cid or cid in seen_ids or parse_iso_time(last_mod_str) < int(time.time()) - 7 * 86400:
                    continue
                
                transcript_path = os.path.join(root_dir, 'brain', cid, '.system_generated', 'logs', 'transcript.jsonl')
                last_step = None
                steps = []
                if os.path.exists(transcript_path):
                    try:
                        with open(transcript_path, 'r', encoding='utf-8', errors='ignore') as f:
                            steps = [json.loads(l) for l in f if l.strip()]
                            if steps:
                                last_step = steps[-1]
                    except:
                        pass
                
                is_interrupted = False
                error_message = None
                is_quota_limited = False

                if last_step:
                    st = last_step.get('status')
                    err = last_step.get('error') or ''
                    content = str(last_step.get('content', ''))
                    
                    if st == 'ERROR' or err or (last_step.get('type') == 'SYSTEM_MESSAGE' and 'RESOURCE_EXHAUSTED' in content):
                        is_interrupted = True
                        error_message = err or content[:250]
                        combined = (str(err) + ' ' + content).lower()
                        if any(k in combined for k in ['quota', 'resource_exhausted', 'rate limit', 'rate_limit', 'usage limit', '429', 'capacity']):
                            is_quota_limited = True
                
                if not is_interrupted and (not_fully_idle or c_status == 'CASCADE_RUN_STATUS_RUNNING') and killed:
                    is_interrupted = True
                    error_message = 'Task was cancelled or interrupted during execution'
                
                parent_id = r.get('parent_conversation_id')
                parent = by_id.get(parent_id)
                restart_interrupted = (
                    latest_children.get(parent_id) == cid
                    and restart_interrupted_background(r, parent, steps, switch_at, int(time.time()))
                )
                if restart_interrupted:
                    error_message = 'Background agent ' + cid + ' stopped mid-turn during account restart'
                    # Resume the owning conversation so it can inspect/recover its worker.
                    cid = parent_id
                    title, preview, ws_uris = parent.get('title'), parent.get('preview'), parent.get('workspace_uris')
                if (is_interrupted and is_quota_limited) or restart_interrupted:
                    if cid in seen_ids:
                        continue
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
                        'background_agent_id': r['conversation_id'] if restart_interrupted else None,
                        'is_goal': False,
                        'title': display_title,
                        'objective': preview,
                        'cwd': cwd,
                        'model': None,
                        'status': 'blocked' if restart_interrupted else 'usage_limited',
                        'error_message': error_message,
                        'resets_at': None,
                        'reset_window_minutes': None,
                        'updated_at': parse_iso_time(last_mod_str),
                        'can_resume': False
                    })
            conn.close()
            snapshot.cleanup()
        except Exception as error:
            print('Antigravity index probe failed: ' + str(error), file=sys.stderr)
            raise

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

                            if last_step_status.as_deref() == Some("ERROR") || err_str.is_some()
                                || (val.get("type").and_then(|v| v.as_str()) == Some("SYSTEM_MESSAGE") && content_str.contains("RESOURCE_EXHAUSTED")) {
                                error_msg = err_str.map(str::to_string).or_else(|| {
                                    if !content_str.is_empty() {
                                        Some(content_str.chars().take(250).collect())
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
                if now_ts - ts < 20 && last_step_status.as_deref() != Some("ERROR") && !is_quota_limited {
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
                    background_agent_id: None,
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
    let switch_at = server.last_antigravity_sync.as_ref()
        .filter(|sync| sync.verified)
        .map(|sync| sync.synced_at)
        .unwrap_or(0);
    let script = format!(
        "import os\nos.environ['ANTIGRAVITY_SWITCH_AT'] = '{}'\n{}",
        switch_at, PYTHON_REMOTE_PROBE,
    );
    let output = ssh_server::run_ssh(
        server,
        15,
        &["python3", "-"],
        Some(ssh_server::remote_python_with_home(&script, "ANTIGRAVITY_HOME", &server.antigravity_home)),
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
        if task.status == "usage_limited" || task.background_agent_id.is_some() {
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
    let dismissed_ids = load_dismissed_task_ids();

    all_tasks.retain(|t| !dismissed_ids.contains(&t.id) && !dismissed_ids.contains(&t.thread_id));

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
            let combined = format!("{} {}", stdout, stderr);
            let is_conn_err = combined.contains("connection error")
                || combined.contains("connection reset")
                || combined.contains("read: connection reset by peer")
                || combined.contains("error reading server preface")
                || combined.contains("Unavailable")
                || combined.contains("ANTIGRAVITY_LS_ADDRESS is not set");
            if !is_conn_err && (combined.contains("trajectory not found") || combined.contains("conversationMetadata") || combined.contains("error ID:")) {
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

        return Err(format!("已唤醒 Antigravity 窗口并定位到会话，但未能通过 API 自动注入继续指令，请在会话中手动继续: {}", thread_id));
    }

    #[cfg(not(target_os = "macos"))]
    Err(format!("未找到可用的 Antigravity 恢复入口，请确保 Antigravity IDE 或 agy 运行中"))
}

const PYTHON_REMOTE_RESUME: &str = r#"
import os, sys, json, base64, shutil, subprocess, time, shlex
from pathlib import Path
args = json.loads(base64.b64decode(sys.argv[1]))
home = Path(os.environ.get('ANTIGRAVITY_HOME', str(Path.home() / '.gemini'))).expanduser()
cid, prompt = args['thread_id'], args['prompt']
def resume():
    agentapis = [home / folder / 'bin/agentapi' for folder in ('antigravity', 'antigravity-cli')]
    agentapi = next((str(p) for p in agentapis if p.is_file() and os.access(p, os.X_OK)), None)
    if agentapi:
        deadline = time.monotonic() + 90
        last_error = "desktop API is not ready"
        while time.monotonic() < deadline:
            ports = []
            csrf_by_pid = {}
            try:
                processes = subprocess.run(['ps', '-axww', '-o', 'pid=,command='], capture_output=True, text=True, errors='replace', timeout=5)
                for line in processes.stdout.splitlines():
                    if 'language_server' not in line: continue
                    try: command = shlex.split(line)
                    except ValueError: continue
                    for i, arg in enumerate(command):
                        if arg == '--csrf_token' and i + 1 < len(command):
                            csrf_by_pid[command[0]] = command[i + 1]
                        elif arg.startswith('--csrf_token='):
                            csrf_by_pid[command[0]] = arg.split('=', 1)[1]
                out = subprocess.run([shutil.which('lsof') or '/usr/sbin/lsof', '-iTCP', '-sTCP:LISTEN', '-n', '-P'], capture_output=True, text=True, errors='replace', timeout=5)
                for line in out.stdout.splitlines():
                    if 'language_' in line or 'language_server' in line:
                        for word in line.split():
                            if word.startswith('127.0.0.1:'):
                                columns = line.split()
                                ports.append((word.rsplit(':', 1)[1], csrf_by_pid.get(columns[1])))
            except Exception as error: last_error = type(error).__name__
            for port, csrf in dict.fromkeys(ports):
                if not port.isdigit(): continue
                env = dict(os.environ, ANTIGRAVITY_LS_ADDRESS='localhost:' + port)
                if csrf: env['ANTIGRAVITY_CSRF_TOKEN'] = csrf
                try:
                    probe = subprocess.run([agentapi, 'get-conversation-metadata', cid], env=env, capture_output=True, text=True, errors='replace', timeout=5)
                except subprocess.TimeoutExpired:
                    last_error = 'desktop API metadata probe timed out'
                    continue
                if probe.returncode:
                    last_error = "desktop API metadata probe failed"
                    continue
                out = subprocess.run([agentapi, 'send-message', cid, prompt], env=env, capture_output=True, text=True, timeout=10)
                if out.returncode:
                    return {'success': False, 'message': out.stderr.strip() or 'Antigravity API rejected recovery'}
                return {'success': True, 'message': 'Restored through running Antigravity API'}
            time.sleep(2)
        return {'success': False, 'message': 'Antigravity desktop API was not ready within 90 seconds: ' + last_error}
    candidates = [shutil.which('agy'), str(Path.home() / '.local/bin/agy'),
        '/opt/homebrew/bin/agy', '/usr/local/bin/agy',
        '/Applications/Antigravity.app/Contents/Resources/app/bin/agy']
    agy = next((p for p in candidates if p and os.path.isfile(p) and os.access(p, os.X_OK)), None)
    if not agy:
        return {'success': False, 'message': 'No executable agy or running Antigravity API found'}
    home.mkdir(parents=True, exist_ok=True)
    # Keep diagnostics instead of reporting success for an immediately failed process.
    log = home / ('auto_resume_' + ''.join(c for c in cid if c.isalnum() or c in '-_') + '.log')
    with log.open('a') as output:
        child = subprocess.Popen([agy, '--conversation', cid, '--dangerously-skip-permissions', '--print', prompt],
            stdin=subprocess.DEVNULL, stdout=output, stderr=output, start_new_session=True)
    time.sleep(1)
    code = child.poll()
    if code is not None and code != 0:
        return {'success': False, 'message': 'agy exited with code ' + str(code) + '; see ' + str(log)}
    return {'success': True, 'message': 'Started Antigravity recovery; log: ' + str(log)}
try:
    result = resume()
except Exception as error:
    result = {'success': False, 'message': str(error)}
print('__ANTIGRAVITY_RESUME_RESULT__' + json.dumps(result))
"#;

async fn resume_remote_task(
    server: &SshServer,
    thread_id: &str,
    prompt: &str,
) -> Result<ResumeTaskResult, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let encoded = STANDARD.encode(serde_json::json!({"thread_id": thread_id, "prompt": prompt}).to_string());
    let script = ssh_server::remote_python_with_home(PYTHON_REMOTE_RESUME, "ANTIGRAVITY_HOME", &server.antigravity_home);
    let output = ssh_server::run_ssh(server, 120, &["python3", "-", &encoded], Some(script)).await?;
    let json = output.lines().find_map(|line| line.strip_prefix("__ANTIGRAVITY_RESUME_RESULT__"))
        .ok_or_else(|| format!("Antigravity recovery did not return a result on {}", server.name))?;
    let result: ResumeTaskResult = serde_json::from_str(json).map_err(|err| err.to_string())?;
    if result.success { Ok(result) } else { Err(result.message) }
}

const RECOVERY_QUOTA_EXHAUSTED: &str = "Antigravity recovery quota exhausted";

fn validate_fresh_recovery_quota(
    quota: &crate::models::quota::QuotaData,
    has_fetch_error: bool,
) -> Result<(), String> {
    // A passed reset time is not evidence of usable quota; only the fresh
    // percentages can authorize a recovery launch.
    if quota.is_forbidden || quota.models.iter().any(|model| model.percentage <= 0) {
        return Err(RECOVERY_QUOTA_EXHAUSTED.to_string());
    }
    if has_fetch_error || quota.quota_summary_stale || quota.models.is_empty() {
        return Err("Cannot confirm fresh Antigravity quota; recovery deferred".to_string());
    }
    Ok(())
}

async fn confirm_current_recovery_quota() -> Result<(), String> {
    let mut account = crate::modules::account::get_current_account()?
        .ok_or_else(|| "No current Antigravity account; recovery deferred".to_string())?;
    let quota = crate::modules::account::fetch_quota_with_fresh_token(&mut account, true)
        .await
        .map_err(|error| format!("Antigravity quota refresh failed; recovery deferred: {error}"))?;
    // Persist exhaustion as well as availability so automatic switching evaluates
    // the response we just fetched, rather than the old cached percentages.
    crate::modules::account::update_account_quota(&account.id, quota.clone())?;
    if crate::modules::account::get_current_account_id()?.as_deref() != Some(account.id.as_str()) {
        return Err("Antigravity account changed during quota refresh; recovery deferred".to_string());
    }
    crate::modules::websocket::broadcast_data_changed("quota_refreshed");
    validate_fresh_recovery_quota(&quota, account.quota_error.is_some())
}

pub async fn resume_task(
    task: &InterruptedAntigravityTask,
    custom_prompt: Option<String>,
    trigger_type: &str,
) -> Result<ResumeTaskResult, String> {
    confirm_current_recovery_quota().await?;
    let prompt = custom_prompt.unwrap_or_else(|| {
        if let Some(agent_id) = &task.background_agent_id {
            format!(
                "An account restart interrupted background agent {agent_id} mid-turn. Inspect its latest progress and current state, then resume or replace it to finish the remaining work. Preserve completed work and verify the results of prior actions before retrying them. If the task is already finished, do not repeat it."
            )
        } else {
            "Continue where you left off and finish the task.".to_string()
        }
    });

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

        let current = crate::modules::account::get_current_account()?
            .ok_or_else(|| "No current Antigravity account".to_string())?;
        let bundle = ssh_server::build_antigravity_projection_bundle(&current)?;
        let synced = server.last_antigravity_sync.as_ref().map(|status|
            status.verified && status.account_id == current.id && status.bundle_hash == bundle.bundle_hash
        ).unwrap_or(false);
        if !synced {
            let sync = ssh_server::sync_current_antigravity_account_to_server(Some(server_id.to_string())).await?;
            if !sync.verified {
                return Err(sync.error.unwrap_or_else(|| "Remote Antigravity auth sync failed".to_string()));
            }
        }
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
        if task.is_excluded {
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
                if err == RECOVERY_QUOTA_EXHAUSTED {
                    // Do not consume the recovery cooldown before switching;
                    // the switch callback needs to recover this same task.
                    resumed_tasks_cache().lock().unwrap_or_else(|e| e.into_inner()).remove(&task.id);
                    logger::log_info("[AntigravityAutoResume] Fresh quota is exhausted; switching account before recovery");
                    match crate::modules::account::run_auto_switch_if_needed().await {
                        Ok(Some(_)) => {},
                        Ok(None) => logger::log_info("[AntigravityAutoResume] No eligible automatic switch; recovery deferred"),
                        Err(error) => logger::log_warn(&format!("[AntigravityAutoResume] Account switch failed; recovery deferred: {error}")),
                    }
                    return;
                }
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

pub async fn resume_checked_tasks_on_account_switch(app: &AppHandle) {
    let cfg = config::get_user_config();
    if !cfg.antigravity_auto_resume_interrupted_tasks_enabled {
        logger::log_info("[AntigravityAutoResume] 切号后检查：自动恢复功能未开启，跳过");
        return;
    }

    logger::log_info("[AntigravityAutoResume] Checking interrupted tasks after account switch; each recovery requires fresh quota");
    let tasks = detect_all_interrupted_tasks().await;
    let now_ts = Utc::now().timestamp();

    for task in tasks {
        if task.is_excluded {
            continue;
        }

        {
            let mut cache = resumed_tasks_cache().lock().unwrap_or_else(|e| e.into_inner());
            cache.insert(task.id.clone(), now_ts);
        }

        logger::log_info(&format!(
            "[AntigravityAutoResume] 切号后立即恢复任务: id={}, title={}",
            task.id, task.title
        ));

        let res = resume_task(&task, None, "auto").await;
        match res {
            Ok(result) => {
                logger::log_info(&format!(
                    "[AntigravityAutoResume] 切号后自动恢复成功: {} - {}",
                    task.id, result.message
                ));

                let title = "Antigravity 任务已在切号后自动恢复";
                let display_title = if task.title.len() > 50 {
                    format!("{}...", &task.title[..47])
                } else {
                    task.title.clone()
                };
                let body = format!("账号切换后已立即恢复执行：{}", display_title);

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
                resumed_tasks_cache().lock().unwrap_or_else(|e| e.into_inner()).remove(&task.id);
                logger::log_warn(&format!(
                    "[AntigravityAutoResume] 切号后自动恢复任务失败: {} - {}",
                    task.id, err
                ));
                if err == RECOVERY_QUOTA_EXHAUSTED {
                    return;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_resume_regression_fresh_quota_rejects_exhaustion_even_after_reset() {
        let mut quota = crate::models::quota::QuotaData::new();
        quota.add_model("model".to_string(), None, 0, "2020-01-01T00:00:00Z".to_string());
        assert_eq!(validate_fresh_recovery_quota(&quota, false).unwrap_err(), RECOVERY_QUOTA_EXHAUSTED);
        quota.models[0].percentage = 80;
        assert!(validate_fresh_recovery_quota(&quota, false).is_ok());
        quota.is_forbidden = true;
        assert_eq!(validate_fresh_recovery_quota(&quota, false).unwrap_err(), RECOVERY_QUOTA_EXHAUSTED);
    }

    #[test]
    fn auto_resume_regression_fresh_quota_requires_successful_complete_response() {
        let mut quota = crate::models::quota::QuotaData::new();
        assert!(validate_fresh_recovery_quota(&quota, false).is_err());
        quota.add_model("model".to_string(), None, 100, String::new());
        assert!(validate_fresh_recovery_quota(&quota, true).is_err());
        quota.quota_summary_stale = true;
        assert!(validate_fresh_recovery_quota(&quota, false).is_err());
        quota.quota_summary_stale = false;
        assert!(validate_fresh_recovery_quota(&quota, false).is_ok());
    }

    #[test]
    fn auto_resume_regression_fresh_quota_blocks_partially_exhausted_account() {
        let mut quota = crate::models::quota::QuotaData::new();
        quota.add_model("available".to_string(), None, 100, String::new());
        quota.add_model("exhausted".to_string(), None, 0, String::new());
        assert_eq!(validate_fresh_recovery_quota(&quota, false).unwrap_err(), RECOVERY_QUOTA_EXHAUSTED);
    }

    #[test]
    fn auto_resume_regression_remote_probe_ignores_stale_quota_errors() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!("cockpit-ag-probe-{}", uuid::Uuid::new_v4()));
        let db_home = root.join("antigravity");
        std::fs::create_dir_all(&db_home).unwrap();
        let db = rusqlite::Connection::open(db_home.join("conversation_summaries.db")).unwrap();
        db.execute_batch("CREATE TABLE conversation_summaries (conversation_id TEXT,title TEXT,preview TEXT,status TEXT,not_fully_idle INTEGER,killed INTEGER,last_modified_time TEXT,workspace_uris TEXT)").unwrap();
        for (id, age) in [("stale", 20), ("current", 0), ("system", 0)] {
            let timestamp = (Utc::now() - chrono::Duration::days(age)).to_rfc3339();
            db.execute("INSERT INTO conversation_summaries VALUES (?, ?, '', 'CASCADE_RUN_STATUS_IDLE', 0, 1, ?, NULL)", rusqlite::params![id, id, timestamp]).unwrap();
            let logs = db_home.join("brain").join(id).join(".system_generated/logs");
            std::fs::create_dir_all(&logs).unwrap();
            std::fs::write(logs.join("transcript.jsonl"), if id == "system" { r#"{"status":"DONE","type":"SYSTEM_MESSAGE","content":"Subagent stopped: RESOURCE_EXHAUSTED (code 429)"}"# } else { r#"{"status":"ERROR","error":"RESOURCE_EXHAUSTED 429"}"# }).unwrap();
        }
        let mut child = std::process::Command::new("python3").arg("-").env("ANTIGRAVITY_HOME", &root)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(PYTHON_REMOTE_PROBE.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let text = String::from_utf8_lossy(&output.stdout);
        let json = text.split("__ANTIGRAVITY_PROBE_JSON_START__").nth(1).unwrap().split("__ANTIGRAVITY_PROBE_JSON_END__").next().unwrap();
        let tasks: Vec<InterruptedAntigravityTask> = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().any(|t| t.thread_id == "current"));
        assert!(tasks.iter().any(|t| t.thread_id == "system"));
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn auto_resume_regression_background_restart_requires_unfinished_latest_worker() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!("cockpit-ag-background-{}", uuid::Uuid::new_v4()));
        let db_home = root.join("antigravity");
        std::fs::create_dir_all(&db_home).unwrap();
        let db = rusqlite::Connection::open(db_home.join("conversation_summaries.db")).unwrap();
        db.execute_batch("CREATE TABLE conversation_summaries (conversation_id TEXT,title TEXT,preview TEXT,status TEXT,not_fully_idle INTEGER,killed INTEGER,last_modified_time TEXT,workspace_uris TEXT,parent_conversation_id TEXT)").unwrap();
        let now = Utc::now().timestamp();
        let switch_at = now - 60;
        let timestamp = |ts| DateTime::from_timestamp(ts, 0).unwrap().to_rfc3339();
        for case in ["interrupted", "finished", "stopped", "running", "parent_finished", "old", "replaced", "fresh", "old_transcript"] {
            let parent = format!("parent-{case}");
            let child = format!("worker-{case}");
            let child_at = match case { "old" => now - 1000, "fresh" => now - 5, _ => switch_at + 2 };
            let parent_at = if case == "parent_finished" { child_at + 1 } else { child_at - 4 };
            db.execute("INSERT INTO conversation_summaries VALUES (?, ?, '', 'CASCADE_RUN_STATUS_IDLE', 0, 0, ?, NULL, '')",
                rusqlite::params![parent, format!("Review {case}"), timestamp(parent_at)]).unwrap();
            db.execute("INSERT INTO conversation_summaries VALUES (?, '', '', ?, 0, ?, ?, NULL, ?)",
                rusqlite::params![child, if case == "running" { "CASCADE_RUN_STATUS_RUNNING" } else { "CASCADE_RUN_STATUS_IDLE" }, i32::from(case == "stopped"), timestamp(child_at), parent]).unwrap();
            let event_at = if case == "old_transcript" { now - 1000 } else { child_at };
            let mut steps = vec![
                serde_json::json!({"type":"PLANNER_RESPONSE", "status":"DONE", "created_at":timestamp(event_at - 2), "tool_calls":[{"name":"take_snapshot"}]}),
                serde_json::json!({"type":"GENERIC", "status":"DONE", "created_at":timestamp(event_at - 1), "content":"Tool completed successfully"}),
            ];
            if case == "finished" {
                steps.push(serde_json::json!({"type":"PLANNER_RESPONSE", "status":"DONE", "created_at":timestamp(event_at), "content":"All work finished"}));
            }
            steps.push(serde_json::json!({"type":"EPHEMERAL_MESSAGE", "status":"DONE", "created_at":timestamp(event_at)}));
            let logs = db_home.join("brain").join(&child).join(".system_generated/logs");
            std::fs::create_dir_all(&logs).unwrap();
            std::fs::write(logs.join("transcript.jsonl"), steps.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("\n")).unwrap();
            if case == "replaced" {
                db.execute("INSERT INTO conversation_summaries VALUES ('new-worker', '', '', 'CASCADE_RUN_STATUS_IDLE', 0, 0, ?, NULL, ?)",
                    rusqlite::params![timestamp(child_at + 1), parent]).unwrap();
            }
        }
        for verified_switch in [0, switch_at] {
            let mut child = std::process::Command::new("python3").arg("-")
                .env("ANTIGRAVITY_HOME", &root)
                .env("ANTIGRAVITY_SWITCH_AT", verified_switch.to_string())
                .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
            child.stdin.take().unwrap().write_all(PYTHON_REMOTE_PROBE.as_bytes()).unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            let text = String::from_utf8_lossy(&output.stdout);
            let json = text.split("__ANTIGRAVITY_PROBE_JSON_START__").nth(1).unwrap().split("__ANTIGRAVITY_PROBE_JSON_END__").next().unwrap();
            let tasks: Vec<InterruptedAntigravityTask> = serde_json::from_str(json.trim()).unwrap();
            if verified_switch == 0 {
                assert!(tasks.is_empty());
            } else {
                assert_eq!(tasks.len(), 1, "Unexpected recoveries: {tasks:?}");
                assert_eq!(tasks[0].thread_id, "parent-interrupted");
                assert_eq!(tasks[0].background_agent_id.as_deref(), Some("worker-interrupted"));
                assert_eq!(tasks[0].title, "Review interrupted");
                assert_eq!(tasks[0].status, "blocked");
            }
        }
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn auto_resume_regression_macos_custom_home_and_cli_failure() {
        use std::io::Write;
        use base64::{engine::general_purpose::STANDARD, Engine};
        let dir = std::env::temp_dir().join(format!("cockpit-ag-recovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("antigravity/bin")).unwrap();
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let agentapi = dir.join("antigravity/bin/agentapi");
        std::fs::write(&agentapi, "#!/bin/sh\n[ \"$ANTIGRAVITY_LS_ADDRESS\" = localhost:54321 ] || exit 9\n[ \"$ANTIGRAVITY_CSRF_TOKEN\" = fixture-secret ] || exit 8\nexit 0\n").unwrap();
        std::fs::write(dir.join("bin/lsof"), "#!/bin/sh\nif [ ! -f \"$ANTIGRAVITY_HOME/probed\" ]; then touch \"$ANTIGRAVITY_HOME/probed\"; exit 0; fi\necho 'language_server 1 user TCP 127.0.0.1:54321 LISTEN'\n").unwrap();
        std::fs::write(dir.join("bin/agy"), "#!/bin/sh\nexit 5\n").unwrap();
        std::fs::write(dir.join("bin/ps"), "#!/bin/sh\necho '1 /Applications/Antigravity.app/Contents/Resources/bin/language_server --csrf_token fixture-secret'\n").unwrap();
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            for path in [&agentapi, &dir.join("bin/lsof"), &dir.join("bin/agy"), &dir.join("bin/ps")] {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let encoded = STANDARD.encode(serde_json::json!({"thread_id":"test-thread", "prompt":"Continue 'quoted' task"}).to_string());
        for expect_success in [true, false] {
            if !expect_success { std::fs::remove_file(&agentapi).unwrap(); }
            let mut child = std::process::Command::new("python3").args(["-", &encoded])
                .env("ANTIGRAVITY_HOME", &dir).env("PATH", format!("{}:/usr/bin:/bin", dir.join("bin").display()))
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            child.stdin.take().unwrap().write_all(PYTHON_REMOTE_RESUME.as_bytes()).unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let text = String::from_utf8_lossy(&output.stdout);
            let json = text.lines().find_map(|line| line.strip_prefix("__ANTIGRAVITY_RESUME_RESULT__")).unwrap();
            let result: ResumeTaskResult = serde_json::from_str(json).unwrap();
            assert_eq!(result.success, expect_success, "{}", result.message);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

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

    #[test]
    fn test_task_delete_dismiss_logic() {
        let test_task_id = "test:antigravity:delete:999";
        let _ = delete_interrupted_task(test_task_id);
        assert!(load_dismissed_task_ids().contains(test_task_id));
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

    #[test]
    fn test_find_local_ls_port() {
        let port = find_local_ls_port();
        println!("find_local_ls_port result: {:?}", port);
        if let Some(p) = port {
            assert!(p > 1024);
        }
    }

    #[test]
    fn test_ensure_antigravity_launch_path() {
        let result = crate::modules::process::ensure_antigravity_launch_path_configured();
        println!("ensure_antigravity_launch_path_configured: {:?}", result);
        assert!(result.is_ok());
    }
}
