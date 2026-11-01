import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Server, Settings2 } from 'lucide-react';
import { useSshServerStore } from '../../stores/useSshServerStore';
import { CodexSshSyncModal } from './CodexSshSyncModal';

type ControlVariant = 'quick' | 'settings';

interface CodexSshSyncSettingsControlProps {
  variant?: ControlVariant;
}

/**
 * Codex 设置中的「切号同步 SSH」开关 + 配置入口。
 * 打开开关会弹出配置弹框；关闭开关会关闭所有主机的 Codex 自动同步。
 */
export function CodexSshSyncSettingsControl({
  variant = 'settings',
}: CodexSshSyncSettingsControlProps) {
  const { t } = useTranslation();
  const servers = useSshServerStore((s) => s.servers);
  const fetchServers = useSshServerStore((s) => s.fetchServers);
  const upsertServer = useSshServerStore((s) => s.upsertServer);
  const [modalOpen, setModalOpen] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void fetchServers();
  }, [fetchServers]);

  const enabledServers = useMemo(
    () => servers.filter((server) => server.sync_on_codex_switch),
    [servers],
  );
  const enabled = enabledServers.length > 0;

  const openModal = useCallback(() => setModalOpen(true), []);
  const closeModal = useCallback(() => setModalOpen(false), []);

  const handleToggle = async (next: boolean) => {
    if (busy) return;
    if (next) {
      setModalOpen(true);
      return;
    }
    setBusy(true);
    try {
      // Preserve host configuration and the separate Antigravity sync setting.
      for (const server of enabledServers) {
        await upsertServer({ ...server, sync_on_codex_switch: false });
      }
    } catch (error) {
      console.warn('[Codex SSH] disable sync failed:', error);
    } finally {
      setBusy(false);
    }
  };

  const hintText = enabled
    ? t('codex.ssh.syncSwitchActiveHosts', '已启用 · {{names}}', {
        names: enabledServers.map((server) => server.name).join(', '),
      })
    : t('codex.ssh.syncSwitchDesc', 'Codex 切号时同步到所有开启自动同步的 SSH 主机。');

  if (variant === 'quick') {
    return (
      <>
        <div className="qs-row" style={{ marginTop: 8 }}>
          <div className="qs-row-label">
            <Server size={15} />
            <span>{t('codex.ssh.syncSwitch', '切号同步 SSH')}</span>
          </div>
          <div className="qs-row-control" style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
            <button
              type="button"
              className="btn btn-secondary icon-only"
              title={t('codex.ssh.configure', '配置')}
              aria-label={t('codex.ssh.configure', '配置')}
              onClick={openModal}
            >
              <Settings2 size={14} />
            </button>
            <label className="qs-switch">
              <input
                type="checkbox"
                checked={enabled}
                disabled={busy}
                onChange={(e) => void handleToggle(e.target.checked)}
              />
              <span className="qs-switch-slider" />
            </label>
          </div>
        </div>
        <div className="qs-hint">{hintText}</div>
        <CodexSshSyncModal open={modalOpen} onClose={closeModal} />
      </>
    );
  }

  return (
    <>
      <div className="settings-row">
        <div className="row-label">
          <div className="row-title">{t('codex.ssh.syncSwitch', '切号同步 SSH')}</div>
          <div className="row-desc">{hintText}</div>
        </div>
        <div className="row-control" style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <button
            type="button"
            className="btn btn-secondary"
            onClick={openModal}
          >
            <Settings2 size={14} />
            <span>{t('codex.ssh.configure', '配置')}</span>
          </button>
          <label className="switch">
            <input
              type="checkbox"
              checked={enabled}
              disabled={busy}
              onChange={(e) => void handleToggle(e.target.checked)}
            />
            <span className="slider" />
          </label>
        </div>
      </div>
      <CodexSshSyncModal open={modalOpen} onClose={closeModal} />
    </>
  );
}
