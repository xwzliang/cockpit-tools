export type TaskStatus =
  | 'pending'
  | 'waiting_interrupted'
  | 'running'
  | 'completed'
  | 'failed'
  | 'cancelled';

export interface CodexQueuedTask {
  id: string;
  title: string;
  prompt: string;
  source: 'local' | 'ssh';
  server_id?: string;
  server_name?: string;
  cwd: string;
  thread_id?: string;
  thread_title?: string;
  model?: string;
  status: TaskStatus;
  error_message?: string;
  output_preview?: string;
  assigned_thread_id?: string;
  pid?: number;
  created_at: number;
  started_at?: number;
  completed_at?: number;
}

export interface CodexQueueConfig {
  concurrency_mode: 'sequential' | 'concurrent';
  max_concurrent_tasks: number;
  auto_dispatch: boolean;
}

export interface CodexQueueState {
  tasks: CodexQueuedTask[];
  config: CodexQueueConfig;
}

export interface CodexProjectItem {
  cwd: string;
  name: string;
  last_active: number;
  thread_count: number;
}

export interface CodexThreadSummary {
  id: string;
  title: string;
  model?: string;
  cwd?: string;
  updated_at: number;
  has_interrupted_task: boolean;
}

export interface CreateQueuedTaskRequest {
  title?: string;
  prompt: string;
  source: 'local' | 'ssh';
  server_id?: string;
  server_name?: string;
  cwd: string;
  thread_id?: string;
  thread_title?: string;
  model?: string;
}
