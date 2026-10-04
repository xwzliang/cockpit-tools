#[tauri::command]
pub async fn codex_detect_interrupted_tasks(
) -> Result<Vec<crate::modules::codex_auto_resume::InterruptedCodexTask>, String> {
    Ok(crate::modules::codex_auto_resume::detect_all_interrupted_tasks().await)
}

#[tauri::command]
pub async fn codex_resume_interrupted_task(
    task: crate::modules::codex_auto_resume::InterruptedCodexTask,
    custom_prompt: Option<String>,
) -> Result<crate::modules::codex_auto_resume::ResumeTaskResult, String> {
    crate::modules::codex_auto_resume::resume_task(&task, custom_prompt, "manual").await
}

#[tauri::command]
pub fn codex_get_auto_resumed_tasks(
) -> Result<Vec<crate::modules::codex_auto_resume::ResumedTaskRecord>, String> {
    Ok(crate::modules::codex_auto_resume::load_resumed_task_records())
}

#[tauri::command]
pub fn codex_clear_auto_resumed_tasks() -> Result<(), String> {
    crate::modules::codex_auto_resume::clear_resumed_task_records()
}

#[tauri::command]
pub fn codex_get_auto_resume_excluded_tasks() -> Result<Vec<String>, String> {
    Ok(crate::modules::codex_auto_resume::load_excluded_task_ids()
        .into_iter()
        .collect())
}

#[tauri::command]
pub fn codex_set_auto_resume_task_excluded(
    task_id: String,
    excluded: bool,
) -> Result<(), String> {
    crate::modules::codex_auto_resume::set_task_excluded(&task_id, excluded)
}

