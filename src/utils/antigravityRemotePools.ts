import type { SshServer } from '../types/sshServer';

export const REMOTE_POOL_COLOR_COUNT = 4;

export interface AntigravityRemotePoolUsage {
  serverId: string;
  serverName: string;
  host: string;
  syncedAt: number;
  colorIndex: number;
}

export interface AntigravityRemotePoolInfo {
  serverId: string;
  serverName: string;
  host: string;
  colorIndex: number;
  excludedAccountIds: ReadonlySet<string>;
  activeAccountId: string | null;
}

/**
 * Returns metadata for every SSH server configured with an independent
 * Antigravity account pool, in deterministic palette order.
 */
export function buildAntigravityRemotePools(
  servers: readonly SshServer[],
): AntigravityRemotePoolInfo[] {
  const pools: AntigravityRemotePoolInfo[] = [];

  for (const server of servers) {
    if (server.antigravity_account_policy?.mode !== 'independent') {
      continue;
    }

    const colorIndex = pools.length % REMOTE_POOL_COLOR_COUNT;
    const lastSync = server.last_antigravity_sync;
    const activeAccountId =
      lastSync?.verified && lastSync.account_id?.trim()
        ? lastSync.account_id.trim()
        : null;

    pools.push({
      serverId: server.id,
      serverName: server.name.trim() || server.host,
      host: server.host,
      colorIndex,
      excludedAccountIds: new Set(server.antigravity_account_policy.excluded_account_ids ?? []),
      activeAccountId,
    });
  }

  return pools;
}

/**
 * Filters independent remote pools to those allowed to switch to the given account.
 */
export function getEligibleRemotePoolsForAccount(
  pools: readonly AntigravityRemotePoolInfo[],
  account: { id: string; disabled?: boolean; pending_oauth?: boolean },
): AntigravityRemotePoolInfo[] {
  if (account.disabled || account.pending_oauth) {
    return [];
  }
  return pools.filter((pool) => !pool.excludedAccountIds.has(account.id));
}

export function getRemotePoolSwitchKey(serverId: string, accountId: string): string {
  return `${serverId}::${accountId}`;
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
