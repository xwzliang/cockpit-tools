import { invoke } from '@tauri-apps/api/core';

export interface InterruptedAntigravityTask {
  id: string;
  source: 'local' | 'ssh';
  server_id?: string | null;
  server_name?: string | null;
  thread_id: string;
  goal_id?: string | null;
  is_goal: boolean;
  title: string;
  objective?: string | null;
  cwd?: string | null;
  model?: string | null;
  status: string;
  error_message?: string | null;
  resets_at?: number | null;
  reset_window_minutes?: number | null;
  updated_at: number;
  can_resume: boolean;
  is_excluded?: boolean;
}

export interface ResumeTaskResult {
  success: boolean;
  message: string;
}

export interface ResumedTaskRecord {
  id: string;
  source: 'local' | 'ssh';
  server_id?: string | null;
  server_name?: string | null;
  thread_id: string;
  goal_id?: string | null;
  is_goal: boolean;
  title: string;
  cwd?: string | null;
  model?: string | null;
  resumed_at: number;
  trigger_type: 'auto' | 'manual';
  success: boolean;
  message: string;
}

export async function detectInterruptedAntigravityTasks(): Promise<InterruptedAntigravityTask[]> {
  return await invoke<InterruptedAntigravityTask[]>('antigravity_detect_interrupted_tasks');
}

export async function resumeInterruptedAntigravityTask(
  task: InterruptedAntigravityTask,
  customPrompt?: string,
): Promise<ResumeTaskResult> {
  return await invoke<ResumeTaskResult>('antigravity_resume_interrupted_task', {
    task,
    customPrompt: customPrompt || null,
  });
}

export async function getAntigravityAutoResumedTasks(): Promise<ResumedTaskRecord[]> {
  return await invoke<ResumedTaskRecord[]>('antigravity_get_auto_resumed_tasks');
}

export async function clearAntigravityAutoResumedTasks(): Promise<void> {
  return await invoke<void>('antigravity_clear_auto_resumed_tasks');
}

export async function getAntigravityExcludedTasks(): Promise<string[]> {
  return await invoke<string[]>('antigravity_get_auto_resume_excluded_tasks');
}

export async function setAntigravityTaskExcluded(taskId: string, excluded: boolean): Promise<void> {
  return await invoke<void>('antigravity_set_auto_resume_task_excluded', { taskId, excluded });
}
