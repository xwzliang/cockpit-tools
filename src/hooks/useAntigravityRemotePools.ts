import { useCallback, useEffect, useMemo, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useSshServerStore } from '../stores/useSshServerStore';
import type { SshAntigravitySyncResult } from '../types/sshServer';
import {
  buildAntigravityRemotePools,
  buildAntigravityRemotePoolUsageMap,
  getRemotePoolSwitchKey,
  type AntigravityRemotePoolInfo,
  type AntigravityRemotePoolUsage,
} from '../utils/antigravityRemotePools';

export interface UseAntigravityRemotePoolsResult {
  usageMap: Record<string, AntigravityRemotePoolUsage[]>;
  pools: AntigravityRemotePoolInfo[];
  switchingKeys: ReadonlySet<string>;
  switchRemotePoolAccount: (serverId: string, accountId: string) => Promise<SshAntigravitySyncResult>;
}

export function useAntigravityRemotePools(): UseAntigravityRemotePoolsResult {
  const servers = useSshServerStore((state) => state.servers);
  const fetchServers = useSshServerStore((state) => state.fetchServers);
  const syncAntigravityAccount = useSshServerStore((state) => state.syncAntigravityAccount);
  const [switchingKeys, setSwitchingKeys] = useState<Set<string>>(() => new Set());

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

  const usageMap = useMemo(() => buildAntigravityRemotePoolUsageMap(servers), [servers]);
  const pools = useMemo(() => buildAntigravityRemotePools(servers), [servers]);

  const switchRemotePoolAccount = useCallback(
    async (serverId: string, accountId: string): Promise<SshAntigravitySyncResult> => {
      const key = getRemotePoolSwitchKey(serverId, accountId);
      setSwitchingKeys((prev) => {
        const next = new Set(prev);
        next.add(key);
        return next;
      });
      try {
        return await syncAntigravityAccount(serverId, accountId);
      } finally {
        setSwitchingKeys((prev) => {
          const next = new Set(prev);
          next.delete(key);
          return next;
        });
      }
    },
    [syncAntigravityAccount],
  );

  return { usageMap, pools, switchingKeys, switchRemotePoolAccount };
}
