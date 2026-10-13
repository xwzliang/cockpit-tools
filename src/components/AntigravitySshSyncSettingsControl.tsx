import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Server, Settings2 } from 'lucide-react';
import { useSshServerStore } from '../stores/useSshServerStore';
import { CodexSshSyncModal } from './codex/CodexSshSyncModal';

type ControlVariant = 'quick' | 'settings';

interface AntigravitySshSyncSettingsControlProps {
  variant?: ControlVariant;
}

/**
 * Antigravity 设置中的「切号同步 SSH」开关 + 配置入口。
 * 打开开关会弹出配置弹框；关闭开关会取消该服务器的 Antigravity 自动同步。
 */
export function AntigravitySshSyncSettingsControl({
  variant = 'settings',
}: AntigravitySshSyncSettingsControlProps) {
  const { t } = useTranslation();
  const servers = useSshServerStore((s) => s.servers);
  const selectedServerId = useSshServerStore((s) => s.selectedAntigravityServerId);
  const fetchServers = useSshServerStore((s) => s.fetchServers);
  const upsertServer = useSshServerStore((s) => s.upsertServer);
  const [modalOpen, setModalOpen] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void fetchServers();
  }, [fetchServers]);

  const selectedServer = useMemo(
    () => servers.find((server) => server.id === selectedServerId) ?? null,
    [servers, selectedServerId],
  );

  const enabled = Boolean(
    selectedServer && selectedServer.sync_on_antigravity_switch,
  );

  const openModal = useCallback(() => setModalOpen(true), []);
  const closeModal = useCallback(() => setModalOpen(false), []);

  const handleToggle = async (next: boolean) => {
    if (busy) return;
    if (next) {
      if (!selectedServer) {
        setModalOpen(true);
        return;
      }
      setBusy(true);
      try {
        await upsertServer({
          id: selectedServer.id,
          name: selectedServer.name,
          host: selectedServer.host,
          port: selectedServer.port,
          username: selectedServer.username,
          codex_home: selectedServer.codex_home,
          antigravity_home: selectedServer.antigravity_home,
          auth: selectedServer.auth,
          sync_on_codex_switch: selectedServer.sync_on_codex_switch,
          sync_on_antigravity_switch: true,
        });
      } catch (error) {
        console.warn('[Antigravity SSH] enable sync failed:', error);
      } finally {
        setBusy(false);
      }
      return;
    }

    if (selectedServer) {
      setBusy(true);
      try {
        await upsertServer({
          id: selectedServer.id,
          name: selectedServer.name,
          host: selectedServer.host,
          port: selectedServer.port,
          username: selectedServer.username,
          codex_home: selectedServer.codex_home,
          antigravity_home: selectedServer.antigravity_home,
          auth: selectedServer.auth,
          sync_on_codex_switch: selectedServer.sync_on_codex_switch,
          sync_on_antigravity_switch: false,
        });
      } catch (error) {
        console.warn('[Antigravity SSH] disable sync failed:', error);
      } finally {
        setBusy(false);
      }
    }
  };

  const hintText = enabled && selectedServer
    ? t('antigravity.ssh.syncSwitchActive', '已启用 · {{name}}（{{user}}@{{host}}）', {
        name: selectedServer.name,
        user: selectedServer.username,
        host: selectedServer.host,
      })
    : t(
        'antigravity.ssh.syncSwitchDesc',
        '开启后配置 SSH 主机；Antigravity 切号时把当前账号同步到远端。',
      );

  if (variant === 'quick') {
    return (
      <>
        <div className="qs-row" style={{ marginTop: 8 }}>
          <div className="qs-row-label">
            <Server size={15} />
            <span>{t('antigravity.ssh.syncSwitch', '切号同步 SSH')}</span>
          </div>
          <div className="qs-row-control" style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
            <button
              type="button"
              className="btn btn-secondary icon-only"
              title={t('antigravity.ssh.configure', '配置')}
              aria-label={t('antigravity.ssh.configure', '配置')}
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
        <CodexSshSyncModal platform="antigravity" open={modalOpen} onClose={closeModal} />
      </>
    );
  }

  return (
    <>
      <div className="settings-row">
        <div className="row-label">
          <div className="row-title">{t('antigravity.ssh.syncSwitch', '切号同步 SSH')}</div>
          <div className="row-desc">{hintText}</div>
        </div>
        <div className="row-control" style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <button
            type="button"
            className="btn btn-secondary"
            onClick={openModal}
          >
            <Settings2 size={14} />
            <span>{t('antigravity.ssh.configure', '配置')}</span>
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
      <CodexSshSyncModal platform="antigravity" open={modalOpen} onClose={closeModal} />
    </>
  );
}
