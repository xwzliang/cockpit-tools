import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export interface PendingAutoRestore {
  provider: 'codex' | 'antigravity';
  task_id: string;
  title: string;
  deadline: number;
}

/** Deadlines belong to the backend: reopening the UI must not restart the grace period. */
export function usePendingAutoRestores(provider: PendingAutoRestore['provider']) {
  const [pending, setPending] = useState<PendingAutoRestore[]>([]);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));

  useEffect(() => {
    let disposed = false;
    let revision = 0;
    let request = 0;
    let unlisten: (() => void) | undefined;
    const refresh = async () => {
      const startedRevision = revision;
      const startedRequest = ++request;
      try {
        const snapshot = await invoke<PendingAutoRestore[]>('get_pending_auto_restores');
        // A newer event or request must never be overwritten by a stale snapshot.
        if (!disposed && revision === startedRevision && request === startedRequest) {
          setPending(snapshot);
        }
      } catch (error) {
        console.warn('Failed to read pending automatic restores:', error);
      }
    };
    void listen<PendingAutoRestore[]>('auto-restore://pending-changed', ({ payload }) => {
      if (!disposed) {
        revision += 1;
        setPending(payload);
      }
    }).then((stop) => {
      if (disposed) stop();
      else {
        unlisten = stop;
        void refresh();
      }
    }).catch((error) => {
      console.warn('Failed to listen for pending automatic restores:', error);
      if (!disposed) void refresh();
    });
    const poll = window.setInterval(() => void refresh(), 5000);
    const tick = window.setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000);
    window.addEventListener('focus', refresh);
    return () => {
      disposed = true;
      unlisten?.();
      window.clearInterval(poll);
      window.clearInterval(tick);
      window.removeEventListener('focus', refresh);
    };
  }, []);

  return pending.filter((entry) => entry.provider === provider).map((entry) => ({
    ...entry,
    secondsRemaining: Math.max(0, entry.deadline - now),
  }));
}
