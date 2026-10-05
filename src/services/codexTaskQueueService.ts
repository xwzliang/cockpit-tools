import { invoke } from '@tauri-apps/api/core';
import type {
  CodexProjectItem,
  CodexQueueConfig,
  CodexQueueState,
  CodexQueuedTask,
  CodexThreadSummary,
  CreateQueuedTaskRequest,
} from '../types/codexTaskQueue';

export async function getCodexQueueState(): Promise<CodexQueueState> {
  return invoke<CodexQueueState>('codex_queue_get_state');
}

export async function addCodexQueuedTask(
  req: CreateQueuedTaskRequest
): Promise<CodexQueuedTask> {
  return invoke<CodexQueuedTask>('codex_queue_add_task', { req });
}

export async function updateCodexQueuedTask(
  task: CodexQueuedTask
): Promise<CodexQueuedTask> {
  return invoke<CodexQueuedTask>('codex_queue_update_task', { task });
}

export async function deleteCodexQueuedTask(taskId: string): Promise<void> {
  return invoke<void>('codex_queue_delete_task', { taskId });
}

export async function reorderCodexQueue(taskIds: string[]): Promise<void> {
  return invoke<void>('codex_queue_reorder', { taskIds });
}

export async function retryCodexQueuedTask(taskId: string): Promise<void> {
  return invoke<void>('codex_queue_retry_task', { taskId });
}

export async function clearCodexCompletedTasks(): Promise<void> {
  return invoke<void>('codex_queue_clear_completed');
}

export async function updateCodexQueueConfig(
  config: CodexQueueConfig
): Promise<void> {
  return invoke<void>('codex_queue_update_config', { config });
}

export async function listCodexProjects(
  source: 'local' | 'ssh',
  serverId?: string
): Promise<CodexProjectItem[]> {
  return invoke<CodexProjectItem[]>('codex_queue_list_projects', {
    source,
    serverId: serverId || null,
  });
}

export async function listCodexThreads(
  source: 'local' | 'ssh',
  serverId?: string,
  cwd?: string
): Promise<CodexThreadSummary[]> {
  return invoke<CodexThreadSummary[]>('codex_queue_list_threads', {
    source,
    serverId: serverId || null,
    cwd: cwd || null,
  });
}

export async function triggerCodexQueueDispatch(): Promise<void> {
  return invoke<void>('codex_queue_trigger_dispatch');
}
