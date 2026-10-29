use crate::modules::codex_task_queue::{
    self, CodexProjectItem, CodexQueueConfig, CodexQueueState, CodexQueuedTask,
    CodexThreadSummary, CreateQueuedTaskRequest,
};

#[tauri::command]
pub fn codex_queue_get_state() -> Result<CodexQueueState, String> {
    codex_task_queue::get_queue_state()
}

#[tauri::command]
pub async fn codex_queue_add_task(
    app: AppHandle,
    req: CreateQueuedTaskRequest,
) -> Result<CodexQueuedTask, String> {
    let task = codex_task_queue::add_task(req)?;
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = codex_task_queue::run_queue_dispatcher_step(Some(&app_clone)).await;
    });
    Ok(task)
}

#[tauri::command]
pub async fn codex_queue_update_task(
    app: AppHandle,
    task: CodexQueuedTask,
) -> Result<CodexQueuedTask, String> {
    let updated = codex_task_queue::edit_task(task).await?;
    let _ = app.emit("codex://task-queue-updated", ());
    // The periodic dispatcher will pick up the edited schedule. Do not start a
    // second dispatcher here or retain the edit guard while dispatching.
    Ok(updated)
}

#[tauri::command]
pub fn codex_queue_delete_task(task_id: String) -> Result<(), String> {
    codex_task_queue::delete_task(&task_id)
}

#[tauri::command]
pub fn codex_queue_reorder(task_ids: Vec<String>) -> Result<(), String> {
    codex_task_queue::reorder_queue(task_ids)
}

#[tauri::command]
pub async fn codex_queue_retry_task(app: AppHandle, task_id: String) -> Result<(), String> {
    codex_task_queue::retry_task(&task_id)?;
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = codex_task_queue::run_queue_dispatcher_step(Some(&app_clone)).await;
    });
    Ok(())
}

#[tauri::command]
pub fn codex_queue_clear_completed() -> Result<(), String> {
    codex_task_queue::clear_completed_tasks()
}

#[tauri::command]
pub async fn codex_queue_update_config(
    app: AppHandle,
    config: CodexQueueConfig,
) -> Result<(), String> {
    codex_task_queue::update_config(config)?;
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = codex_task_queue::run_queue_dispatcher_step(Some(&app_clone)).await;
    });
    Ok(())
}

#[tauri::command]
pub async fn codex_queue_list_projects(
    source: String,
    server_id: Option<String>,
) -> Result<Vec<CodexProjectItem>, String> {
    codex_task_queue::list_projects(&source, server_id.as_deref()).await
}

#[tauri::command]
pub async fn codex_queue_list_threads(
    source: String,
    server_id: Option<String>,
    cwd: Option<String>,
) -> Result<Vec<CodexThreadSummary>, String> {
    codex_task_queue::list_threads(&source, server_id.as_deref(), cwd.as_deref()).await
}

#[tauri::command]
pub async fn codex_queue_trigger_dispatch(app: AppHandle) -> Result<(), String> {
    codex_task_queue::run_queue_dispatcher_step(Some(&app)).await
}
