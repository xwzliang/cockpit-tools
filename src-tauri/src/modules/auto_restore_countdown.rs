//! Automatic restores have a cancellable, in-memory grace period. Never persist
//! a deadline: after an app restart every restore must receive a new full minute.
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::Emitter;

#[derive(Clone, Serialize)]
pub struct PendingAutoRestore {
    pub provider: String,
    pub task_id: String,
    pub title: String,
    pub deadline: i64,
}
struct Entry { pending: PendingAutoRestore, token: u64 }
static PENDING: OnceLock<Mutex<HashMap<(String, String), Entry>>> = OnceLock::new();
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
fn state() -> &'static Mutex<HashMap<(String, String), Entry>> {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}
#[tauri::command]
pub fn get_pending_auto_restores() -> Vec<PendingAutoRestore> {
    state().lock().unwrap_or_else(|e| e.into_inner()).values().map(|entry| entry.pending.clone()).collect()
}
fn emit_changed() {
    if let Some(app) = crate::get_app_handle() {
        let _ = app.emit("auto-restore://pending-changed", get_pending_auto_restores());
    }
}
pub fn cancel(provider: &str, task_id: &str) {
    state().lock().unwrap_or_else(|e| e.into_inner()).retain(|(p, id), _|
        p != provider || !(id == task_id || id.rsplit(':').next() == Some(task_id)));
    emit_changed();
}
pub fn cancel_provider(provider: &str) {
    state().lock().unwrap_or_else(|e| e.into_inner()).retain(|(p, _), _| p != provider);
    emit_changed();
}
pub fn token(provider: &str, task_id: &str) -> Option<u64> {
    state().lock().unwrap_or_else(|e| e.into_inner()).get(&(provider.into(), task_id.into())).map(|entry| entry.token)
}
pub struct CountdownGuard { key: (String, String), token: u64 }
impl CountdownGuard {
    pub fn valid(&self) -> bool {
        state().lock().unwrap_or_else(|e| e.into_inner()).get(&self.key)
            .map(|entry| entry.token == self.token).unwrap_or(false)
    }
}
impl Drop for CountdownGuard {
    fn drop(&mut self) {
        let mut pending = state().lock().unwrap_or_else(|e| e.into_inner());
        if pending.get(&self.key).map(|entry| entry.token) == Some(self.token) {
            pending.remove(&self.key);
        }
        drop(pending);
        emit_changed();
    }
}
pub async fn wait<F>(provider: &str, task_id: &str, title: &str, eligible: F) -> Option<CountdownGuard>
where F: Fn() -> bool {
    if !eligible() { return None; }
    let key = (provider.to_string(), task_id.to_string());
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    {
        let mut pending = state().lock().unwrap_or_else(|e| e.into_inner());
        if pending.contains_key(&key) { return None; }
        pending.insert(key.clone(), Entry { token, pending: PendingAutoRestore {
            provider: provider.into(), task_id: task_id.into(), title: title.into(),
            deadline: chrono::Utc::now().timestamp() + 60,
        }});
    }
    let guard = CountdownGuard { key, token };
    emit_changed();
    // Monotonic time guarantees a full minute even if the wall clock changes.
    let end = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if !guard.valid() || !eligible() { return None; }
        if tokio::time::Instant::now() >= end { return Some(guard); }
        tokio::time::sleep_until(std::cmp::min(end,
            tokio::time::Instant::now() + std::time::Duration::from_millis(250))).await;
    }
}
