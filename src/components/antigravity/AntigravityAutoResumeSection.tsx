import { useEffect, useState, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { confirm as confirmDialog } from '@tauri-apps/plugin-dialog';
import {
  AlertTriangle,
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
  detectInterruptedAntigravityTasks,
  getAntigravityAutoResumedTasks,
  clearAntigravityAutoResumedTasks,
  resumeInterruptedAntigravityTask,
  setAntigravityTaskExcluded,
  type InterruptedAntigravityTask,
  type ResumedTaskRecord,
} from '../../services/antigravityAutoResumeService';
import './AntigravityAutoResumeSection.css';

interface AntigravityAutoResumeSectionProps {
  variant?: 'overview' | 'settings';
  onTaskResumed?: () => void;
}

export function AntigravityAutoResumeSection({
  variant = 'overview',
  onTaskResumed,
}: AntigravityAutoResumeSectionProps) {
  const { t } = useTranslation();

  const [loading, setLoading] = useState(false);
  const [interruptedTasks, setInterruptedTasks] = useState<InterruptedAntigravityTask[]>([]);
  const [resumedTasks, setResumedTasks] = useState<ResumedTaskRecord[]>([]);
  const [activeTab, setActiveTab] = useState<'interrupted' | 'resumed'>('interrupted');
  const [isCollapsed, setIsCollapsed] = useState<boolean>(() => {
    const saved = localStorage.getItem('antigravity_auto_resume_section_collapsed');
    return saved === 'true';
  });
  const [autoResumeEnabled, setAutoResumeEnabled] = useState(false);
  const [syncSshEnabled, setSyncSshEnabled] = useState(true);
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
        detectInterruptedAntigravityTasks().catch((err) => {
          console.warn('Failed to detect interrupted tasks:', err);
          return [] as InterruptedAntigravityTask[];
        }),
        getAntigravityAutoResumedTasks().catch((err) => {
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
        antigravity_auto_resume_interrupted_tasks_enabled?: boolean;
        antigravity_auto_resume_sync_ssh?: boolean;
      }>('get_general_config');
      setAutoResumeEnabled(Boolean(cfg.antigravity_auto_resume_interrupted_tasks_enabled));
      setSyncSshEnabled(cfg.antigravity_auto_resume_sync_ssh ?? true);
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
          antigravity_auto_resume_interrupted_tasks_enabled: enabled,
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
          antigravity_auto_resume_sync_ssh: syncSsh,
        },
      });
      setTimeout(() => {
        void loadData();
      }, 300);
    } catch (err) {
      console.error('Failed to patch sync ssh config:', err);
    }
  };

  const handleToggleTaskExcluded = async (taskId: string, excluded: boolean) => {
    setInterruptedTasks((prev) =>
      prev.map((t) => (t.id === taskId ? { ...t, is_excluded: excluded } : t)),
    );
    try {
      await setAntigravityTaskExcluded(taskId, excluded);
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
    listen('antigravity://task-auto-resumed', () => {
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
      localStorage.setItem('antigravity_auto_resume_section_collapsed', String(next));
      return next;
    });
  };

  const handleResumeTask = async (task: InterruptedAntigravityTask) => {
    setResumingTaskId(task.id);
    setActionMessage(null);
    setError(null);
    try {
      const res = await resumeInterruptedAntigravityTask(task);
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

  const handleClearHistory = async () => {
    const confirmed = await confirmDialog(
      t('antigravity.autoResume.clearHistoryConfirm', '确定要清空所有恢复记录吗？'),
      {
        title: t('antigravity.autoResume.clearHistory', '清空记录'),
        kind: 'warning',
      },
    );
    if (!confirmed) return;

    try {
      await clearAntigravityAutoResumedTasks();
      setResumedTasks([]);
      setActionMessage(t('antigravity.autoResume.historyCleared', '恢复记录已清空'));
    } catch (err) {
      setError(`清空记录失败: ${err}`);
    }
  };

  const formatCountdown = (resetsAt?: number | null) => {
    if (!resetsAt) return null;
    const diff = resetsAt - nowSec;
    if (diff <= 0) {
      return t('antigravity.autoResume.quotaRestored', '配额已到重置时间，可立即恢复');
    }
    const hours = Math.floor(diff / 3600);
    const minutes = Math.floor((diff % 3600) / 60);
    const seconds = diff % 60;
    if (hours > 0) {
      return t(
        'antigravity.autoResume.remainingHoursMinutesSeconds',
        '距离配额重置还剩 {{hours}} 小时 {{minutes}} 分钟 {{seconds}} 秒',
        { hours, minutes, seconds },
      );
    }
    return t(
      'antigravity.autoResume.remainingMinutesSeconds',
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
      className={`antigravity-auto-resume-section ${hasInterrupted ? 'has-interrupted' : ''} ${
        effectiveCollapsed ? 'is-collapsed' : ''
      } ${isSettings ? 'is-settings-variant' : ''}`}
    >
      {/* Header */}
      <div className="antigravity-auto-resume-header">
        <div className="antigravity-auto-resume-header-left">
          {!isSettings && (
            <>
              <div
                className={`antigravity-auto-resume-icon ${hasInterrupted ? 'has-tasks' : ''}`}
                title={hasInterrupted ? '有任务因配额受限中断中' : '任务运行正常'}
              >
                {hasInterrupted ? <ClockAlert size={16} /> : <Zap size={16} />}
              </div>

              <div className="antigravity-auto-resume-header-title">
                <span>{t('antigravity.autoResume.sectionTitle', '受限中断与恢复任务')}</span>
              </div>
            </>
          )}

          {/* Sub tabs */}
          <div className="antigravity-auto-resume-tabs">
            <button
              type="button"
              className={`antigravity-auto-resume-tab ${activeTab === 'interrupted' ? 'active' : ''}`}
              onClick={() => {
                setActiveTab('interrupted');
                if (isCollapsed) setIsCollapsed(false);
              }}
            >
              <span>{t('antigravity.autoResume.tabInterrupted', '受限中断中')}</span>
              <span
                className={`antigravity-auto-resume-tab-badge ${
                  interruptedTasks.length > 0 ? 'warning' : 'neutral'
                }`}
              >
                {interruptedTasks.length}
              </span>
            </button>

            <button
              type="button"
              className={`antigravity-auto-resume-tab ${activeTab === 'resumed' ? 'active' : ''}`}
              onClick={() => {
                setActiveTab('resumed');
                if (isCollapsed) setIsCollapsed(false);
              }}
            >
              <span>{t('antigravity.autoResume.tabResumed', '已自动恢复')}</span>
              <span
                className={`antigravity-auto-resume-tab-badge ${
                  resumedTasks.length > 0 ? 'success' : 'neutral'
                }`}
              >
                {resumedTasks.length}
              </span>
            </button>
          </div>

          <span
            className={`antigravity-auto-resume-status-badge ${autoResumeEnabled ? 'on' : 'off'}`}
            title={t(
              'antigravity.autoResume.enableDesc',
              '当 Antigravity 任务因达到配额限制中断时，后台在配额恢复后自动恢复执行。',
            )}
          >
            {autoResumeEnabled
              ? `⚡ ${t('antigravity.autoResume.statusEnabled', '自动恢复已开启')}`
              : t('antigravity.autoResume.statusDisabled', '自动恢复未开启')}
          </span>
        </div>

        <div className="antigravity-auto-resume-header-right">
          {activeTab === 'resumed' && resumedTasks.length > 0 && !effectiveCollapsed && (
            <button
              type="button"
              className="antigravity-auto-resume-btn-text"
              onClick={handleClearHistory}
              title={t('antigravity.autoResume.clearHistory', '清空记录')}
            >
              <Trash2 size={13} />
              <span>{t('antigravity.autoResume.clearHistory', '清空记录')}</span>
            </button>
          )}

          <button
            type="button"
            className="antigravity-auto-resume-btn-icon"
            onClick={() => void loadData(false)}
            disabled={loading}
            title={`${t('common.refresh', '刷新扫描')} (${t('antigravity.autoResume.autoUpdateInterval', '每 1 分钟自动更新')} · ${t('antigravity.autoResume.lastUpdated', '上次更新: {{time}}', { time: lastUpdated.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' }) })})`}
          >
            <RefreshCw size={14} className={loading ? 'spin' : undefined} />
          </button>

          {!isSettings && (
            <button
              type="button"
              className="antigravity-auto-resume-btn-icon"
              onClick={toggleCollapse}
              title={
                isCollapsed
                  ? t('antigravity.autoResume.expand', '展开')
                  : t('antigravity.autoResume.collapse', '收起')
              }
            >
              {isCollapsed ? <ChevronDown size={14} /> : <ChevronUp size={14} />}
            </button>
          )}
        </div>
      </div>

      {/* Settings bar for quick inline management */}
      {!effectiveCollapsed && (
        <div className="antigravity-auto-resume-settings-bar">
          <div className="antigravity-auto-resume-setting-item">
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
              {t('antigravity.autoResume.enableTitle', '受限中断任务自动恢复')}
            </span>
          </div>

          <div className="antigravity-auto-resume-setting-item">
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
              {t('antigravity.autoResume.syncSshTitle', '包含远程 SSH 服务器')}
            </span>
          </div>
        </div>
      )}

      {/* Alert message if any */}
      {actionMessage && (
        <div className="antigravity-auto-resume-alert success">
          <CheckCircle size={14} />
          <span>{actionMessage}</span>
        </div>
      )}

      {error && (
        <div className="antigravity-auto-resume-alert error">
          <AlertTriangle size={14} />
          <span>{error}</span>
        </div>
      )}

      {/* Body content */}
      {!isCollapsed && (
        <div className="antigravity-auto-resume-body">
          {loading && interruptedTasks.length === 0 && resumedTasks.length === 0 ? (
            <div className="antigravity-auto-resume-empty">
              <RefreshCw size={24} className="spin antigravity-auto-resume-empty-icon" />
              <div className="antigravity-auto-resume-empty-title">
                {t('antigravity.autoResume.scanning', '正在扫描本地及远程 SSH 服务器的中断任务...')}
              </div>
            </div>
          ) : activeTab === 'interrupted' ? (
            interruptedTasks.length === 0 ? (
              <div className="antigravity-auto-resume-empty">
                <CheckCircle size={28} className="antigravity-auto-resume-empty-icon" />
                <div className="antigravity-auto-resume-empty-title">
                  {t('antigravity.autoResume.emptyTitle', '当前无受限中断的任务')}
                </div>
                <div className="antigravity-auto-resume-empty-desc">
                  {t(
                    'antigravity.autoResume.emptyDesc',
                    '本地及已配置的 SSH 服务器上没有处于配额受限中断状态的 Antigravity 任务。若后续有任务达到配额中断，将在此处显示并自动排队等待恢复。',
                  )}
                </div>
              </div>
            ) : (
              interruptedTasks.map((task) => {
                const countdown = formatCountdown(task.resets_at);
                const resetTimeStr = formatResetTime(task.resets_at);
                const isResuming = resumingTaskId === task.id;

                return (
                  <div key={task.id} className="antigravity-task-card">
                    <div className="antigravity-task-card__header">
                      <div className="antigravity-task-card__tags">
                        {task.source === 'local' ? (
                          <span className="antigravity-task-tag antigravity-task-tag--source-local">
                            <Laptop size={12} />
                            {t('antigravity.autoResume.localMachine', '本地')}
                          </span>
                        ) : (
                          <span className="antigravity-task-tag antigravity-task-tag--source-ssh">
                            <Server size={12} />
                            {task.server_name || t('antigravity.autoResume.sshServer', 'SSH 服务器')}
                          </span>
                        )}

                        <span className="antigravity-task-tag antigravity-task-tag--task">
                          📝 {t('antigravity.autoResume.taskType', '会话任务')}
                        </span>

                        {task.status === 'blocked' ? (
                          <span className="antigravity-task-tag antigravity-task-tag--status-blocked">
                            ⏸️ {t('antigravity.autoResume.statusBlocked', '已暂停/中断')}
                          </span>
                        ) : task.can_resume ? (
                          <span className="antigravity-task-tag antigravity-task-tag--status-ready">
                            ✨ {t('antigravity.autoResume.statusReady', '可恢复')}
                          </span>
                        ) : (
                          <span className="antigravity-task-tag antigravity-task-tag--status-waiting">
                            ⏳ {t('antigravity.autoResume.statusWaiting', '等待配额重置')}
                          </span>
                        )}

                        {task.is_excluded && (
                          <span
                            className="antigravity-task-tag antigravity-task-tag--excluded"
                            title={t('antigravity.autoResume.excludedBadgeDesc', '此任务已勾选排除，即使配额恢复也不会自动恢复')}
                          >
                            🚫 {t('antigravity.autoResume.excludedBadge', '已排除自动恢复')}
                          </span>
                        )}
                      </div>

                      <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
                        <label
                          className={`antigravity-task-auto-resume-checkbox ${task.is_excluded ? 'is-excluded' : ''}`}
                          title={t('antigravity.autoResume.taskCheckboxTooltip', '取消勾选将不自动恢复此任务')}
                        >
                          <input
                            type="checkbox"
                            checked={!task.is_excluded}
                            onChange={(e) => void handleToggleTaskExcluded(task.id, !e.target.checked)}
                          />
                          <span>{t('antigravity.autoResume.autoResumeCheckbox', '自动恢复此任务')}</span>
                        </label>
                      </div>
                    </div>

                    <div className="antigravity-task-card__title" title={task.title}>
                      {task.title}
                    </div>

                    <div className="antigravity-task-card__meta">
                      {task.cwd && (
                        <span className="antigravity-task-card__meta-item" title={task.cwd}>
                          <Folder size={12} />
                          {task.cwd}
                        </span>
                      )}
                      <span className="antigravity-task-card__meta-item">
                        <Clock size={12} />
                        {t('antigravity.autoResume.lastActivity', '最近活动: {{time}}', {
                          time: formatDateTime(task.updated_at),
                        })}
                      </span>
                    </div>

                    {task.error_message && (
                      <div className="antigravity-task-card__error-box">
                        <AlertTriangle size={12} style={{ flexShrink: 0 }} />
                        <span>{task.error_message}</span>
                      </div>
                    )}

                    <div className="antigravity-task-card__reset-box">
                      <div className="antigravity-task-card__reset-info">
                        {task.status === 'blocked' ? (
                          <div className="antigravity-task-card__reset-countdown can-resume-text">
                            ✨ {t('antigravity.autoResume.canResumeNow', '任务未受额度硬性拦截，可立即尝试恢复')}
                          </div>
                        ) : task.resets_at ? (
                          <>
                            <div
                              className={`antigravity-task-card__reset-countdown ${
                                task.can_resume ? 'can-resume-text' : ''
                              }`}
                            >
                              <Clock size={13} />
                              <span>{countdown}</span>
                            </div>
                            <div className="antigravity-task-card__reset-time">
                              {t('antigravity.autoResume.expectedResetTime', '预计重置时间: {{time}}', {
                                time: resetTimeStr,
                              })}
                            </div>
                          </>
                        ) : (
                          <div className="antigravity-task-card__reset-countdown can-resume-text">
                            ✨ {t('antigravity.autoResume.canResumeNow', '任务未受额度硬性拦截，可立即尝试恢复')}
                          </div>
                        )}
                      </div>

                      <div className="antigravity-task-card__actions">
                        <button
                          type="button"
                          className="btn-resume"
                          disabled={isResuming}
                          onClick={() => void handleResumeTask(task)}
                        >
                          {isResuming ? (
                            <>
                              <RefreshCw size={13} className="spin" />
                              <span>{t('antigravity.autoResume.resuming', '正在恢复...')}</span>
                            </>
                          ) : (
                            <>
                              <Play size={13} />
                              <span>{t('antigravity.autoResume.resumeNow', '立即恢复')}</span>
                            </>
                          )}
                        </button>
                      </div>
                    </div>
                  </div>
                );
              })
            )
          ) : resumedTasks.length === 0 ? (
            <div className="antigravity-auto-resume-empty">
              <CheckCircle size={28} className="antigravity-auto-resume-empty-icon" />
              <div className="antigravity-auto-resume-empty-title">
                {t('antigravity.autoResume.noResumedTasks', '暂无恢复历史记录')}
              </div>
              <div className="antigravity-auto-resume-empty-desc">
                {t(
                  'antigravity.autoResume.noResumedTasksDesc',
                  '当后台检测到配额满足并成功恢复受限中断任务后，恢复的详细记录将保存在此处。',
                )}
              </div>
            </div>
          ) : (
            <div className="antigravity-resumed-history-list">
              {resumedTasks.map((rec) => (
                <div key={`${rec.id}-${rec.resumed_at}`} className="antigravity-resumed-item">
                  <div className="antigravity-resumed-item-top">
                    <div className="antigravity-task-card__tags">
                      {rec.source === 'local' ? (
                        <span className="antigravity-task-tag antigravity-task-tag--source-local">
                          <Laptop size={11} />
                          {t('antigravity.autoResume.localMachine', '本地')}
                        </span>
                      ) : (
                        <span className="antigravity-task-tag antigravity-task-tag--source-ssh">
                          <Server size={11} />
                          {rec.server_name || t('antigravity.autoResume.sshServer', 'SSH 服务器')}
                        </span>
                      )}

                      <span className="antigravity-task-tag antigravity-task-tag--task">
                        📝 {t('antigravity.autoResume.taskType', '会话任务')}
                      </span>

                      {rec.trigger_type === 'auto' ? (
                        <span className="antigravity-task-tag antigravity-task-tag--trigger-auto">
                          ⚡ {t('antigravity.autoResume.triggerAuto', '自动唤醒恢复')}
                        </span>
                      ) : (
                        <span className="antigravity-task-tag antigravity-task-tag--trigger-manual">
                          👆 {t('antigravity.autoResume.triggerManual', '手动立即恢复')}
                        </span>
                      )}

                      {rec.success ? (
                        <span className="antigravity-task-tag antigravity-task-tag--status-ready">
                          <CheckCircle size={11} />
                          {t('antigravity.autoResume.resumedSuccess', '恢复成功')}
                        </span>
                      ) : (
                        <span className="antigravity-task-tag antigravity-task-tag--status-blocked">
                          <XCircle size={11} />
                          {t('antigravity.autoResume.resumedFailed', '恢复失败')}
                        </span>
                      )}
                    </div>

                    <div className="antigravity-resumed-item-time">
                      <Clock size={12} />
                      <span>{formatDateTime(rec.resumed_at)}</span>
                    </div>
                  </div>

                  <div className="antigravity-task-card__title">{rec.title}</div>

                  {rec.cwd && (
                    <div className="antigravity-task-card__meta">
                      <span className="antigravity-task-card__meta-item" title={rec.cwd}>
                        <Folder size={12} />
                        {rec.cwd}
                      </span>
                    </div>
                  )}

                  {rec.message && (
                    <div
                      className={`antigravity-resumed-item-msg ${
                        rec.success ? '' : 'error'
                      }`}
                    >
                      {rec.message}
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
