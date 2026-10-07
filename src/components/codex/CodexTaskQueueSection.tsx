import { useEffect, useState, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { listen } from '@tauri-apps/api/event';
import {
  ListOrdered,
  Plus,
  Play,
  Pause,
  RefreshCw,
  Trash2,
  RotateCcw,
  ChevronDown,
  ChevronUp,
  ArrowUp,
  ArrowDown,
  Laptop,
  Server,
  Folder,
  MessageSquare,
  Sparkles,
  AlertTriangle,
  CheckCircle,
  Clock,
  Loader2,
  XCircle,
  SlidersHorizontal,
} from 'lucide-react';
import {
  getCodexQueueState,
  deleteCodexQueuedTask,
  reorderCodexQueue,
  retryCodexQueuedTask,
  clearCodexCompletedTasks,
  updateCodexQueueConfig,
  triggerCodexQueueDispatch,
} from '../../services/codexTaskQueueService';
import type {
  CodexQueueConfig,
  CodexQueuedTask,
  TaskStatus,
} from '../../types/codexTaskQueue';
import { CodexAddTaskModal } from './CodexAddTaskModal';
import './CodexTaskQueueSection.css';

interface CodexTaskQueueSectionProps {
  variant?: 'overview' | 'settings';
}

export function CodexTaskQueueSection({
  variant = 'overview',
}: CodexTaskQueueSectionProps) {
  const { t } = useTranslation();

  const [tasks, setTasks] = useState<CodexQueuedTask[]>([]);
  const [config, setConfig] = useState<CodexQueueConfig>({
    concurrency_mode: 'sequential',
    max_concurrent_tasks: 2,
    auto_dispatch: true,
  });
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isCollapsed, setIsCollapsed] = useState<boolean>(() => {
    const saved = localStorage.getItem('codex_task_queue_collapsed');
    return saved === 'true';
  });
  const [isAddModalOpen, setIsAddModalOpen] = useState(false);
  const [expandedTaskIds, setExpandedTaskIds] = useState<Set<string>>(new Set());
  const [activeTab, setActiveTab] = useState<'queue' | 'finished'>('queue');

  const loadData = useCallback(async (isSilent = false) => {
    if (!isSilent) {
      setLoading(true);
      setError(null);
    }
    try {
      const state = await getCodexQueueState();
      setTasks(state.tasks);
      setConfig(state.config);
    } catch (err) {
      if (!isSilent) {
        setError(String(err));
      }
    } finally {
      if (!isSilent) {
        setLoading(false);
      }
    }
  }, []);

  // Initial load
  useEffect(() => {
    loadData();
  }, [loadData]);

  // Listen to background updates
  useEffect(() => {
    const unlistenPromise = listen('codex://task-queue-updated', () => {
      loadData(true);
    });
    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, [loadData]);

  // Polling while tasks are running or pending
  useEffect(() => {
    const hasActiveTasks = tasks.some(
      (t) => t.status === 'running' || t.status === 'pending' || t.status === 'waiting_interrupted'
    );
    if (!hasActiveTasks) return;

    const interval = setInterval(() => {
      loadData(true);
    }, 4000);
    return () => clearInterval(interval);
  }, [tasks, loadData]);

  const toggleCollapse = () => {
    const next = !isCollapsed;
    setIsCollapsed(next);
    localStorage.setItem('codex_task_queue_collapsed', String(next));
  };

  const toggleExpandTask = (taskId: string) => {
    setExpandedTaskIds((prev) => {
      const next = new Set(prev);
      if (next.has(taskId)) {
        next.delete(taskId);
      } else {
        next.add(taskId);
      }
      return next;
    });
  };

  // Concurrency mode toggle
  const handleModeChange = async (mode: 'sequential' | 'concurrent') => {
    const nextConfig = { ...config, concurrency_mode: mode };
    setConfig(nextConfig);
    try {
      await updateCodexQueueConfig(nextConfig);
    } catch (err) {
      setError(String(err));
    }
  };

  // Max concurrent tasks change
  const handleMaxConcurrentChange = async (val: number) => {
    const clamped = Math.min(Math.max(val, 1), 10);
    const nextConfig = { ...config, max_concurrent_tasks: clamped };
    setConfig(nextConfig);
    try {
      await updateCodexQueueConfig(nextConfig);
    } catch (err) {
      setError(String(err));
    }
  };

  // Auto dispatch toggle
  const handleToggleAutoDispatch = async () => {
    const nextConfig = { ...config, auto_dispatch: !config.auto_dispatch };
    setConfig(nextConfig);
    try {
      await updateCodexQueueConfig(nextConfig);
    } catch (err) {
      setError(String(err));
    }
  };

  // Delete task
  const handleDeleteTask = async (taskId: string) => {
    try {
      await deleteCodexQueuedTask(taskId);
      setTasks((prev) => prev.filter((t) => t.id !== taskId));
    } catch (err) {
      setError(String(err));
    }
  };

  // Retry task
  const handleRetryTask = async (taskId: string) => {
    try {
      await retryCodexQueuedTask(taskId);
      setActiveTab('queue');
      await loadData(true);
    } catch (err) {
      setError(String(err));
    }
  };

  // Clear completed
  const handleClearCompleted = async () => {
    try {
      await clearCodexCompletedTasks();
      await loadData(true);
    } catch (err) {
      setError(String(err));
    }
  };

  // Move task up/down in active queue
  const handleMoveActiveTask = async (taskId: string, direction: 'up' | 'down') => {
    const activeIndex = activeTasks.findIndex((t) => t.id === taskId);
    if (activeIndex === -1) return;
    const targetActiveIndex = direction === 'up' ? activeIndex - 1 : activeIndex + 1;
    if (targetActiveIndex < 0 || targetActiveIndex >= activeTasks.length) return;

    const currentTask = activeTasks[activeIndex];
    const targetTask = activeTasks[targetActiveIndex];

    // Cannot move above a currently running task
    if (direction === 'up' && targetTask.status === 'running') return;

    const idxCurrent = tasks.findIndex((t) => t.id === currentTask.id);
    const idxTarget = tasks.findIndex((t) => t.id === targetTask.id);
    if (idxCurrent === -1 || idxTarget === -1) return;

    const newTasks = [...tasks];
    const temp = newTasks[idxCurrent];
    newTasks[idxCurrent] = newTasks[idxTarget];
    newTasks[idxTarget] = temp;
    setTasks(newTasks);

    try {
      await reorderCodexQueue(newTasks.map((t) => t.id));
    } catch (err) {
      setError(String(err));
      loadData(true);
    }
  };

  const activeTasks = tasks.filter(
    (t) => t.status === 'pending' || t.status === 'waiting_interrupted' || t.status === 'running'
  );
  const finishedTasks = tasks.filter(
    (t) => t.status === 'completed' || t.status === 'failed' || t.status === 'cancelled'
  );
  const displayedTasks = activeTab === 'queue' ? activeTasks : finishedTasks;

  const runningCount = tasks.filter((t) => t.status === 'running').length;
  const pendingCount = tasks.filter(
    (t) => t.status === 'pending' || t.status === 'waiting_interrupted'
  ).length;
  const completedCount = tasks.filter((t) => t.status === 'completed').length;

  const renderStatusBadge = (status: TaskStatus) => {
    switch (status) {
      case 'running':
        return (
          <span className="queue-status-badge running">
            <Loader2 className="spinning-icon" />
            <span>{t('codexQueue.statusRunning', '执行中')}</span>
          </span>
        );
      case 'waiting_interrupted':
        return (
          <span className="queue-status-badge waiting-interrupted" title={t('codexQueue.waitingInterruptedTitle', '会话存在中断任务，正优先恢复中断任务')}>
            <AlertTriangle className="badge-icon" />
            <span>{t('codexQueue.statusWaitingInterrupted', '等待中断任务完成')}</span>
          </span>
        );
      case 'pending':
        return (
          <span className="queue-status-badge pending">
            <Clock className="badge-icon" />
            <span>{t('codexQueue.statusPending', '排队中')}</span>
          </span>
        );
      case 'completed':
        return (
          <span className="queue-status-badge completed">
            <CheckCircle className="badge-icon" />
            <span>{t('codexQueue.statusCompleted', '已完成')}</span>
          </span>
        );
      case 'failed':
        return (
          <span className="queue-status-badge failed">
            <XCircle className="badge-icon" />
            <span>{t('codexQueue.statusFailed', '失败')}</span>
          </span>
        );
      case 'cancelled':
        return (
          <span className="queue-status-badge cancelled">
            <XCircle className="badge-icon" />
            <span>{t('codexQueue.statusCancelled', '已取消')}</span>
          </span>
        );
    }
  };

  const formatDuration = (task: CodexQueuedTask) => {
    if (!task.started_at) return null;
    const end = task.completed_at || Math.floor(Date.now() / 1000);
    const secs = Math.max(0, end - task.started_at);
    if (secs < 60) return `${secs}s`;
    const mins = Math.floor(secs / 60);
    return `${mins}m ${secs % 60}s`;
  };

  return (
    <div className={`codex-task-queue-section ${variant} ${isCollapsed ? 'collapsed' : ''}`}>
      {/* Header */}
      <div className="queue-section-header" onClick={toggleCollapse}>
        <div className="header-left">
          <div className="header-icon-box">
            <ListOrdered className="section-main-icon" />
          </div>
          <div className="header-title-group">
            <div className="title-row">
              <h3>{t('codexQueue.title', 'Codex 任务排队')}</h3>
              <div className="task-counts-chips">
                <span className="count-chip total" title="总任务数">
                  {tasks.length}
                </span>
                {runningCount > 0 && (
                  <span className="count-chip running" title="执行中">
                    <span className="pulsing-dot" />
                    {runningCount} 执行中
                  </span>
                )}
                {pendingCount > 0 && (
                  <span className="count-chip pending" title="等待中">
                    {pendingCount} 排队
                  </span>
                )}
                {completedCount > 0 && (
                  <span className="count-chip completed" title="已完成">
                    {completedCount} 完成
                  </span>
                )}
              </div>
            </div>
            <p className="section-subtitle">
              {t(
                'codexQueue.subtitle',
                '多任务排队执行 · 同会话严格串行 · 跨会话并发或串行 · 优先恢复中断任务'
              )}
            </p>
          </div>
        </div>

        <div className="header-actions" onClick={(e) => e.stopPropagation()}>
          {/* Concurrency Mode Controls */}
          <div className="concurrency-toolbar">
            <div className="mode-toggle-group">
              <button
                type="button"
                className={`mode-btn ${config.concurrency_mode === 'sequential' ? 'active' : ''}`}
                onClick={() => handleModeChange('sequential')}
                title={t('codexQueue.sequentialTip', '全局顺序串行执行：每次仅执行一个任务')}
              >
                <span>{t('codexQueue.sequential', '顺序执行')}</span>
              </button>
              <button
                type="button"
                className={`mode-btn ${config.concurrency_mode === 'concurrent' ? 'active' : ''}`}
                onClick={() => handleModeChange('concurrent')}
                title={t('codexQueue.concurrentTip', '跨会话/跨项目并发执行：同一会话仍保持串行')}
              >
                <span>{t('codexQueue.concurrent', '并发执行')}</span>
              </button>
            </div>

            {config.concurrency_mode === 'concurrent' && (
              <div className="max-concurrent-box" title={t('codexQueue.maxConcurrentTip', '最大并发任务数 (1-10)')}>
                <span className="max-label">{t('codexQueue.concurrencyLimit', '上限:')}</span>
                <select
                  className="max-select"
                  value={config.max_concurrent_tasks}
                  onChange={(e) => handleMaxConcurrentChange(Number(e.target.value))}
                >
                  {[1, 2, 3, 4, 5, 8, 10].map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </select>
              </div>
            )}

            <button
              type="button"
              className={`dispatch-toggle-btn ${config.auto_dispatch ? 'active' : 'paused'}`}
              onClick={handleToggleAutoDispatch}
              title={config.auto_dispatch ? '自动调度已开启' : '自动调度已暂停'}
            >
              {config.auto_dispatch ? (
                <>
                  <Play className="btn-icon" />
                  <span>{t('codexQueue.dispatching', '调度中')}</span>
                </>
              ) : (
                <>
                  <Pause className="btn-icon" />
                  <span>{t('codexQueue.paused', '已暂停')}</span>
                </>
              )}
            </button>
          </div>

          {/* Action buttons */}
          <button
            type="button"
            className="action-btn primary-action-btn"
            onClick={() => setIsAddModalOpen(true)}
          >
            <Plus className="btn-icon" />
            <span>{t('codexQueue.addTask', '添加任务')}</span>
          </button>

          {completedCount > 0 && (
            <button
              type="button"
              className="action-btn secondary-btn"
              onClick={handleClearCompleted}
              title={t('codexQueue.clearCompleted', '清空已完成任务')}
            >
              <Trash2 className="btn-icon" />
              <span>{t('codexQueue.clearCompletedBtn', '清空完成')}</span>
            </button>
          )}

          <button
            type="button"
            className="icon-action-btn"
            onClick={() => {
              loadData();
              triggerCodexQueueDispatch();
            }}
            disabled={loading}
            title={t('common.refresh', '刷新')}
          >
            <RefreshCw className={`btn-icon ${loading ? 'spinning-icon' : ''}`} />
          </button>

          <button
            type="button"
            className="icon-action-btn toggle-collapse-btn"
            onClick={toggleCollapse}
            title={isCollapsed ? t('common.expand', '展开') : t('common.collapse', '收起')}
          >
            {isCollapsed ? <ChevronDown /> : <ChevronUp />}
          </button>
        </div>
      </div>

      {/* Body Content */}
      {!isCollapsed && (
        <div className="queue-section-body">
          {error && (
            <div className="queue-error-banner">
              <AlertTriangle className="error-icon" />
              <span>{error}</span>
            </div>
          )}

          {/* Dispatcher Notice */}
          <div className="queue-rules-notice">
            <SlidersHorizontal className="notice-icon" />
            <div className="notice-content">
              <strong>{t('codexQueue.rulesHeader', '调度机制：')}</strong>
              <span>
                {t(
                  'codexQueue.rulesDesc',
                  '同一会话内的任务按先后顺序严格串行执行；若会话存在受限中断任务，系统将优先自动恢复中断任务并在其完成后继续派发排队任务；不同会话或项目按上方设置支持并发展开。'
                )}
              </span>
            </div>
          </div>

          {/* Task Cards List with Tabs */}
          <div className="queue-task-list-wrapper">
            <div className="queue-list-toolbar">
              <div className="list-toolbar-left">
                <div className="queue-tabs-group" role="tablist">
                  <button
                    type="button"
                    role="tab"
                    aria-selected={activeTab === 'queue'}
                    className={`queue-tab-btn ${activeTab === 'queue' ? 'active' : ''}`}
                    onClick={() => setActiveTab('queue')}
                  >
                    <Clock className="tab-icon" />
                    <span className="tab-label">{t('codexQueue.tabWaiting', '排队中')}</span>
                    <span className={`queue-tab-badge ${activeTasks.length > 0 ? 'highlight' : ''}`}>
                      {activeTasks.length}
                    </span>
                  </button>
                  <button
                    type="button"
                    role="tab"
                    aria-selected={activeTab === 'finished'}
                    className={`queue-tab-btn ${activeTab === 'finished' ? 'active' : ''}`}
                    onClick={() => setActiveTab('finished')}
                  >
                    <CheckCircle className="tab-icon" />
                    <span className="tab-label">{t('codexQueue.tabFinished', '已结束')}</span>
                    <span className="queue-tab-badge">
                      {finishedTasks.length}
                    </span>
                  </button>
                </div>
              </div>
              <div className="list-toolbar-right">
                <button
                  type="button"
                  className="btn-add-queued-task"
                  onClick={() => setIsAddModalOpen(true)}
                >
                  <Plus className="btn-icon" />
                  <span>{t('codexQueue.addTask', '添加排队任务')}</span>
                </button>
                {completedCount > 0 && (
                  <button
                    type="button"
                    className="btn-clear-completed"
                    onClick={handleClearCompleted}
                    title={t('codexQueue.clearCompleted', '清空已完成任务')}
                  >
                    <Trash2 className="btn-icon" />
                    <span>{t('codexQueue.clearCompletedBtn', '清空完成')}</span>
                  </button>
                )}
              </div>
            </div>

            {displayedTasks.length === 0 ? (
              <div className="queue-tab-empty-state">
                {activeTab === 'queue' ? (
                  <>
                    <Clock className="tab-empty-icon" />
                    <h4>{t('codexQueue.noWaitingTasks', '当前没有排队中的任务')}</h4>
                    <p>
                      {t(
                        'codexQueue.noWaitingTasksDesc',
                        '点击右上角「添加排队任务」，可在本机或远程服务器的项目中创建新会话或追加已有会话任务。'
                      )}
                    </p>
                    <button
                      type="button"
                      className="empty-add-btn"
                      onClick={() => setIsAddModalOpen(true)}
                    >
                      <Plus className="btn-icon" />
                      <span>{t('codexQueue.addTask', '立即添加任务')}</span>
                    </button>
                  </>
                ) : (
                  <>
                    <CheckCircle className="tab-empty-icon" />
                    <h4>{t('codexQueue.noFinishedTasks', '暂无已结束的任务')}</h4>
                    <p>
                      {t(
                        'codexQueue.noFinishedTasksDesc',
                        '排队执行完成或失败的任务会在此处归档显示。'
                      )}
                    </p>
                  </>
                )}
              </div>
            ) : (
              <div className="queue-task-list">
                {displayedTasks.map((task, index) => {
                  const isExpanded = expandedTaskIds.has(task.id);
                  const isPending =
                    activeTab === 'queue' &&
                    (task.status === 'pending' || task.status === 'waiting_interrupted');
                  const duration = formatDuration(task);
                  const cwdBasename = task.cwd.split('/').filter(Boolean).pop() || task.cwd;

                  return (
                    <div
                      key={task.id}
                      className={`queue-task-card status-${task.status}`}
                    >
                      {/* Left Reorder controls for pending tasks */}
                      <div className="card-left-controls">
                        {isPending ? (
                          <div className="reorder-btn-group">
                            <button
                              type="button"
                              className="reorder-btn"
                              disabled={index === 0 || activeTasks[index - 1]?.status === 'running'}
                              onClick={() => handleMoveActiveTask(task.id, 'up')}
                              title={t('codexQueue.moveUp', '上移')}
                            >
                              <ArrowUp />
                            </button>
                            <span className="order-number">#{index + 1}</span>
                            <button
                              type="button"
                              className="reorder-btn"
                              disabled={index === activeTasks.length - 1}
                              onClick={() => handleMoveActiveTask(task.id, 'down')}
                              title={t('codexQueue.moveDown', '下移')}
                            >
                              <ArrowDown />
                            </button>
                          </div>
                        ) : (
                          <div className="static-order-number">#{index + 1}</div>
                        )}
                      </div>

                      {/* Main content */}
                      <div className="card-main-content">
                        <div className="task-header-row">
                          <div className="title-and-status">
                            {renderStatusBadge(task.status)}
                            <h4 className="task-title" onClick={() => toggleExpandTask(task.id)}>
                              {task.title}
                            </h4>
                          </div>

                          {/* Top Chips */}
                          <div className="task-meta-chips">
                            {/* Target Source */}
                            <span className="meta-chip target" title={`执行环境: ${task.source}`}>
                              {task.source === 'local' ? (
                                <>
                                  <Laptop className="chip-icon" />
                                  <span>本机</span>
                                </>
                              ) : (
                                <>
                                  <Server className="chip-icon" />
                                  <span>{task.server_name || '远端 SSH'}</span>
                                </>
                              )}
                            </span>

                            {/* Project cwd */}
                            <span className="meta-chip cwd" title={task.cwd}>
                              <Folder className="chip-icon" />
                              <span>{cwdBasename}</span>
                            </span>

                            {/* Thread */}
                            <span className="meta-chip thread" title={task.thread_id ? `会话: ${task.thread_id}` : '新建独立会话'}>
                              {task.thread_id ? (
                                <>
                                  <MessageSquare className="chip-icon" />
                                  <span>{task.thread_title || '已有会话'}</span>
                                </>
                              ) : (
                                <>
                                  <Sparkles className="chip-icon" />
                                  <span>新建会话</span>
                                </>
                              )}
                            </span>

                            {/* Model */}
                            {task.model && (
                              <span className="meta-chip model">
                                <span>{task.model}</span>
                              </span>
                            )}

                            {/* Duration */}
                            {duration && (
                              <span className="meta-chip duration">
                                <Clock className="chip-icon" />
                                <span>{duration}</span>
                              </span>
                            )}
                          </div>
                        </div>

                        {/* Prompt Content Preview / Expandable */}
                        <div
                          className={`task-prompt-box ${isExpanded ? 'expanded' : ''}`}
                          onClick={() => toggleExpandTask(task.id)}
                        >
                          <p className="prompt-text">{task.prompt}</p>
                          {task.prompt.length > 90 && (
                            <span className="expand-hint">
                              {isExpanded ? t('common.collapse', '收起') : t('common.expand', '展开全部')}
                            </span>
                          )}
                        </div>

                        {/* Waiting Interrupted Notice or Error banner */}
                        {task.status === 'waiting_interrupted' && (
                          <div className="task-waiting-interrupted-banner">
                            <AlertTriangle className="warning-icon" />
                            <span>
                              {task.error_message ||
                                t(
                                  'codexQueue.waitingInterruptedDesc',
                                  '检测到当前会话存在用量中断的任务。调度器已优先恢复该任务，待其完全执行完成后自动提交本任务。'
                                )}
                            </span>
                          </div>
                        )}

                        {task.status === 'failed' && task.error_message && (
                          <div className="task-failed-banner">
                            <XCircle className="error-icon" />
                            <span>{task.error_message}</span>
                          </div>
                        )}

                        {/* Footer Timestamps */}
                        <div className="task-card-footer">
                          <span className="timestamp-text">
                            创建于 {new Date(task.created_at * 1000).toLocaleTimeString()}
                          </span>
                          {task.started_at && (
                            <span className="timestamp-text">
                              · 开始于 {new Date(task.started_at * 1000).toLocaleTimeString()}
                            </span>
                          )}
                          {task.completed_at && (
                            <span className="timestamp-text">
                              · 完成于 {new Date(task.completed_at * 1000).toLocaleTimeString()}
                            </span>
                          )}
                          {task.assigned_thread_id && (
                            <span className="timestamp-text thread-id-text">
                              · 关联会话: {task.assigned_thread_id.slice(0, 8)}...
                            </span>
                          )}
                        </div>
                      </div>

                      {/* Right side actions */}
                      <div className="card-right-actions">
                        {task.status === 'failed' && (
                          <button
                            type="button"
                            className="card-action-btn retry-btn"
                            onClick={() => handleRetryTask(task.id)}
                            title={t('common.retry', '重试')}
                          >
                            <RotateCcw />
                          </button>
                        )}
                        <button
                          type="button"
                          className="card-action-btn delete-btn"
                          onClick={() => handleDeleteTask(task.id)}
                          title={t('common.delete', '删除任务')}
                        >
                          <Trash2 />
                        </button>
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </div>
      )}

      {/* Add Task Modal */}
      <CodexAddTaskModal
        isOpen={isAddModalOpen}
        onClose={() => setIsAddModalOpen(false)}
        onTaskAdded={() => {
          setActiveTab('queue');
          loadData(true);
        }}
      />
    </div>
  );
}
