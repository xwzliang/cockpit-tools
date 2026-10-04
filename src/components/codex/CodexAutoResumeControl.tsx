import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ClockAlert } from 'lucide-react';
import { CodexInterruptedTasksModal } from './CodexInterruptedTasksModal';

interface CodexAutoResumeControlProps {
  enabled: boolean;
  onEnabledChange: (enabled: boolean) => void;
  syncSsh: boolean;
  onSyncSshChange: (syncSsh: boolean) => void;
}

export function CodexAutoResumeControl({
  enabled,
  onEnabledChange,
  syncSsh,
  onSyncSshChange,
}: CodexAutoResumeControlProps) {
  const { t } = useTranslation();
  const [modalOpen, setModalOpen] = useState(false);

  return (
    <>
      <div className="settings-row">
        <div className="row-label">
          <div className="row-title">
            {t('codex.autoResume.enableTitle', '受限中断任务自动恢复')}
          </div>
          <div className="row-desc">
            {t(
              'codex.autoResume.enableDesc',
              '当 Codex 任务或 /goal 因达到 5 小时（或周度）配额限制中断时，后台在配额恢复后自动恢复执行。',
            )}
          </div>
        </div>
        <div className="row-control">
          <label className="switch">
            <input
              type="checkbox"
              checked={enabled}
              onChange={(e) => onEnabledChange(e.target.checked)}
            />
            <span className="slider"></span>
          </label>
        </div>
      </div>

      {enabled && (
        <div className="settings-row" style={{ animation: 'fadeUp 0.3s ease both' }}>
          <div className="row-label">
            <div className="row-title">
              {t('codex.autoResume.syncSshTitle', '包含远程 SSH 服务器')}
            </div>
            <div className="row-desc">
              {t(
                'codex.autoResume.syncSshDesc',
                '除本机外，同时监控与恢复已配置 SSH 服务器上因配额受限中断的 Codex 任务。',
              )}
            </div>
          </div>
          <div className="row-control">
            <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
              <label className="switch">
                <input
                  type="checkbox"
                  checked={syncSsh}
                  onChange={(e) => onSyncSshChange(e.target.checked)}
                />
                <span className="slider"></span>
              </label>

              <button
                type="button"
                className="btn btn-secondary"
                onClick={() => setModalOpen(true)}
                style={{ display: 'inline-flex', alignItems: 'center', gap: '6px' }}
              >
                <ClockAlert size={15} />
                <span>{t('codex.autoResume.viewTasksButton', '查看受限中断任务')}</span>
              </button>
            </div>
          </div>
        </div>
      )}

      <CodexInterruptedTasksModal
        open={modalOpen}
        onClose={() => setModalOpen(false)}
      />
    </>
  );
}
