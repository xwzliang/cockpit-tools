import { invoke } from '@tauri-apps/api/core';

export interface InterruptedCodexTask {
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

export async function detectInterruptedCodexTasks(): Promise<InterruptedCodexTask[]> {
  return await invoke<InterruptedCodexTask[]>('codex_detect_interrupted_tasks');
}

export async function resumeInterruptedCodexTask(
  task: InterruptedCodexTask,
  customPrompt?: string,
): Promise<ResumeTaskResult> {
  return await invoke<ResumeTaskResult>('codex_resume_interrupted_task', {
    task,
    customPrompt: customPrompt || null,
  });
}

export async function getAutoResumedTasks(): Promise<ResumedTaskRecord[]> {
  return await invoke<ResumedTaskRecord[]>('codex_get_auto_resumed_tasks');
}

export async function clearAutoResumedTasks(): Promise<void> {
  return await invoke<void>('codex_clear_auto_resumed_tasks');
}

export async function getCodexExcludedTasks(): Promise<string[]> {
  return await invoke<string[]>('codex_get_auto_resume_excluded_tasks');
}

export async function setCodexTaskExcluded(taskId: string, excluded: boolean): Promise<void> {
  return await invoke<void>('codex_set_auto_resume_task_excluded', { taskId, excluded });
}

