import { useEffect, useMemo } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useSshServerStore } from '../stores/useSshServerStore';
import type { SshAntigravitySyncResult } from '../types/sshServer';
import {
  buildAntigravityRemotePoolUsageMap,
  type AntigravityRemotePoolUsage,
} from '../utils/antigravityRemotePools';

export function useAntigravityRemotePools(): Record<string, AntigravityRemotePoolUsage[]> {
  const servers = useSshServerStore((state) => state.servers);
  const fetchServers = useSshServerStore((state) => state.fetchServers);

  useEffect(() => {
    void fetchServers();

    let cancelled = false;
    let unlistenFn: (() => void) | null = null;

    listen<SshAntigravitySyncResult>('antigravity:ssh-sync-result', (event) => {
      const store = useSshServerStore.getState();
      const hasServer = store.servers.some((server) => server.id === event.payload.server_id);
      if (hasServer) {
        store.applyAntigravitySyncResult(event.payload);
      } else {
        void store.fetchServers();
      }
    })
      .then((unlisten) => {
        if (cancelled) {
          unlisten();
        } else {
          unlistenFn = unlisten;
        }
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      if (unlistenFn) {
        unlistenFn();
      }
    };
  }, [fetchServers]);

  return useMemo(() => buildAntigravityRemotePoolUsageMap(servers), [servers]);
}
