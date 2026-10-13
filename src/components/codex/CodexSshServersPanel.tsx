import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import {
  Check,
  KeyRound,
  Pencil,
  PlugZap,
  RefreshCw,
  Server,
  Trash2,
  X,
} from 'lucide-react';
import { listSshConfigHosts } from '../../services/sshServerService';
import { useSshServerStore } from '../../stores/useSshServerStore';
import type {
  SshAuthConfig,
  SshCodexSyncResult,
  SshServer,
  SshServerDraft,
} from '../../types/sshServer';

interface FormState {
  id?: string;
  name: string;
  host: string;
  port: string;
  username: string;
  codexHome: string;
  antigravityHome: string;
  authKind: 'agent' | 'private_key_file' | 'ssh_config';
  privateKeyPath: string;
  syncOnSwitch: boolean;
  syncOnAntigravitySwitch: boolean;
}

const emptyForm: FormState = {
  name: '',
  host: '',
  port: '22',
  username: '',
  codexHome: '~/.codex',
  antigravityHome: '~/.gemini',
  authKind: 'agent',
  privateKeyPath: '',
  syncOnSwitch: true,
  syncOnAntigravitySwitch: false,
};

function formFromServer(server: SshServer): FormState {
  return {
    id: server.id,
    name: server.name,
    host: server.host,
    port: String(server.port || 22),
    username: server.username,
    codexHome: server.codex_home || '~/.codex',
    antigravityHome: server.antigravity_home || '~/.gemini',
    authKind: server.auth.kind,
    privateKeyPath: server.auth.kind === 'private_key_file' ? server.auth.path : '',
    syncOnSwitch: server.sync_on_codex_switch,
    syncOnAntigravitySwitch: server.sync_on_antigravity_switch ?? false,
  };
}

function draftFromForm(form: FormState): SshServerDraft {
  const auth: SshAuthConfig =
    form.authKind === 'private_key_file'
      ? { kind: 'private_key_file', path: form.privateKeyPath.trim() }
      : { kind: form.authKind };
  return {
    id: form.id,
    name: form.name.trim() || (form.authKind === 'ssh_config' ? form.host.trim() : ''),
    host: form.host.trim(),
    port: form.authKind === 'ssh_config' ? 22 : Number.parseInt(form.port, 10) || 22,
    username: form.authKind === 'ssh_config' ? '' : form.username.trim(),
    codex_home: form.codexHome.trim() || '~/.codex',
    antigravity_home: form.antigravityHome.trim() || '~/.gemini',
    auth,
    sync_on_codex_switch: form.syncOnSwitch,
    sync_on_antigravity_switch: form.syncOnAntigravitySwitch,
  };
}

function formatSyncTime(timestamp?: number) {
  if (!timestamp) return '';
  return new Date(timestamp * 1000).toLocaleString();
}

interface CodexSshServersPanelProps {
  /** 嵌入设置弹框时隐藏顶部大标题区 */
  embedded?: boolean;
  platform?: "codex" | "antigravity";
}

