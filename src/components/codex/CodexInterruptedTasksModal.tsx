import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { Clock, X } from 'lucide-react';
import { useEscClose } from '../../hooks/useEscClose';
import { CodexAutoResumeSection } from './CodexAutoResumeSection';
import './CodexInterruptedTasksModal.css';

interface CodexInterruptedTasksModalProps {
  open: boolean;
  onClose: () => void;
}

export function CodexInterruptedTasksModal({
  open,
  onClose,
}: CodexInterruptedTasksModalProps) {
  const { t } = useTranslation();
  useEscClose(open, onClose);

  if (!open || typeof document === 'undefined') return null;

  return createPortal(
    <div className="codex-interrupted-modal-overlay">
      <div
        className="codex-interrupted-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="codex-interrupted-modal-title"
      >
        <div className="codex-interrupted-modal__header">
          <div className="codex-interrupted-modal__header-main">
            <div className="codex-interrupted-modal__icon">
              <Clock size={20} />
            </div>
            <div className="codex-interrupted-modal__titles">
              <h2 id="codex-interrupted-modal-title">
                {t('codex.autoResume.modalTitle', '受限中断任务列表')}
              </h2>
              <p className="codex-interrupted-modal__subtitle">
                {t(
                  'codex.autoResume.modalSubtitle',
                  '监控因 5 小时（或周度）配额受限中断的 Codex 任务与 /goal。配额重置后系统将自动恢复，您也可以随时手动恢复。',
                )}
              </p>
            </div>
          </div>
          <div className="codex-interrupted-modal__actions">
            <button
              type="button"
              className="codex-interrupted-modal__close"
              onClick={onClose}
              title={t('common.close', '关闭')}
              aria-label={t('common.close', '关闭')}
            >
              <X size={16} />
            </button>
          </div>
        </div>

        <div className="codex-interrupted-modal__body">
          <CodexAutoResumeSection variant="settings" />
        </div>
      </div>
    </div>,
    document.body,
  );
}
