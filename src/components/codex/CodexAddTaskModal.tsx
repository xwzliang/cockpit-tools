import { useState, useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import {
  X,
  Plus,
  Save,
  Folder,
  FolderOpen,
  Laptop,
  Server,
  MessageSquare,
  Sparkles,
  AlertTriangle,
  Loader2,
} from 'lucide-react';
import { listSshServers } from '../../services/sshServerService';
import type { SshServer } from '../../types/sshServer';
import {
  listCodexProjects,
  listCodexThreads,
  addCodexQueuedTask,
  updateCodexQueuedTask,
} from '../../services/codexTaskQueueService';
import type {
  CodexProjectItem,
  CodexQueuedTask,
  CodexThreadSummary,
  CreateQueuedTaskRequest,
  TaskDeliveryMode,
} from '../../types/codexTaskQueue';
import { formatQueueTimeRemaining, parseLocalQueueDateTime, toLocalQueueDateTime } from '../../utils/codexQueueTime';
import './CodexTaskQueueSection.css';

interface CodexAddTaskModalProps {
  isOpen: boolean;
  onClose: () => void;
  onTaskAdded: () => void;
  defaultCwd?: string;
  defaultThreadId?: string;
  taskToEdit?: CodexQueuedTask;
}

const COMMON_MODELS = [
  { value: '', label: '默认模型 (Default)' },
  { value: 'gpt-5.6-luna', label: 'gpt-5.6-luna (Codex 默认)' },
  { value: 'claude-3-7-sonnet', label: 'claude-3-7-sonnet' },
  { value: 'o3-mini', label: 'o3-mini' },
  { value: 'gpt-4o', label: 'gpt-4o' },
  { value: 'gpt-4.5-preview', label: 'gpt-4.5-preview' },
];

export function CodexAddTaskModal({
  isOpen,
  onClose,
  onTaskAdded,
  defaultCwd,
  defaultThreadId,
  taskToEdit,
}: CodexAddTaskModalProps) {
  const { t } = useTranslation();

  const [source, setSource] = useState<'local' | 'ssh'>('local');
  const [sshServers, setSshServers] = useState<SshServer[]>([]);
  const [selectedServerId, setSelectedServerId] = useState<string>('');

  const [projects, setProjects] = useState<CodexProjectItem[]>([]);
  const [loadingProjects, setLoadingProjects] = useState(false);
  const [cwd, setCwd] = useState(defaultCwd || '');
  const [isCustomCwd, setIsCustomCwd] = useState(false);

  const [threads, setThreads] = useState<CodexThreadSummary[]>([]);
  const [loadingThreads, setLoadingThreads] = useState(false);
  const [threadMode, setThreadMode] = useState<'new' | 'existing'>(defaultThreadId ? 'existing' : 'new');
  const [selectedThreadId, setSelectedThreadId] = useState<string>(defaultThreadId || '');

  const [deliveryMode, setDeliveryMode] = useState<TaskDeliveryMode>('wait');

  const [queueDate, setQueueDate] = useState('');
  const [queueTime, setQueueTime] = useState('');
  const [now, setNow] = useState(Date.now());
  const submitLock = useRef(false);
  const [title, setTitle] = useState('');
  const [prompt, setPrompt] = useState('');
  const [model, setModel] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Reset only on opening or selecting another task, never on background queue refresh.
  const initialValues = useRef({ taskToEdit, defaultCwd, defaultThreadId });
  initialValues.current = { taskToEdit, defaultCwd, defaultThreadId };
  useEffect(() => {
    if (!isOpen) return;
    const { taskToEdit: task, defaultCwd: initialCwd, defaultThreadId: initialThread } = initialValues.current;
    setSource(task?.source || 'local');
    setSelectedServerId(task?.server_id || '');
    setCwd(task?.cwd || initialCwd || '');
    setIsCustomCwd(Boolean(task?.cwd || initialCwd));
    setThreadMode(task?.thread_id || initialThread ? 'existing' : 'new');
    setSelectedThreadId(task?.thread_id || initialThread || '');
    setTitle(task?.title || '');
    setPrompt(task?.prompt || '');
    setModel(task?.model || '');
    setDeliveryMode(task?.delivery_mode === 'steer' ? 'steer' : 'wait');
    const schedule = toLocalQueueDateTime(task?.queue_start_at);
    setQueueDate(schedule.date);
    setQueueTime(schedule.time);
    setProjects([]);
    setThreads([]);
    setError(null);
  }, [isOpen, taskToEdit?.id]);

  useEffect(() => {
    if (!isOpen) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen) return;
    let cancelled = false;
    listSshServers().then((res) => {
      if (cancelled) return;
      setSshServers(res.servers);
      setSelectedServerId((current) => current || res.selected_server_id || res.servers[0]?.id || '');
    }).catch((err) => {
      if (!cancelled) console.warn('Failed to load SSH servers:', err);
    });
    return () => { cancelled = true; };
  }, [isOpen, taskToEdit?.id]);

  // Handler for target source switch (local <-> ssh)
  const handleSourceChange = (newSource: 'local' | 'ssh') => {
    if (newSource === source) return;
    setSource(newSource);
    setCwd('');
    setProjects([]);
    setThreads([]);
    setSelectedThreadId('');
    setIsCustomCwd(false);
  };

  // Handler for SSH server switch
  const handleServerChange = (newServerId: string) => {
    if (newServerId === selectedServerId) return;
    setSelectedServerId(newServerId);
    setCwd('');
    setProjects([]);
    setThreads([]);
    setSelectedThreadId('');
    setIsCustomCwd(false);
  };

  // Lists are suggestions: do not replace saved/custom selections when loading completes.
  useEffect(() => {
    if (!isOpen) return;
    let cancelled = false;
    if (source === 'ssh' && !selectedServerId) {
      setProjects([]);
      setLoadingProjects(false);
      return;
    }
    setLoadingProjects(true);
    listCodexProjects(source, source === 'ssh' ? selectedServerId : undefined)
      .then((list) => { if (!cancelled) setProjects(list); })
      .catch((err) => { if (!cancelled) { console.warn('Failed to list projects:', err); setProjects([]); } })
      .finally(() => { if (!cancelled) setLoadingProjects(false); });
    return () => { cancelled = true; };
  }, [isOpen, source, selectedServerId, taskToEdit?.id]);

  useEffect(() => {
    if (!isOpen) return;
    let cancelled = false;
    setThreads([]);
    if (!cwd || (source === 'ssh' && !selectedServerId)) {
      setLoadingThreads(false);
      return;
    }
    setLoadingThreads(true);
    listCodexThreads(source, source === 'ssh' ? selectedServerId : undefined, cwd)
      .then((list) => { if (!cancelled) setThreads(list); })
      .catch((err) => { if (!cancelled) console.warn('Failed to list threads:', err); })
      .finally(() => { if (!cancelled) setLoadingThreads(false); });
    return () => { cancelled = true; };
  }, [isOpen, source, selectedServerId, cwd, taskToEdit?.id]);

  const handleCwdChange = (value: string) => {
    setCwd(value);
    setSelectedThreadId('');
    setThreads([]);
  };

  // Pick folder locally
  const handleBrowseFolder = async () => {
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: t('codexQueue.pickFolder', '选择项目目录'),
      });
      if (selected && typeof selected === 'string') {
        handleCwdChange(selected);
        setIsCustomCwd(true);
      }
    } catch (err) {
      console.warn('Failed to open dialog:', err);
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (submitLock.current) return;
    if (!prompt.trim()) {
      setError(t('codexQueue.promptRequired', '请输入任务提示词'));
      return;
    }
    if (!cwd.trim()) {
      setError(t('codexQueue.cwdRequired', '请选择或输入项目路径'));
      return;
    }

    if (source === 'ssh' && !selectedServerId) {
      setError(t('codexQueue.serverRequired', '请选择 SSH 服务器'));
      return;
    }
    let queueStartAt: number | undefined;
    try {
      const originalSchedule = toLocalQueueDateTime(taskToEdit?.queue_start_at);
      // Preserve an unchanged timestamp even during a repeated DST wall-clock hour.
      queueStartAt = taskToEdit?.queue_start_at != null && queueDate === originalSchedule.date && queueTime === originalSchedule.time
        ? taskToEdit.queue_start_at
        : parseLocalQueueDateTime(queueDate, queueTime);
    } catch {
      setError(t('codexQueue.invalidQueueTime', '请输入有效的本地日期和 24 小时时间（HH:mm 或 HH:mm:ss）'));
      return;
    }
    submitLock.current = true;
    setSubmitting(true);
    setError(null);

    try {
      const activeServer =
        source === 'ssh'
          ? sshServers.find((s) => s.id === selectedServerId)
          : null;

      let chosenThreadId: string | undefined = undefined;
      let chosenThreadTitle: string | undefined = undefined;

      if (threadMode === 'existing') {
        if (!selectedThreadId) {
          setError(t('codexQueue.threadRequired', '请选择要追加的目标会话'));
          return;
        }
        chosenThreadId = selectedThreadId;
        const matchingThread = threads.find((t) => t.id === selectedThreadId);
        chosenThreadTitle = matchingThread?.title || (taskToEdit?.thread_id === selectedThreadId ? taskToEdit.thread_title : undefined);
      }

      const matchingThread =
        threadMode === 'existing' && chosenThreadId
          ? threads.find((t) => t.id === chosenThreadId)
          : undefined;

      // Preserve an edited task's explicit/default model; new tasks retain model inheritance.
      const effectiveModel =
        model.trim() || (!taskToEdit && threadMode === 'existing' ? matchingThread?.model : undefined);

      const req: CreateQueuedTaskRequest = {
        queue_start_at: queueStartAt,
        delivery_mode: threadMode === 'existing' ? deliveryMode : 'wait',
        title: title.trim() || undefined,
        prompt: prompt.trim(),
        source,
        server_id: source === 'ssh' ? selectedServerId : undefined,
        server_name: source === 'ssh' ? (activeServer?.name || (taskToEdit?.server_id === selectedServerId ? taskToEdit.server_name : undefined)) : undefined,
        cwd: cwd.trim(),
        thread_id: chosenThreadId,
        thread_title: chosenThreadTitle,
        model: effectiveModel,
      };

      if (taskToEdit) {
        await updateCodexQueuedTask({ ...taskToEdit, ...req, title: title.trim() });
      } else {
        await addCodexQueuedTask(req);
      }
      onTaskAdded();
      onClose();
    } catch (err) {
      setError(String(err));
    } finally {
      submitLock.current = false;
      setSubmitting(false);
    }
  };

  if (!isOpen) return null;

  const selectedThreadObj = threads.find((t) => t.id === selectedThreadId);
  let schedulePreview: number | undefined;
  try {
    const originalSchedule = toLocalQueueDateTime(taskToEdit?.queue_start_at);
    schedulePreview = taskToEdit?.queue_start_at != null && queueDate === originalSchedule.date && queueTime === originalSchedule.time
      ? taskToEdit.queue_start_at
      : parseLocalQueueDateTime(queueDate, queueTime);
  } catch { /* Validate on save. */ }
  const closeModal = () => { if (!submitLock.current) onClose(); };

  return (
    <div className="modal-overlay codex-queue-modal-overlay" onClick={closeModal}>
      <div
        className="modal-content codex-queue-add-modal"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-header">
          <div className="modal-header-title">
            {taskToEdit ? <Save className="header-icon" /> : <Plus className="header-icon" />}
            <h2>{taskToEdit ? t('codexQueue.editModalTitle', '编辑 Codex 排队任务') : t('codexQueue.addModalTitle', '添加 Codex 排队任务')}</h2>
          </div>
          <button
            className="modal-close"
            onClick={closeModal}
            aria-label={t('common.close', '关闭')}
            disabled={submitting}
          >
            <X />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="modal-body">
          {error && (
            <div className="codex-queue-form-error">
              <AlertTriangle className="error-icon" />
              <span>{error}</span>
            </div>
          )}

          <fieldset disabled={submitting} style={{ border: 0, padding: 0, margin: 0, minWidth: 0, display: 'contents' }}>
          {/* 1. Target Environment */}
          <div className="form-group">
            <label>{t('codexQueue.queueStartAt', '开始排队时间（可选，留空为现在）')}</label>
            <div className="queue-schedule-inputs">
              <input type="date" className="form-input" aria-label={t('codexQueue.queueDate', '排队日期（本地时间）')} value={queueDate} onChange={(e) => setQueueDate(e.target.value)} />
              <input type="text" className="form-input" aria-label={t('codexQueue.queueTime24h', '排队时间（24 小时制）')} placeholder="HH:mm:ss" value={queueTime} onChange={(e) => setQueueTime(e.target.value)} pattern="[0-9]{2}:[0-9]{2}(:[0-9]{2})?" />
            </div>
            <small>{t('codexQueue.queueTimeHint', '本地时间，24 小时制；日期和时间均留空为立即排队。')}</small>
            {schedulePreview != null && (
              <div className="queue-schedule-countdown">
                {schedulePreview * 1000 > now
                  ? t('codexQueue.timeUntilQueue', '距离进入队列还有 {{time}}', { time: formatQueueTimeRemaining(schedulePreview, now) })
                  : t('codexQueue.queueTimeReached', '已到排队时间，等待调度')}
              </div>
            )}
          </div>
          <div className="form-group">
            <label className="form-label">
              {t('codexQueue.executionTarget', '执行环境')}
            </label>
            <div className="target-segmented-control">
              <button
                type="button"
                className={`segmented-btn ${source === 'local' ? 'active' : ''}`}
                onClick={() => handleSourceChange('local')}
              >
                <Laptop className="btn-icon" />
                <span>{t('codexQueue.localMachine', '本机环境')}</span>
              </button>
              <button
                type="button"
                className={`segmented-btn ${source === 'ssh' ? 'active' : ''}`}
                onClick={() => handleSourceChange('ssh')}
              >
                <Server className="btn-icon" />
                <span>{t('codexQueue.sshServer', 'SSH 远程服务器')}</span>
              </button>
            </div>

            {source === 'ssh' && (
              <div className="ssh-server-select-row">
                {sshServers.length === 0 && !selectedServerId ? (
                  <div className="empty-hint">
                    {t(
                      'codexQueue.noSshServers',
                      '尚未配置 SSH 服务器，请先在上方 SSH 面板中添加。'
                    )}
                  </div>
                ) : (
                  <select
                    className="form-select"
                    value={selectedServerId}
                    onChange={(e) => handleServerChange(e.target.value)}
                  >
                    {selectedServerId && !sshServers.some((server) => server.id === selectedServerId) && (
                      <option value={selectedServerId}>{taskToEdit?.server_name || selectedServerId}</option>
                    )}
                    {sshServers.map((s) => (
                      <option key={s.id} value={s.id}>
                        {s.name} ({s.username}@{s.host}:{s.port})
                      </option>
                    ))}
                  </select>
                )}
              </div>
            )}
          </div>

          {/* 2. Project cwd */}
          <div className="form-group">
            <div className="label-with-action">
              <label className="form-label">
                <Folder className="label-icon" />
                {t('codexQueue.projectDirectory', '项目目录 (cwd)')}
              </label>
              {projects.length > 0 && (
                <button
                  type="button"
                  className="text-toggle-btn"
                  onClick={() => setIsCustomCwd(!isCustomCwd)}
                >
                  {isCustomCwd
                    ? t('codexQueue.selectFromRecent', '从最近项目中选择')
                    : t('codexQueue.manualInput', '手动输入路径')}
                </button>
              )}
            </div>

            {!isCustomCwd && projects.length > 0 ? (
              <div className="project-select-wrapper">
                <select
                  className="form-select"
                  value={cwd}
                  onChange={(e) => handleCwdChange(e.target.value)}
                  disabled={loadingProjects}
                >
                  <option value="">{t('codexQueue.selectProject', '请选择项目目录')}</option>
                  {cwd && !projects.some((project) => project.cwd === cwd) && <option value={cwd}>{cwd}</option>}
                  {projects.map((p) => (
                    <option key={p.cwd} value={p.cwd}>
                      {p.name} ({p.thread_count} 个会话) - {p.cwd}
                    </option>
                  ))}
                </select>
              </div>
            ) : (
              <div className="custom-path-input-group">
                <input
                  type="text"
                  className="form-input"
                  placeholder="/Users/.../project"
                  value={cwd}
                  onChange={(e) => handleCwdChange(e.target.value)}
                />
                {source === 'local' && (
                  <button
                    type="button"
                    className="browse-btn"
                    onClick={handleBrowseFolder}
                    title={t('codexQueue.browse', '浏览')}
                  >
                    <FolderOpen className="btn-icon" />
                  </button>
                )}
              </div>
            )}
          </div>

          {/* 3. Thread Selection */}
          <div className="form-group">
            <label className="form-label">
              <MessageSquare className="label-icon" />
              {t('codexQueue.targetThread', '目标会话')}
            </label>
            <div className="target-segmented-control thread-mode-segmented-control">
              <button
                type="button"
                className={`segmented-btn ${threadMode === 'new' ? 'active' : ''}`}
                onClick={() => { setThreadMode('new'); setDeliveryMode('wait'); }}
              >
                <Sparkles className="btn-icon" />
                <span>{t('codexQueue.newThread', '新建独立会话')}</span>
              </button>
              <button
                type="button"
                className={`segmented-btn ${threadMode === 'existing' ? 'active' : ''}`}
                onClick={() => {
                  setThreadMode('existing');
                  if (!selectedThreadId && threads.length > 0) {
                    setSelectedThreadId(threads[0].id);
                  }
                }}
              >
                <MessageSquare className="btn-icon" />
                <span>{t('codexQueue.existingThread', '追加到已有会话')}</span>
              </button>
            </div>

            {threadMode === 'existing' && (
              <div className="existing-threads-picker">
                {loadingThreads ? (
                  <div className="picker-loading">
                    <Loader2 className="spinning-icon" />
                    <span>{t('codexQueue.loadingThreads', '正在加载项目会话...')}</span>
                  </div>
                ) : threads.length === 0 && !selectedThreadId ? (
                  <div className="empty-hint">
                    {t('codexQueue.noExistingThreads', '此项目下未找到已有会话，请选择新建会话或更换项目。')}
                  </div>
                ) : (
                  <>
                    <select
                      className="form-select"
                      value={selectedThreadId}
                      onChange={(e) => setSelectedThreadId(e.target.value)}
                    >
                      <option value="">{t('codexQueue.selectThreadPlaceholder', '-- 请选择已有会话 --')}</option>
                      {selectedThreadId && !threads.some((thread) => thread.id === selectedThreadId) && (
                        <option value={selectedThreadId}>{taskToEdit?.thread_title || selectedThreadId}</option>
                      )}
                      {threads.map((t) => (
                        <option key={t.id} value={t.id}>
                          {t.has_interrupted_task ? '⚠️ [中断任务] ' : ''}
                          {t.title} ({new Date(t.updated_at * 1000).toLocaleDateString()})
                        </option>
                      ))}
                    </select>
                    {selectedThreadObj?.has_interrupted_task && (
                      <div className="thread-interrupted-warning">
                        <AlertTriangle className="warning-icon" />
                        <span>
                          {t(
                            deliveryMode === 'steer' ? 'codexQueue.threadInterruptedSteerNotice' : 'codexQueue.threadInterruptedNotice',
                            deliveryMode === 'steer'
                              ? '提示：中断任务需先恢复。恢复后可向正在执行的轮次发送追加指令。'
                              : '提示：该会话当前存在受限中断任务。排队任务将优先恢复并等待中断任务执行完毕后，再自动提交本任务。'
                          )}
                        </span>
                      </div>
                    )}
                  </>
                )}
              </div>
            )}
          </div>

          {threadMode === 'existing' && (
            <div className="form-group">
              <label className="form-label" htmlFor="codex-queue-delivery-mode">
                {t('codexQueue.deliveryMode', '会话正在执行时')}
              </label>
              <select
                id="codex-queue-delivery-mode"
                className="form-select"
                value={deliveryMode}
                onChange={(e) => setDeliveryMode(e.target.value as TaskDeliveryMode)}
              >
                <option value="wait">{t('codexQueue.deliveryWait', '等待当前任务完成')}</option>
                <option value="steer">{t('codexQueue.deliverySteer', '立即发送到执行中的轮次（Steer）')}</option>
              </select>
              <small>
                {deliveryMode === 'steer'
                  ? t('codexQueue.deliverySteerHint', '到达入队时间且调度开启后，向当前轮次发送追加指令，不等待完成。沿用该轮次的模型和工作目录；下面的模型仅用于启动新轮次。没有执行中轮次时正常启动新轮次。需要连接持有该轮次的 Codex app-server。')
                  : t('codexQueue.deliveryWaitHint', '到达入队时间后，等待当前任务及中断恢复完成，再按队列顺序启动新轮次。')}
              </small>
            </div>
          )}

          {/* 4. Task Prompt */}
          <div className="form-group">
            <label className="form-label required">
              {t('codexQueue.taskPrompt', '任务内容 / 提示词')}
            </label>
            <textarea
              className="form-textarea codex-queue-prompt-area"
              rows={4}
              placeholder={t(
                'codexQueue.promptPlaceholder',
                '请输入交付给 Codex 执行的具体指令或需求...'
              )}
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              required
            />
          </div>

          {/* 5. Title & Model (Advanced / Optional) */}
          <div className="form-row-2">
            <div className="form-group">
              <label className="form-label optional">
                {t('codexQueue.taskTitle', '任务标题 (可选)')}
              </label>
              <input
                type="text"
                className="form-input"
                placeholder={t('codexQueue.titlePlaceholder', '默认自动提取提示词前部')}
                value={title}
                onChange={(e) => setTitle(e.target.value)}
              />
            </div>

            <div className="form-group">
              <label className="form-label optional">
                {t('codexQueue.model', '模型 (可选)')}
              </label>
              <input
                type="text"
                className="form-input"
                list="codex-queue-model-suggestions"
                value={model}
                onChange={(e) => setModel(e.target.value)}
                placeholder={t('codexQueue.defaultModelPlaceholder', '留空使用默认模型，可输入任意模型名称')}
              />
              <datalist id="codex-queue-model-suggestions">
                {COMMON_MODELS.filter((item) => item.value).map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}
              </datalist>
            </div>
          </div>

          </fieldset>
          <div className="modal-footer">
            <button
              type="button"
              className="btn-secondary"
              onClick={closeModal}
              disabled={submitting}
            >
              {t('common.cancel', '取消')}
            </button>
            <button
              type="submit"
              className="btn-primary"
              disabled={submitting || !prompt.trim() || !cwd.trim()}
            >
              {submitting ? (
                <>
                  <Loader2 className="spinning-icon" />
                  <span>{taskToEdit ? t('codexQueue.saving', '正在保存...') : t('codexQueue.adding', '正在加入队列...')}</span>
                </>
              ) : (
                <>
                  {taskToEdit ? <Save className="btn-icon" /> : <Plus className="btn-icon" />}
                  <span>{taskToEdit ? t('codexQueue.saveTask', '保存任务') : t('codexQueue.addToQueue', '加入任务队列')}</span>
                </>
              )}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
