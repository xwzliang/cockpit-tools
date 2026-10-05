import { useEffect, useState, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { confirm as confirmDialog } from '@tauri-apps/plugin-dialog';
import {
  AlertTriangle,
  Bot,
  CheckCircle,
  ChevronDown,
  ChevronUp,
  Clock,
  ClockAlert,
  Folder,
  Laptop,
  Play,
  RefreshCw,
  Server,
  Trash2,
  XCircle,
  Zap,
} from 'lucide-react';
import {
  detectInterruptedCodexTasks,
  getAutoResumedTasks,
  clearAutoResumedTasks,
  resumeInterruptedCodexTask,
  setCodexTaskExcluded,
  deleteInterruptedCodexTask,
  type InterruptedCodexTask,
  type ResumedTaskRecord,
} from '../../services/codexAutoResumeService';
import './CodexAutoResumeSection.css';

interface CodexAutoResumeSectionProps {
  variant?: 'overview' | 'settings';
  onTaskResumed?: () => void;
}

export function CodexAutoResumeSection({
  variant = 'overview',
  onTaskResumed,
}: CodexAutoResumeSectionProps) {
  const { t } = useTranslation();

  const [loading, setLoading] = useState(false);
  const [interruptedTasks, setInterruptedTasks] = useState<InterruptedCodexTask[]>([]);
  const [resumedTasks, setResumedTasks] = useState<ResumedTaskRecord[]>([]);
  const [activeTab, setActiveTab] = useState<'interrupted' | 'resumed'>('interrupted');
  const [isCollapsed, setIsCollapsed] = useState<boolean>(() => {
    const saved = localStorage.getItem('codex_auto_resume_section_collapsed');
    return saved === 'true';
  });
  const [autoResumeEnabled, setAutoResumeEnabled] = useState(false);
  const [syncSshEnabled, setSyncSshEnabled] = useState(true);
  const [autoApplyExpiringResetEnabled, setAutoApplyExpiringResetEnabled] = useState(true);
  const [resumingTaskId, setResumingTaskId] = useState<string | null>(null);
  const [actionMessage, setActionMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [nowSec, setNowSec] = useState<number>(Math.floor(Date.now() / 1000));
  const [lastUpdated, setLastUpdated] = useState<Date>(new Date());

  // Ticking timer for live countdown
  useEffect(() => {
    const timer = setInterval(() => {
      setNowSec(Math.floor(Date.now() / 1000));
    }, 1000);
    return () => clearInterval(timer);
  }, []);

  const loadData = useCallback(async (isSilent = false) => {
    if (!isSilent) {
      setLoading(true);
      setError(null);
    }
    try {
      const [interrupted, resumed] = await Promise.all([
        detectInterruptedCodexTasks().catch((err) => {
          console.warn('Failed to detect interrupted tasks:', err);
          return [] as InterruptedCodexTask[];
        }),
        getAutoResumedTasks().catch((err) => {
          console.warn('Failed to load auto-resumed tasks:', err);
          return [] as ResumedTaskRecord[];
        }),
      ]);
      setInterruptedTasks(interrupted);
      setResumedTasks(resumed);
      setLastUpdated(new Date());

      // Auto expand on initial/manual load if there are interrupted tasks
      if (!isSilent && interrupted.length > 0) {
        setIsCollapsed(false);
      }
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

  const loadConfig = useCallback(async () => {
    try {
      const cfg = await invoke<{
        codex_auto_resume_interrupted_tasks_enabled?: boolean;
        codex_auto_resume_sync_ssh?: boolean;
        codex_auto_apply_reset_before_expiration_enabled?: boolean;
      }>('get_general_config');
      setAutoResumeEnabled(Boolean(cfg.codex_auto_resume_interrupted_tasks_enabled));
      setSyncSshEnabled(cfg.codex_auto_resume_sync_ssh ?? true);
      setAutoApplyExpiringResetEnabled(cfg.codex_auto_apply_reset_before_expiration_enabled ?? true);
    } catch (err) {
      console.warn('Failed to load general config in auto resume section:', err);
    }
  }, []);

  useEffect(() => {
    void loadConfig();
  }, [loadConfig]);

  const handleToggleAutoResume = async (enabled: boolean) => {
    setAutoResumeEnabled(enabled);
    try {
      await invoke('patch_general_config', {
        updates: {
          codex_auto_resume_interrupted_tasks_enabled: enabled,
        },
      });
    } catch (err) {
      console.error('Failed to patch auto resume config:', err);
    }
  };

  const handleToggleSyncSsh = async (syncSsh: boolean) => {
    setSyncSshEnabled(syncSsh);
    try {
      await invoke('patch_general_config', {
        updates: {
          codex_auto_resume_sync_ssh: syncSsh,
        },
      });
      setTimeout(() => {
        void loadData();
      }, 300);
    } catch (err) {
      console.error('Failed to patch sync ssh config:', err);
    }
  };

  const handleToggleAutoApplyExpiringReset = async (enabled: boolean) => {
    setAutoApplyExpiringResetEnabled(enabled);
    try {
      await invoke('patch_general_config', {
        updates: {
          codex_auto_apply_reset_before_expiration_enabled: enabled,
        },
      });
    } catch (err) {
      console.error('Failed to patch auto apply expiring reset config:', err);
    }
  };

  const handleToggleTaskExcluded = async (taskId: string, excluded: boolean) => {
    setInterruptedTasks((prev) =>
      prev.map((t) => (t.id === taskId ? { ...t, is_excluded: excluded } : t)),
    );
    try {
      await setCodexTaskExcluded(taskId, excluded);
    } catch (err) {
      console.error('Failed to update task exclusion:', err);
      setInterruptedTasks((prev) =>
        prev.map((t) => (t.id === taskId ? { ...t, is_excluded: !excluded } : t)),
      );
    }
  };

  useEffect(() => {
    void loadData();
  }, [loadData]);

  // Periodic auto-update every 1 minute so interrupted and resumed tasks are continuously tracked
  useEffect(() => {
    const timer = setInterval(() => {
      if (!resumingTaskId) {
        void loadData(true);
      }
    }, 60000);
    return () => clearInterval(timer);
  }, [loadData, resumingTaskId]);

  // Refresh when window regains focus
  useEffect(() => {
    const handleFocus = () => {
      if (!resumingTaskId) {
        void loadData(true);
      }
    };
    window.addEventListener('focus', handleFocus);
    return () => window.removeEventListener('focus', handleFocus);
  }, [loadData, resumingTaskId]);

  // Listen to background auto-resume events
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen('codex://task-auto-resumed', () => {
      void loadData(true);
    }).then((fn) => {
      unlisten = fn;
    });

    return () => {
      unlisten?.();
    };
  }, [loadData]);

  const toggleCollapse = () => {
    setIsCollapsed((prev) => {
      const next = !prev;
      localStorage.setItem('codex_auto_resume_section_collapsed', String(next));
      return next;
    });
  };

  const handleResumeTask = async (task: InterruptedCodexTask) => {
    setResumingTaskId(task.id);
    setActionMessage(null);
    setError(null);
    try {
      const res = await resumeInterruptedCodexTask(task);
      setActionMessage(res.message);
      onTaskResumed?.();
      setTimeout(() => {
        void loadData();
      }, 1500);
    } catch (err) {
      setError(`恢复任务失败: ${err}`);
    } finally {
      setResumingTaskId(null);
    }
  };

  const handleDeleteTask = async (task: InterruptedCodexTask) => {
    const confirmed = await confirmDialog(
      t(
        'codex.autoResume.deleteConfirmMessage',
        '确定要从受限中断列表中删除该任务吗？删除后将不再跟踪和提示此任务。',
      ),
      {
        title: t('codex.autoResume.deleteConfirmTitle', '删除中断任务'),
        kind: 'warning',
      },
    );
    if (!confirmed) return;

    try {
      await deleteInterruptedCodexTask(task.id);
      setInterruptedTasks((prev) => prev.filter((t) => t.id !== task.id));
      setActionMessage(t('codex.autoResume.deleteSuccess', '任务已从列表中移除'));
      setTimeout(() => setActionMessage(null), 3000);
    } catch (err) {
      setError(`删除任务失败: ${err}`);
    }
  };

  const handleClearHistory = async () => {
    const confirmed = await confirmDialog(
      t('codex.autoResume.clearHistoryConfirm', '确定要清空所有恢复记录吗？'),
      {
        title: t('codex.autoResume.clearHistory', '清空记录'),
        kind: 'warning',
      },
    );
    if (!confirmed) return;

    try {
      await clearAutoResumedTasks();
      setResumedTasks([]);
      setActionMessage(t('codex.autoResume.historyCleared', '恢复记录已清空'));
    } catch (err) {
      setError(`清空记录失败: ${err}`);
    }
  };

  const formatCountdown = (resetsAt?: number | null) => {
    if (!resetsAt) return null;
    const diff = resetsAt - nowSec;
    if (diff <= 0) {
      return t('codex.autoResume.quotaRestored', '配额已到重置时间，可立即恢复');
    }
    const hours = Math.floor(diff / 3600);
    const minutes = Math.floor((diff % 3600) / 60);
    const seconds = diff % 60;
    if (hours > 0) {
      return t(
        'codex.autoResume.remainingHoursMinutesSeconds',
        '距离配额重置还剩 {{hours}} 小时 {{minutes}} 分钟 {{seconds}} 秒',
        { hours, minutes, seconds },
      );
    }
    return t(
      'codex.autoResume.remainingMinutesSeconds',
      '距离配额重置还剩 {{minutes}} 分钟 {{seconds}} 秒',
      { minutes, seconds },
    );
  };

  const formatResetTime = (resetsAt?: number | null) => {
    if (!resetsAt) return '';
    const date = new Date(resetsAt * 1000);
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  };

  const formatDateTime = (timestampSec: number) => {
    if (!timestampSec) return '';
    const date = new Date(timestampSec * 1000);
    return date.toLocaleString();
  };

  const hasInterrupted = interruptedTasks.length > 0;
  const isSettings = variant === 'settings';
  const effectiveCollapsed = isSettings ? false : isCollapsed;

  return (
    <div
      className={`codex-auto-resume-section ${hasInterrupted ? 'has-interrupted' : ''} ${
        effectiveCollapsed ? 'is-collapsed' : ''
      } ${isSettings ? 'is-settings-variant' : ''}`}
    >
      {/* Header */}
      <div className="codex-auto-resume-header">
        <div className="codex-auto-resume-header-left">
          {!isSettings && (
            <>
              <div
                className={`codex-auto-resume-icon ${hasInterrupted ? 'has-tasks' : ''}`}
                title={hasInterrupted ? '有任务因配额受限中断中' : '任务运行正常'}
              >
                {hasInterrupted ? <ClockAlert size={16} /> : <Zap size={16} />}
              </div>

              <div className="codex-auto-resume-header-title">
                <span>{t('codex.autoResume.sectionTitle', '受限中断与恢复任务')}</span>
              </div>
            </>
          )}

          {/* Sub tabs */}
          <div className="codex-auto-resume-tabs">
            <button
              type="button"
              className={`codex-auto-resume-tab ${activeTab === 'interrupted' ? 'active' : ''}`}
              onClick={() => {
                setActiveTab('interrupted');
                if (isCollapsed) setIsCollapsed(false);
              }}
            >
              <span>{t('codex.autoResume.tabInterrupted', '受限中断中')}</span>
              <span
                className={`codex-auto-resume-tab-badge ${
                  interruptedTasks.length > 0 ? 'warning' : 'neutral'
                }`}
              >
                {interruptedTasks.length}
              </span>
            </button>

            <button
              type="button"
              className={`codex-auto-resume-tab ${activeTab === 'resumed' ? 'active' : ''}`}
              onClick={() => {
                setActiveTab('resumed');
                if (isCollapsed) setIsCollapsed(false);
              }}
            >
              <span>{t('codex.autoResume.tabResumed', '已自动恢复')}</span>
              <span
                className={`codex-auto-resume-tab-badge ${
                  resumedTasks.length > 0 ? 'success' : 'neutral'
                }`}
              >
                {resumedTasks.length}
              </span>
            </button>
          </div>

          <span
            className={`codex-auto-resume-status-badge ${autoResumeEnabled ? 'on' : 'off'}`}
            title={t(
              'codex.autoResume.enableDesc',
              '当 Codex 任务或 /goal 因达到 5 小时（或周度）配额限制中断时，后台在配额恢复后自动恢复执行。',
            )}
          >
            {autoResumeEnabled
              ? `⚡ ${t('codex.autoResume.statusEnabled', '自动恢复已开启')}`
              : t('codex.autoResume.statusDisabled', '自动恢复未开启')}
          </span>
        </div>

        <div className="codex-auto-resume-header-right">
          {activeTab === 'resumed' && resumedTasks.length > 0 && !effectiveCollapsed && (
            <button
              type="button"
              className="codex-auto-resume-btn-text"
              onClick={handleClearHistory}
              title={t('codex.autoResume.clearHistory', '清空记录')}
            >
              <Trash2 size={13} />
              <span>{t('codex.autoResume.clearHistory', '清空记录')}</span>
            </button>
          )}

          <button
            type="button"
            className="codex-auto-resume-btn-icon"
            onClick={() => void loadData(false)}
            disabled={loading}
            title={`${t('common.refresh', '刷新扫描')} (${t('codex.autoResume.autoUpdateInterval', '每 1 分钟自动更新')} · ${t('codex.autoResume.lastUpdated', '上次更新: {{time}}', { time: lastUpdated.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' }) })})`}
          >
            <RefreshCw size={14} className={loading ? 'spin' : undefined} />
          </button>

          {!isSettings && (
            <button
              type="button"
              className="codex-auto-resume-btn-icon"
              onClick={toggleCollapse}
              title={
                isCollapsed
                  ? t('codex.autoResume.expand', '展开')
                  : t('codex.autoResume.collapse', '收起')
              }
            >
              {isCollapsed ? <ChevronDown size={14} /> : <ChevronUp size={14} />}
            </button>
          )}
        </div>
      </div>

      {/* Settings bar for quick inline management */}
      {!effectiveCollapsed && (
        <div className="codex-auto-resume-settings-bar">
          <div className="codex-auto-resume-setting-item">
            <label className="switch switch-sm">
              <input
                type="checkbox"
                checked={autoResumeEnabled}
                onChange={(e) => void handleToggleAutoResume(e.target.checked)}
              />
              <span className="slider"></span>
            </label>
            <span
              className="setting-label"
              onClick={() => void handleToggleAutoResume(!autoResumeEnabled)}
            >
              {t('codex.autoResume.enableTitle', '受限中断任务自动恢复')}
            </span>
          </div>

          <div className="codex-auto-resume-setting-item">
            <label className="switch switch-sm">
              <input
                type="checkbox"
                checked={syncSshEnabled}
                onChange={(e) => void handleToggleSyncSsh(e.target.checked)}
              />
              <span className="slider"></span>
            </label>
            <span
              className="setting-label"
              onClick={() => void handleToggleSyncSsh(!syncSshEnabled)}
            >
              {t('codex.autoResume.syncSshTitle', '包含远程 SSH 服务器')}
            </span>
          </div>

          <div className="codex-auto-resume-setting-item">
            <label className="switch switch-sm">
              <input
                type="checkbox"
                checked={autoApplyExpiringResetEnabled}
                onChange={(e) => void handleToggleAutoApplyExpiringReset(e.target.checked)}
              />
              <span className="slider"></span>
            </label>
            <span
              className="setting-label"
              title={t('codex.autoResume.autoApplyExpiringResetTooltip', '当重置额度距离到期时间不足 10 分钟时自动使用，避免过期作废')}
              onClick={() => void handleToggleAutoApplyExpiringReset(!autoApplyExpiringResetEnabled)}
            >
              {t('codex.autoResume.autoApplyExpiringResetTitle', '重置额度到期前 10 分钟自动使用')}
            </span>
          </div>
        </div>
      )}

      {/* Alert message if any */}
      {actionMessage && (
        <div className="codex-auto-resume-alert success">
          <CheckCircle size={14} />
          <span>{actionMessage}</span>
        </div>
      )}

      {error && (
        <div className="codex-auto-resume-alert error">
          <AlertTriangle size={14} />
          <span>{error}</span>
        </div>
      )}

      {/* Body content */}
      {!isCollapsed && (
        <div className="codex-auto-resume-body">
          {loading && interruptedTasks.length === 0 && resumedTasks.length === 0 ? (
            <div key="loading" className="codex-auto-resume-empty">
              <RefreshCw size={24} className="spin codex-auto-resume-empty-icon" />
              <div className="codex-auto-resume-empty-title">
                {t('codex.autoResume.scanning', '正在扫描本地及远程 SSH 服务器的中断任务...')}
              </div>
            </div>
          ) : activeTab === 'interrupted' ? (
            interruptedTasks.length === 0 ? (
              <div key="interrupted-empty" className="codex-auto-resume-empty">
                <CheckCircle size={28} className="codex-auto-resume-empty-icon" />
                <div className="codex-auto-resume-empty-title">
                  {t('codex.autoResume.emptyTitle', '当前无受限中断的任务')}
                </div>
                <div className="codex-auto-resume-empty-desc">
                  {t(
                    'codex.autoResume.emptyDesc',
                    '本地及已配置的 SSH 服务器上没有处于配额受限中断状态的 Codex 任务。若后续有任务达到 5 小时配额中断，将在此处显示并自动排队等待恢复。',
                  )}
                </div>
              </div>
            ) : (
              interruptedTasks.map((task, index) => {
                const countdown = !task.can_resume ? formatCountdown(task.resets_at) : null;
                const resetTimeStr = formatResetTime(task.resets_at);
                const isResuming = resumingTaskId === task.id;

                return (
                  <div key={`${task.id}-${index}`} className="codex-task-card">
                    <div className="codex-task-card__header">
                      <div className="codex-task-card__tags">
                        {task.source === 'local' ? (
                          <span className="codex-task-tag codex-task-tag--source-local">
                            <Laptop size={12} />
                            {t('codex.autoResume.localMachine', '本地')}
                          </span>
                        ) : (
                          <span className="codex-task-tag codex-task-tag--source-ssh">
                            <Server size={12} />
                            {task.server_name || t('codex.autoResume.sshServer', 'SSH 服务器')}
                          </span>
                        )}

                        {task.is_goal ? (
                          <span className="codex-task-tag codex-task-tag--goal">
                            🎯 {t('codex.autoResume.goalType', 'Goal 目标')}
                          </span>
                        ) : (
                          <span className="codex-task-tag codex-task-tag--task">
                            📝 {t('codex.autoResume.taskType', '常规任务')}
                          </span>
                        )}

                        {task.can_resume ? (
                          <span className="codex-task-tag codex-task-tag--status-ready">
                            <CheckCircle size={12} />
                            {t('codex.autoResume.canResumeNow', '可恢复')}
                          </span>
                        ) : (
                          <span className="codex-task-tag codex-task-tag--status-waiting">
                            <Clock size={12} />
                            {t('codex.autoResume.waitingQuota', '等待配额')}
                          </span>
                        )}

                        {task.is_excluded && (
                          <span className="codex-task-tag codex-task-tag--excluded">
                            {t('codex.autoResume.taskExcludedBadge', '已排除自动恢复')}
                          </span>
                        )}
                      </div>

                      {autoResumeEnabled && (
                        <label
                          className={`codex-task-auto-resume-checkbox ${task.is_excluded ? 'is-excluded' : 'is-included'}`}
                          title={
                            task.is_excluded
                              ? t('codex.autoResume.taskExcludedTooltip', '已排除：配额恢复后不会自动恢复此任务')
                              : t('codex.autoResume.taskIncludedTooltip', '配额恢复后将自动恢复此任务')
                          }
                          onClick={(e) => e.stopPropagation()}
                        >
                          <input
                            type="checkbox"
                            checked={!task.is_excluded}
                            onChange={() => void handleToggleTaskExcluded(task.id, !task.is_excluded)}
                          />
                          <span>{t('codex.autoResume.taskCheckboxLabel', '自动恢复')}</span>
                        </label>
                      )}
                    </div>

                    <div className="codex-task-card__title">
                      {task.title}
                    </div>

                    <div className="codex-task-card__meta">
                      {task.cwd && (
                        <span className="codex-task-card__meta-item" title={task.cwd}>
                          <Folder size={12} />
                          {task.cwd}
                        </span>
                      )}
                      {task.model && (
                        <span className="codex-task-card__meta-item">
                          <Bot size={12} />
                          {task.model}
                        </span>
                      )}
                    </div>

                    <div className="codex-task-card__reset-box">
                      <div className="codex-task-card__reset-info">
                        {countdown ? (
                          <div className="codex-task-card__reset-countdown">
                            <Clock size={14} />
                            {countdown}
                          </div>
                        ) : task.can_resume ? (
                          <div className="codex-task-card__reset-countdown can-resume-text">
                            <CheckCircle size={14} />
                            {t('codex.autoResume.quotaRestored', '可立即恢复执行')}
                          </div>
                        ) : (
                          <div className="codex-task-card__reset-countdown">
                            <Clock size={14} />
                            {t('codex.autoResume.waitingQuota', '等待配额重置')}
                          </div>
                        )}
                        {!task.can_resume && resetTimeStr && (
                          <div className="codex-task-card__reset-time">
                            {t('codex.autoResume.resetTimeLabel', '预计重置时间: {{time}}', {
                              time: resetTimeStr,
                            })}
                          </div>
                        )}
                      </div>

                      <div className="codex-task-card__actions">
                        <button
                          type="button"
                          className="btn-delete"
                          onClick={() => void handleDeleteTask(task)}
                          disabled={isResuming}
                          title={t('codex.autoResume.deleteTaskTooltip', '从任务列表中删除此中断任务，不再监控')}
                        >
                          <Trash2 size={13} />
                          {t('common.delete', '删除')}
                        </button>
                        <button
                          type="button"
                          className="btn-resume"
                          onClick={() => handleResumeTask(task)}
                          disabled={isResuming || !task.can_resume}
                          title={
                            !task.can_resume
                              ? t('codex.autoResume.cannotResumeYet', '配额尚未重置，暂无法恢复')
                              : undefined
                          }
                        >
                          {isResuming ? (
                            <>
                              <RefreshCw size={13} className="spin" />
                              {t('codex.autoResume.resuming', '正在恢复...')}
                            </>
                          ) : (
                            <>
                              <Play size={13} />
                              {t('codex.autoResume.resumeNow', '立即恢复')}
                            </>
                          )}
                        </button>
                      </div>
                    </div>
                  </div>
                );
              })
            )
          ) : (
            // Resumed history tab
            resumedTasks.length === 0 ? (
              <div key="resumed-empty" className="codex-auto-resume-empty">
                <CheckCircle size={28} className="codex-auto-resume-empty-icon" />
                <div className="codex-auto-resume-empty-title">
                  {t('codex.autoResume.emptyResumed', '暂无自动恢复记录')}
                </div>
                <div className="codex-auto-resume-empty-desc">
                  {t(
                    'codex.autoResume.emptyResumedDesc',
                    '当后台自动或手动恢复因配额受限中断的任务后，恢复详情与执行状态将记录在此处。',
                  )}
                </div>
              </div>
            ) : (
              <div key="resumed-list" className="codex-resumed-history-list">
                {resumedTasks.map((record, index) => (
                  <div key={`${record.id}-${record.resumed_at}-${index}`} className="codex-resumed-item">
                    <div className="codex-resumed-item-top">
                      <div className="codex-task-card__tags">
                        {record.source === 'local' ? (
                          <span className="codex-task-tag codex-task-tag--source-local">
                            <Laptop size={12} />
                            {t('codex.autoResume.localMachine', '本地')}
                          </span>
                        ) : (
                          <span className="codex-task-tag codex-task-tag--source-ssh">
                            <Server size={12} />
                            {record.server_name || t('codex.autoResume.sshServer', 'SSH 服务器')}
                          </span>
                        )}

                        {record.trigger_type === 'auto' ? (
                          <span className="codex-task-tag codex-task-tag--trigger-auto">
                            ⚡ {t('codex.autoResume.triggerAuto', '自动恢复')}
                          </span>
                        ) : (
                          <span className="codex-task-tag codex-task-tag--trigger-manual">
                            👆 {t('codex.autoResume.triggerManual', '手动恢复')}
                          </span>
                        )}

                        {record.success ? (
                          <span className="codex-task-tag codex-task-tag--status-ready">
                            <CheckCircle size={12} />
                            {t('codex.autoResume.resumedStatusSuccess', '恢复成功')}
                          </span>
                        ) : (
                          <span className="codex-task-tag" style={{ background: 'rgba(239, 68, 68, 0.15)', color: '#f87171', border: '1px solid rgba(239, 68, 68, 0.3)' }}>
                            <XCircle size={12} />
                            {t('codex.autoResume.resumedStatusFailed', '恢复失败')}
                          </span>
                        )}

                        {record.is_goal && (
                          <span className="codex-task-tag codex-task-tag--goal">
                            🎯 {t('codex.autoResume.goalType', 'Goal 目标')}
                          </span>
                        )}
                      </div>

                      <div className="codex-resumed-item-time">
                        <Clock size={12} />
                        <span>{formatDateTime(record.resumed_at)}</span>
                      </div>
                    </div>

                    <div className="codex-task-card__title">{record.title}</div>

                    <div className="codex-task-card__meta">
                      {record.cwd && (
                        <span className="codex-task-card__meta-item" title={record.cwd}>
                          <Folder size={12} />
                          {record.cwd}
                        </span>
                      )}
                      {record.model && (
                        <span className="codex-task-card__meta-item">
                          <Bot size={12} />
                          {record.model}
                        </span>
                      )}
                    </div>

                    {record.message && (
                      <div className={`codex-resumed-item-msg ${record.success ? '' : 'error'}`}>
                        {record.message}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )
          )}
        </div>
      )}
    </div>
  );
}
