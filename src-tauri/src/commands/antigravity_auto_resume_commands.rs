#[tauri::command]
pub async fn antigravity_detect_interrupted_tasks(
) -> Result<Vec<crate::modules::antigravity_auto_resume::InterruptedAntigravityTask>, String> {
    Ok(crate::modules::antigravity_auto_resume::detect_all_interrupted_tasks().await)
}

#[tauri::command]
pub async fn antigravity_resume_interrupted_task(
    task: crate::modules::antigravity_auto_resume::InterruptedAntigravityTask,
    custom_prompt: Option<String>,
) -> Result<crate::modules::antigravity_auto_resume::ResumeTaskResult, String> {
    crate::modules::antigravity_auto_resume::resume_task(&task, custom_prompt, "manual").await
}

#[tauri::command]
pub fn antigravity_get_auto_resumed_tasks(
) -> Result<Vec<crate::modules::antigravity_auto_resume::ResumedTaskRecord>, String> {
    Ok(crate::modules::antigravity_auto_resume::load_resumed_task_records())
}

#[tauri::command]
pub fn antigravity_clear_auto_resumed_tasks() -> Result<(), String> {
    crate::modules::antigravity_auto_resume::clear_resumed_task_records()
}

#[tauri::command]
pub fn antigravity_get_auto_resume_excluded_tasks() -> Result<Vec<String>, String> {
    Ok(crate::modules::antigravity_auto_resume::load_excluded_task_ids()
        .into_iter()
        .collect())
}

#[tauri::command]
pub fn antigravity_set_auto_resume_task_excluded(
    task_id: String,
    excluded: bool,
) -> Result<(), String> {
    crate::modules::antigravity_auto_resume::set_task_excluded(&task_id, excluded)
}