export function CodexSshServersPanel({ embedded = false, platform = "codex" }: CodexSshServersPanelProps) {
  const { t } = useTranslation();
  const {
    servers,
    selectedServerId: codexSelectedId,
    selectedAntigravityServerId,
    loading,
    error,
    lastSyncResult,
    fetchServers,
    upsertServer,
    deleteServer,
    selectServer: selectPlatformServer,
    testConnection,
    syncNow,
    syncAntigravityNow,
    applySyncResult,
  } = useSshServerStore();
  const selectedServerId = platform === "antigravity" ? selectedAntigravityServerId : codexSelectedId;
  const selectServer = (id: string | null) => selectPlatformServer(id, platform);
  const platformEmptyForm = { ...emptyForm, syncOnSwitch: platform === "codex", syncOnAntigravitySwitch: platform === "antigravity" };
  const [form, setForm] = useState<FormState>(platformEmptyForm);
  const [configHosts, setConfigHosts] = useState<string[]>([]);
  const [customAlias, setCustomAlias] = useState(false);
  const [saving, setSaving] = useState(false);
  const [busyServerId, setBusyServerId] = useState<string | null>(null);
  const [localMessage, setLocalMessage] = useState<{
    kind: 'success' | 'warning' | 'error';
    text: string;
  } | null>(null);

  const selectedServer = useMemo(
    () => servers.find((server) => server.id === selectedServerId) ?? null,
    [servers, selectedServerId],
  );

  useEffect(() => {
    void fetchServers();
    let disposed = false;
    void listSshConfigHosts().then((hosts) => {
      if (!disposed) setConfigHosts(hosts);
    }).catch((error: unknown) => {
      if (!disposed) setLocalMessage({ kind: 'error', text: String(error) });
    });
    return () => { disposed = true; };
  }, [fetchServers]);

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    listen<SshCodexSyncResult>('codex:ssh-sync-result', (event) => {
      applySyncResult(event.payload);
      setLocalMessage({
        kind: event.payload.verified ? 'success' : 'warning',
        text: event.payload.verified
          ? t('codex.ssh.syncVerified', 'SSH 已校验同步成功')
          : event.payload.error ??
            t('codex.ssh.syncFailed', '本地切号成功，但 SSH 同步失败'),
      });
    }).then((dispose) => {
      if (disposed) {
        dispose();
        return;
      }
      unlisten = dispose;
    });
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [applySyncResult, t]);

  const handleSubmit = async () => {
    setSaving(true);
    setLocalMessage(null);
    try {
      const draft = draftFromForm(form);
      await upsertServer(draft);
      // 开启任一切号同步时自动选中该服务器，设置里的开关会点亮
      if (platform === "codex" ? draft.sync_on_codex_switch : draft.sync_on_antigravity_switch) {
        const list = useSshServerStore.getState().servers;
        const matched =
          list.find(
            (server) =>
              server.host === draft.host &&
              server.port === draft.port &&
              server.username === draft.username && server.auth.kind === draft.auth.kind,
          ) ?? list.find((server) => server.id === draft.id);
        if (matched) {
          await selectServer(matched.id);
        }
      }
      setForm(platformEmptyForm);
      setLocalMessage({ kind: 'success', text: t('common.saved', '已保存') });
    } catch (err) {
      setLocalMessage({ kind: 'error', text: String(err) });
    } finally {
      setSaving(false);
    }
  };

  const handleTest = async (serverId: string) => {
    setBusyServerId(serverId);
    setLocalMessage(null);
    try {
      await testConnection(serverId);
      setLocalMessage({
        kind: 'success',
        text: t('codex.ssh.connectionOk', '连接成功'),
      });
    } catch (err) {
      setLocalMessage({ kind: 'error', text: String(err) });
    } finally {
      setBusyServerId(null);
    }
  };

  const handleSync = async (serverId?: string) => {
    setBusyServerId(serverId ?? selectedServerId);
    setLocalMessage(null);
    try {
      const result = await syncNow(serverId);
      setLocalMessage({
        kind: result.verified ? 'success' : 'warning',
        text: result.verified
          ? t('codex.ssh.syncVerified', 'Codex SSH 已校验同步成功')
          : result.error ?? t('codex.ssh.syncFailed', 'Codex SSH 同步失败'),
      });
    } catch (err) {
      setLocalMessage({ kind: 'error', text: String(err) });
    } finally {
      setBusyServerId(null);
    }
  };

  const handleSyncAntigravity = async (serverId?: string) => {
    setBusyServerId(serverId ?? selectedServerId);
    setLocalMessage(null);
    try {
      const result = await syncAntigravityNow(serverId);
      setLocalMessage({
        kind: result.verified ? 'success' : 'warning',
        text: result.verified
          ? t('antigravity.ssh.syncVerified', 'Antigravity SSH 已校验同步成功')
          : result.error ?? t('antigravity.ssh.syncFailed', 'Antigravity SSH 同步失败'),
      });
    } catch (err) {
      setLocalMessage({ kind: 'error', text: String(err) });
    } finally {
      setBusyServerId(null);
    }
  };

  return (
    <div className={`codex-ssh-panel${embedded ? ' codex-ssh-panel--embedded' : ''}`}>
      {!embedded && (
        <div className="codex-ssh-panel__header">
          <div className="codex-ssh-panel__header-copy">
            <h2 className="codex-ssh-panel__title">
              {t('codex.ssh.title', 'SSH 服务器')}
            </h2>
            <p className="codex-ssh-panel__subtitle">
              {t(
                'codex.ssh.subtitle',
                '选中的服务器会在 Codex 切号后接收当前账号鉴权包。',
              )}
            </p>
          </div>
          <button
            className="btn btn-secondary"
            type="button"
            onClick={() => void fetchServers()}
            disabled={loading}
          >
            <RefreshCw size={14} className={loading ? 'spin' : undefined} />
            <span>{t('common.refresh', '刷新')}</span>
          </button>
        </div>
      )}

      {(error || localMessage) && (
        <div
          className={`codex-ssh-panel__message codex-ssh-panel__message--${localMessage?.kind ?? 'error'}`}
          role="status"
        >
          {localMessage?.text ?? error}
        </div>
      )}

      <div className="codex-ssh-panel__layout">
        <form
          className="codex-ssh-panel__form"
          onSubmit={(event) => {
            event.preventDefault();
            void handleSubmit();
          }}
        >
          <div className="codex-ssh-panel__form-header">
            <div>
              <div className="codex-ssh-panel__section-kicker">
                {form.id
                  ? t('codex.ssh.editServer', '编辑服务器')
                  : t('codex.ssh.addServer', '添加服务器')}
              </div>
              <h3 className="codex-ssh-panel__form-title">
                {t('codex.ssh.formLead', '填写远端主机信息')}
              </h3>
            </div>
            <div className="codex-ssh-panel__form-header-actions">
              {embedded && (
                <button
                  className="btn btn-secondary icon-only"
                  type="button"
                  title={t('common.refresh', '刷新')}
                  aria-label={t('common.refresh', '刷新')}
                  onClick={() => void fetchServers()}
                  disabled={loading}
                >
                  <RefreshCw size={14} className={loading ? 'spin' : undefined} />
                </button>
              )}
              {form.id && (
                <button
                  className="btn btn-secondary icon-only"
                  type="button"
                  title={t('common.cancel', '取消')}
                  aria-label={t('common.cancel', '取消')}
                  onClick={() => setForm(platformEmptyForm)}
                >
                  <X size={14} />
                </button>
              )}
            </div>
          </div>

          <div className="codex-ssh-panel__fields">
            <label className="codex-ssh-panel__field">
              <span>{t('codex.ssh.connectionSettings', '连接设置')}</span>
              <select className="form-input" value={form.authKind === 'ssh_config' ? 'ssh_config' : 'manual'}
                onChange={(event) => {
                  setCustomAlias(false);
                  setForm({ ...form, authKind: event.target.value === 'ssh_config' ? 'ssh_config' : 'agent' });
                }}>
                <option value="manual">{t('codex.ssh.manualSettings', '手动填写')}</option>
                <option value="ssh_config">{t('codex.ssh.configSettings', '使用 SSH config')}</option>
              </select>
              {form.authKind === 'ssh_config' && <small>{t('codex.ssh.configSettingsHint', '填写 ~/.ssh/config 中的 Host 别名，自动使用主机、用户名、端口、密钥和跳板机设置。')}</small>}
            </label>

            <label className="codex-ssh-panel__field">
              <span>{t('codex.ssh.name', '名称')}</span>
              <input
                className="form-input"
                value={form.name}
                onChange={(event) => setForm({ ...form, name: event.target.value })}
                placeholder={form.authKind === 'ssh_config' ? t('codex.ssh.aliasNamePlaceholder', '默认使用 Host 别名') : t('codex.ssh.namePlaceholder', '例如：家里 NAS')}
                required={form.authKind !== 'ssh_config'}
              />
            </label>

            <div className={form.authKind === 'ssh_config' ? undefined : 'codex-ssh-panel__field-grid codex-ssh-panel__field-grid--host'}>
              <label className="codex-ssh-panel__field">
                <span>{form.authKind === 'ssh_config' ? t('codex.ssh.configAlias', 'SSH Host 别名') : t('codex.ssh.host', '主机')}</span>
                {form.authKind === 'ssh_config' && (
                  <>
                    <select className="form-input" value={customAlias ? '-manual-' : form.host}
                      required={!customAlias}
                      onChange={(event) => {
                        const alias = event.target.value;
                        setCustomAlias(alias === '-manual-');
                        if (alias !== '-manual-') setForm({ ...form, host: alias });
                      }}>
                      <option value="">{t('codex.ssh.chooseConfigHost', '选择 SSH config 主机')}</option>
                      {form.host && !configHosts.includes(form.host) && <option value={form.host}>{form.host}</option>}
                      {configHosts.map((alias) => <option key={alias} value={alias}>{alias}</option>)}
                      <option value="-manual-">{t('codex.ssh.enterConfigAlias', '手动输入其他别名…')}</option>
                    </select>
                    <button className="btn btn-secondary" type="button" onClick={() => {
                      void listSshConfigHosts().then(setConfigHosts).catch((error: unknown) =>
                        setLocalMessage({ kind: 'error', text: String(error) }));
                    }}>{t('codex.ssh.refreshConfigHosts', '刷新 SSH config 主机')}</button>
                  </>
                )}
                {(form.authKind !== 'ssh_config' || customAlias) && <input
                  className="form-input"
                  value={form.host}
                  onChange={(event) => setForm({ ...form, host: event.target.value })}
                  placeholder={form.authKind === 'ssh_config' ? 'macbookair' : '192.168.1.10'}
                  required
                />}
              </label>
              {form.authKind !== 'ssh_config' && <label className="codex-ssh-panel__field">
                <span>{t('codex.ssh.port', '端口')}</span>
                <input
                  className="form-input"
                  inputMode="numeric"
                  value={form.port}
                  onChange={(event) => setForm({ ...form, port: event.target.value })}
                  required
                />
              </label>}
            </div>

            <div className={form.authKind === 'ssh_config' ? undefined : 'codex-ssh-panel__field-grid'}>
              {form.authKind !== 'ssh_config' && <label className="codex-ssh-panel__field">
                <span>{t('codex.ssh.username', '用户名')}</span>
                <input
                  className="form-input"
                  value={form.username}
                  onChange={(event) =>
                    setForm({ ...form, username: event.target.value })
                  }
                  placeholder="ubuntu"
                  required
                />
              </label>}
              <label className="codex-ssh-panel__field">
                <span>{t('codex.ssh.codexHome', '远端 Codex 目录')}</span>
                <input
                  className="form-input"
                  value={form.codexHome}
                  onChange={(event) =>
                    setForm({ ...form, codexHome: event.target.value })
                  }
                  placeholder="~/.codex"
                />
              </label>
            </div>

            <label className="codex-ssh-panel__field">
              <span>{t('antigravity.ssh.antigravityHome', '远端 Antigravity 目录')}</span>
              <input
                className="form-input"
                value={form.antigravityHome}
                onChange={(event) =>
                  setForm({ ...form, antigravityHome: event.target.value })
                }
                placeholder="~/.gemini"
              />
            </label>

            {form.authKind !== 'ssh_config' && <div className="codex-ssh-panel__auth">
              <span className="codex-ssh-panel__field-label">
                {t('codex.ssh.authMethod', '认证方式')}
              </span>
              <div
                className="codex-ssh-panel__segmented"
                role="radiogroup"
                aria-label={t('codex.ssh.authMethod', '认证方式')}
              >
                <button
                  type="button"
                  className={form.authKind === 'agent' ? 'active' : undefined}
                  onClick={() => setForm({ ...form, authKind: 'agent' })}
                >
                  {t('codex.ssh.agentAuth', 'SSH Agent')}
                </button>
                <button
                  type="button"
                  className={
                    form.authKind === 'private_key_file' ? 'active' : undefined
                  }
                  onClick={() =>
                    setForm({ ...form, authKind: 'private_key_file' })
                  }
                >
                  {t('codex.ssh.privateKeyAuth', '私钥文件')}
                </button>
              </div>
            </div>}

            {form.authKind === 'private_key_file' && (
              <label className="codex-ssh-panel__field">
                <span>{t('codex.ssh.privateKeyPath', '私钥路径')}</span>
                <input
                  className="form-input"
                  value={form.privateKeyPath}
                  onChange={(event) =>
                    setForm({ ...form, privateKeyPath: event.target.value })
                  }
                  placeholder="~/.ssh/id_ed25519"
                  required
                />
              </label>
            )}

            <label className="codex-ssh-panel__switch-card">
              <span className="codex-ssh-panel__switch-copy">
                <strong>{t('codex.ssh.syncOnSwitch', 'Codex 切号后自动同步')}</strong>
                <em>
                  {t(
                    'codex.ssh.syncOnSwitchHint',
                    '保存并选中后，Codex 切号会推送鉴权到这台主机',
                  )}
                </em>
              </span>
              <span className="codex-ssh-panel__switch">
                <input
                  type="checkbox"
                  checked={form.syncOnSwitch}
                  onChange={(event) =>
                    setForm({ ...form, syncOnSwitch: event.target.checked })
                  }
                />
                <span className="codex-ssh-panel__switch-slider" />
              </span>
            </label>

            <label className="codex-ssh-panel__switch-card">
              <span className="codex-ssh-panel__switch-copy">
                <strong>{t('antigravity.ssh.syncOnSwitch', 'Antigravity 切号后自动同步')}</strong>
                <em>
                  {t(
                    'antigravity.ssh.syncOnSwitchHint',
                    '保存并选中后，Antigravity 切号会推送鉴权到这台主机',
                  )}
                </em>
              </span>
              <span className="codex-ssh-panel__switch">
                <input
                  type="checkbox"
                  checked={form.syncOnAntigravitySwitch}
                  onChange={(event) =>
                    setForm({ ...form, syncOnAntigravitySwitch: event.target.checked })
                  }
                />
                <span className="codex-ssh-panel__switch-slider" />
              </span>
            </label>
          </div>

          <div className="codex-ssh-panel__form-actions">
            {form.id && (
              <button
                className="btn btn-secondary"
                type="button"
                onClick={() => setForm(platformEmptyForm)}
              >
                {t('common.cancel', '取消')}
              </button>
            )}
            <button className="btn btn-primary" type="submit" disabled={saving}>
              <Check size={14} />
              <span>
                {form.id
                  ? t('common.save', '保存')
                  : t('codex.ssh.addAndSave', '添加并保存')}
              </span>
            </button>
          </div>
        </form>

        <section className="codex-ssh-panel__list-wrap">
          <div className="codex-ssh-panel__list-header">
            <div>
              <div className="codex-ssh-panel__section-kicker">
                {t('codex.ssh.serverList', '服务器列表')}
              </div>
              <h3 className="codex-ssh-panel__list-title">
                {t('codex.ssh.serverListLead', '选择同步目标')}
              </h3>
            </div>
            <span className="codex-ssh-panel__count">
              {t('codex.ssh.serverCount', '{{count}} 台', {
                count: servers.length,
              })}
            </span>
          </div>

          <div className="codex-ssh-panel__list">
            {servers.length === 0 && (
              <div className="codex-ssh-panel__empty">
                <div className="codex-ssh-panel__empty-icon" aria-hidden="true">
                  <Server size={22} />
                </div>
                <h3>{t('codex.ssh.emptyTitle', '暂无 SSH 服务器')}</h3>
                <p>
                  {t(
                    'codex.ssh.emptyBody',
                    '在左侧填写主机信息并保存，即可作为切号同步目标。',
                  )}
                </p>
              </div>
            )}

            {servers.map((server) => {
              const isSelected = server.id === selectedServerId;
              const sync = server.last_sync;
              const busy = busyServerId === server.id;
              return (
                <article
                  className={`codex-ssh-panel__item${isSelected ? ' is-selected' : ''}`}
                  key={server.id}
                >
                  <button
                    type="button"
                    className="codex-ssh-panel__item-main"
                    onClick={() => void selectServer(isSelected ? null : server.id)}
                  >
                    <div className="codex-ssh-panel__item-top">
                      <div className="codex-ssh-panel__item-avatar" aria-hidden="true">
                        <Server size={16} />
                      </div>
                      <div className="codex-ssh-panel__item-copy">
                        <div className="codex-ssh-panel__item-title">
                          <strong>{server.name}</strong>
                          {isSelected && (
                            <span className="codex-ssh-panel__badge">
                              {t('codex.ssh.selected', '已选中')}
                            </span>
                          )}
                          {server.sync_on_codex_switch && (
                            <span className="codex-ssh-panel__badge codex-ssh-panel__badge--soft">
                              Codex 同步
                            </span>
                          )}
                          {server.sync_on_antigravity_switch && (
                            <span className="codex-ssh-panel__badge codex-ssh-panel__badge--soft">
                              Antigravity 同步
                            </span>
                          )}
                        </div>
                        <div className="codex-ssh-panel__item-meta">
                          <code>
                            {server.auth.kind === 'ssh_config' ? server.host : `${server.username}@${server.host}:${server.port}`}
                          </code>
                          <span className="codex-ssh-panel__dot">·</span>
                          <span>{server.codex_home}</span>
                          <span className="codex-ssh-panel__dot">·</span>
                          <span>{server.antigravity_home || '~/.gemini'}</span>
                        </div>
                      </div>
                    </div>

                    <div className="codex-ssh-panel__sync-status-group" style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                      <div
                        className={`codex-ssh-panel__sync-status${
                          sync?.verified ? ' is-ok' : sync ? ' is-fail' : ''
                        }`}
                      >
                        <span style={{ opacity: 0.75, marginRight: 4 }}>[Codex]</span>
                        {sync
                          ? sync.verified
                            ? t('codex.ssh.syncedAs', '已同步 {{email}} · {{time}}', {
                                email: sync.account_email,
                                time: formatSyncTime(sync.synced_at),
                              })
                            : sync.error ??
                              t('codex.ssh.syncFailed', 'SSH 同步失败')
                          : t('codex.ssh.neverSynced', '尚未同步')}
                      </div>
                      {server.last_antigravity_sync && (
                        <div
                          className={`codex-ssh-panel__sync-status${
                            server.last_antigravity_sync.verified ? ' is-ok' : ' is-fail'
                          }`}
                        >
                          <span style={{ opacity: 0.75, marginRight: 4 }}>[Antigravity]</span>
                          {server.last_antigravity_sync.verified
                            ? t('antigravity.ssh.syncedAs', '已同步 {{email}} · {{time}}', {
                                email: server.last_antigravity_sync.account_email,
                                time: formatSyncTime(server.last_antigravity_sync.synced_at),
                              })
                            : server.last_antigravity_sync.error ??
                              t('antigravity.ssh.syncFailed', 'SSH 同步失败')}
                        </div>
                      )}
                    </div>
                  </button>

                  <div className="codex-ssh-panel__item-actions">
                    <button
                      className={`btn btn-secondary${isSelected ? ' is-active' : ''}`}
                      type="button"
                      disabled={busy}
                      onClick={() =>
                        void selectServer(isSelected ? null : server.id)
                      }
                    >
                      <Check size={14} />
                      <span>
                        {isSelected
                          ? t('codex.ssh.selected', '已选中')
                          : t('codex.ssh.select', '选中')}
                      </span>
                    </button>
                    <button
                      className="btn btn-secondary"
                      type="button"
                      disabled={busy}
                      onClick={() => void handleTest(server.id)}
                    >
                      <PlugZap size={14} />
                      <span>{t('codex.ssh.testConnection', '测连')}</span>
                    </button>
                    <button
                      className="btn btn-secondary"
                      type="button"
                      disabled={busy}
                      title={t('codex.ssh.syncCodex', '同步当前 Codex 账号')}
                      onClick={() => void handleSync(server.id)}
                    >
                      <KeyRound size={14} />
                      <span>{t('codex.ssh.syncCodexShort', '同步 Codex')}</span>
                    </button>
                    <button
                      className="btn btn-secondary"
                      type="button"
                      disabled={busy}
                      title={t('antigravity.ssh.syncAntigravity', '同步当前 Antigravity 账号')}
                      onClick={() => void handleSyncAntigravity(server.id)}
                    >
                      <KeyRound size={14} />
                      <span>{t('antigravity.ssh.syncAntigravityShort', '同步 Antigravity')}</span>
                    </button>
                    <button
                      className="btn btn-secondary icon-only"
                      type="button"
                      title={t('common.edit', '编辑')}
                      aria-label={t('common.edit', '编辑')}
                      onClick={() => setForm(formFromServer(server))}
                    >
                      <Pencil size={14} />
                    </button>
                    <button
                      className="btn btn-danger icon-only"
                      type="button"
                      title={t('common.delete', '删除')}
                      aria-label={t('common.delete', '删除')}
                      onClick={() => void deleteServer(server.id)}
                    >
                      <Trash2 size={14} />
                    </button>
                  </div>
                </article>
              );
            })}
          </div>
        </section>
      </div>

      {selectedServer && lastSyncResult?.server_id === selectedServer.id && (
        <div
          className={`codex-ssh-panel__footer${
            lastSyncResult.verified ? ' is-ok' : ' is-fail'
          }`}
        >
          {lastSyncResult.verified
            ? t('codex.ssh.latestVerified', '最近一次 SSH 同步已校验通过')
            : lastSyncResult.error ??
              t('codex.ssh.syncFailed', 'SSH 同步失败')}
        </div>
      )}
    </div>
  );
}
