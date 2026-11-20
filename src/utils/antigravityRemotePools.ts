import type { SshServer } from '../types/sshServer';

export const REMOTE_POOL_COLOR_COUNT = 4;

export interface AntigravityRemotePoolUsage {
  serverId: string;
  serverName: string;
  host: string;
  syncedAt: number;
  colorIndex: number;
}

/**
 * Builds a lookup from Antigravity account ID to the independent-pool remote
 * SSH machines currently verified to be using that account.
 */
export function buildAntigravityRemotePoolUsageMap(
  servers: readonly SshServer[],
): Record<string, AntigravityRemotePoolUsage[]> {
  const usageMap: Record<string, AntigravityRemotePoolUsage[]> = {};
  let independentPoolIndex = 0;

  for (const server of servers) {
    if (server.antigravity_account_policy?.mode !== 'independent') {
      continue;
    }

    const colorIndex = independentPoolIndex % REMOTE_POOL_COLOR_COUNT;
    independentPoolIndex += 1;

    const lastSync = server.last_antigravity_sync;
    const accountId = lastSync?.account_id?.trim();
    if (!lastSync?.verified || !accountId) {
      continue;
    }

    const entry: AntigravityRemotePoolUsage = {
      serverId: server.id,
      serverName: server.name.trim() || server.host,
      host: server.host,
      syncedAt: lastSync.synced_at,
      colorIndex,
    };

    if (!usageMap[accountId]) {
      usageMap[accountId] = [entry];
    } else {
      usageMap[accountId].push(entry);
    }
  }

  return usageMap;
}

export function getRemotePoolHighlightClasses(
  usages?: readonly AntigravityRemotePoolUsage[],
): string {
  if (!usages || usages.length === 0) {
    return '';
  }
  return `remote-pool-active remote-pool-color-${usages[0].colorIndex}`;
}
