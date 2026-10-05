import { useState, useEffect, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import {
  X,
  Plus,
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
} from '../../services/codexTaskQueueService';
import type {
  CodexProjectItem,
  CodexThreadSummary,
  CreateQueuedTaskRequest,
} from '../../types/codexTaskQueue';
import './CodexTaskQueueSection.css';

interface CodexAddTaskModalProps {
  isOpen: boolean;
  onClose: () => void;
  onTaskAdded: () => void;
  defaultCwd?: string;
  defaultThreadId?: string;
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

  const [title, setTitle] = useState('');
  const [prompt, setPrompt] = useState('');
  const [model, setModel] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Load SSH servers list
  useEffect(() => {
    if (!isOpen) return;
    listSshServers()
      .then((res) => {
        setSshServers(res.servers);
        if (res.servers.length > 0 && !selectedServerId) {
          setSelectedServerId(res.selected_server_id || res.servers[0].id);
        }
      })
      .catch((err) => {
        console.warn('Failed to load SSH servers:', err);
      });
  }, [isOpen, selectedServerId]);

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

  // Load projects whenever source or selectedServerId changes
  const loadProjects = useCallback(async () => {
    if (source === 'ssh' && !selectedServerId) {
      setProjects([]);
      return;
    }
    setLoadingProjects(true);
    try {
      const serverId = source === 'ssh' ? selectedServerId : undefined;
      const list = await listCodexProjects(source, serverId);
      setProjects(list);
      setCwd((prevCwd) => {
        if (prevCwd && list.some((p) => p.cwd === prevCwd)) {
          return prevCwd;
        }
        return list.length > 0 ? list[0].cwd : '';
      });
    } catch (err) {
      console.warn('Failed to list projects:', err);
      setProjects([]);
      setCwd('');
    } finally {
      setLoadingProjects(false);
    }
  }, [source, selectedServerId]);

  useEffect(() => {
    if (!isOpen) return;
    loadProjects();
  }, [isOpen, loadProjects]);

  // Load threads whenever cwd changes
  const loadThreads = useCallback(async () => {
    if (!cwd) {
      setThreads([]);
      return;
    }
    setLoadingThreads(true);
    try {
      const serverId = source === 'ssh' ? selectedServerId : undefined;
      const list = await listCodexThreads(source, serverId, cwd);
      setThreads(list);
      setSelectedThreadId((prevId) => {
        if (prevId && list.some((t) => t.id === prevId)) {
          return prevId;
        }
        if (defaultThreadId && list.some((t) => t.id === defaultThreadId)) {
          return defaultThreadId;
        }
        return list.length > 0 ? list[0].id : '';
      });
    } catch (err) {
      console.warn('Failed to list threads:', err);
      setThreads([]);
      setSelectedThreadId('');
    } finally {
      setLoadingThreads(false);
    }
  }, [source, selectedServerId, cwd, defaultThreadId]);

  useEffect(() => {
    if (!isOpen) return;
    loadThreads();
  }, [isOpen, loadThreads]);

  // Pick folder locally
  const handleBrowseFolder = async () => {
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: t('codexQueue.pickFolder', '选择项目目录'),
      });
      if (selected && typeof selected === 'string') {
        setCwd(selected);
        setIsCustomCwd(true);
      }
    } catch (err) {
      console.warn('Failed to open dialog:', err);
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!prompt.trim()) {
      setError(t('codexQueue.promptRequired', '请输入任务提示词'));
      return;
    }
    if (!cwd.trim()) {
      setError(t('codexQueue.cwdRequired', '请选择或输入项目路径'));
      return;
    }

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
        chosenThreadTitle = matchingThread ? matchingThread.title : undefined;
      }

      const matchingThread =
        threadMode === 'existing' && chosenThreadId
          ? threads.find((t) => t.id === chosenThreadId)
          : undefined;

      // Inherit model from existing thread if default model is chosen
      const effectiveModel =
        model.trim() || (threadMode === 'existing' ? matchingThread?.model : undefined);

      const req: CreateQueuedTaskRequest = {
        title: title.trim() || undefined,
        prompt: prompt.trim(),
        source,
        server_id: source === 'ssh' ? selectedServerId : undefined,
        server_name: activeServer ? activeServer.name : undefined,
        cwd: cwd.trim(),
        thread_id: chosenThreadId,
        thread_title: chosenThreadTitle,
        model: effectiveModel,
      };

      await addCodexQueuedTask(req);
      onTaskAdded();
      onClose();
    } catch (err) {
      setError(String(err));
    } finally {
      setSubmitting(false);
    }
  };

  if (!isOpen) return null;

  const selectedThreadObj = threads.find((t) => t.id === selectedThreadId);

  return (
    <div className="modal-overlay codex-queue-modal-overlay" onClick={onClose}>
      <div
        className="modal-content codex-queue-add-modal"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-header">
          <div className="modal-header-title">
            <Plus className="header-icon" />
            <h2>{t('codexQueue.addModalTitle', '添加 Codex 排队任务')}</h2>
          </div>
          <button
            className="modal-close"
            onClick={onClose}
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

          {/* 1. Target Environment */}
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
                {sshServers.length === 0 ? (
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
                  onChange={(e) => setCwd(e.target.value)}
                  disabled={loadingProjects}
                >
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
                  onChange={(e) => setCwd(e.target.value)}
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
                onClick={() => setThreadMode('new')}
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
                ) : threads.length === 0 ? (
                  <div className="empty-hint">
                    {t('codexQueue.noExistingThreads', '此项目下未找到已有会话，将作为新会话启动。')}
                  </div>
                ) : (
                  <>
                    <select
                      className="form-select"
                      value={selectedThreadId}
                      onChange={(e) => setSelectedThreadId(e.target.value)}
                    >
                      <option value="">{t('codexQueue.selectThreadPlaceholder', '-- 请选择已有会话 --')}</option>
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
                            'codexQueue.threadInterruptedNotice',
                            '提示：该会话当前存在受限中断任务。排队任务将优先恢复并等待中断任务执行完毕后，再自动提交本任务。'
                          )}
                        </span>
                      </div>
                    )}
                  </>
                )}
              </div>
            )}
          </div>

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
              <select
                className="form-select"
                value={model}
                onChange={(e) => setModel(e.target.value)}
              >
                <option value="">
                  {threadMode === 'existing' && selectedThreadObj?.model
                    ? `默认模型 (继承会话: ${selectedThreadObj.model})`
                    : '默认模型 (Default)'}
                </option>
                {COMMON_MODELS.filter((m) => m.value !== '').map((m) => (
                  <option key={m.value} value={m.value}>
                    {m.label}
                  </option>
                ))}
              </select>
            </div>
          </div>

          <div className="modal-footer">
            <button
              type="button"
              className="btn-secondary"
              onClick={onClose}
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
                  <span>{t('codexQueue.adding', '正在加入队列...')}</span>
                </>
              ) : (
                <>
                  <Plus className="btn-icon" />
                  <span>{t('codexQueue.addToQueue', '加入任务队列')}</span>
                </>
              )}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
