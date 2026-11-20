import assert from 'node:assert/strict';
import test from 'node:test';
import type { SshServer } from '../types/sshServer';
import {
  buildAntigravityRemotePools,
  buildAntigravityRemotePoolUsageMap,
  getEligibleRemotePoolsForAccount,
  getRemotePoolHighlightClasses,
  getRemotePoolSwitchKey,
  REMOTE_POOL_COLOR_COUNT,
} from './antigravityRemotePools';

function makeServer(overrides: Partial<SshServer> = {}): SshServer {
  return {
    id: 'srv-1',
    name: 'macbookair',
    host: 'macbookair.local',
    port: 22,
    username: 'user',
    codex_home: '~/.codex',
    antigravity_home: '~/.gemini',
    auth: { kind: 'agent' },
    sync_on_codex_switch: false,
    sync_on_antigravity_switch: true,
    antigravity_account_policy: {
      mode: 'independent',
      excluded_account_ids: [],
      quota_threshold: 20,
    },
    created_at: 1,
    updated_at: 1,
    last_sync: null,
    last_antigravity_sync: {
      account_id: 'acc-remote',
      account_email: 'remote@example.com',
      bundle_hash: 'hash',
      synced_at: 100,
      verified: true,
      error: null,
    },
    ...overrides,
  };
}

test('maps verified independent-pool SSH servers to their active account', () => {
  const map = buildAntigravityRemotePoolUsageMap([
    makeServer(),
  ]);

  assert.deepEqual(map, {
    'acc-remote': [
      {
        serverId: 'srv-1',
        serverName: 'macbookair',
        host: 'macbookair.local',
        syncedAt: 100,
        colorIndex: 0,
      },
    ],
  });
  assert.equal(
    getRemotePoolHighlightClasses(map['acc-remote']),
    'remote-pool-active remote-pool-color-0',
  );
  assert.equal(getRemotePoolHighlightClasses(undefined), '');
  assert.equal(getRemotePoolHighlightClasses([]), '');
});

test('ignores follow_local, legacy, unverified, and empty sync entries', () => {
  const map = buildAntigravityRemotePoolUsageMap([
    makeServer({
      id: 'follow',
      name: 'follower',
      antigravity_account_policy: {
        mode: 'follow_local',
        excluded_account_ids: [],
        quota_threshold: 20,
      },
    }),
    makeServer({
      id: 'legacy',
      name: 'legacy',
      antigravity_account_policy: undefined,
    }),
    makeServer({
      id: 'unverified',
      name: 'unverified',
      last_antigravity_sync: {
        account_id: 'acc-1',
        account_email: 'a@example.com',
        bundle_hash: 'hash',
        synced_at: 100,
        verified: false,
        error: 'ssh failed',
      },
    }),
    makeServer({
      id: 'empty-id',
      name: 'empty-id',
      last_antigravity_sync: {
        account_id: '   ',
        account_email: 'a@example.com',
        bundle_hash: 'hash',
        synced_at: 100,
        verified: true,
        error: null,
      },
    }),
  ]);

  assert.deepEqual(map, {});
});

test('assigns distinct colors to multiple independent pools and falls back to host when name is blank', () => {
  const servers = Array.from({ length: REMOTE_POOL_COLOR_COUNT + 1 }, (_, index) =>
    makeServer({
      id: `srv-${index}`,
      name: index === 1 ? '   ' : `remote-${index}`,
      host: `host-${index}.local`,
      last_antigravity_sync: {
        account_id: index === 2 ? 'acc-0' : `acc-${index}`,
        account_email: `u${index}@example.com`,
        bundle_hash: 'hash',
        synced_at: 100 + index,
        verified: true,
        error: null,
      },
    }),
  );

  const map = buildAntigravityRemotePoolUsageMap(servers);

  assert.equal(map['acc-0'].length, 2);
  assert.equal(map['acc-0'][0].colorIndex, 0);
  assert.equal(map['acc-0'][1].colorIndex, 2);
  assert.equal(map['acc-1'][0].serverName, 'host-1.local');
  assert.equal(map['acc-1'][0].colorIndex, 1);
  assert.equal(map['acc-3'][0].colorIndex, 3);
  assert.equal(map[`acc-${REMOTE_POOL_COLOR_COUNT}`][0].colorIndex, 0);
});

test('filters eligible independent pools per account based on exclusions and account state', () => {
  const pools = buildAntigravityRemotePools([
    makeServer({
      id: 'srv-mba',
      name: 'macbookair',
      antigravity_account_policy: {
        mode: 'independent',
        excluded_account_ids: ['acc-local-only'],
        quota_threshold: 20,
      },
    }),
    makeServer({
      id: 'srv-follow',
      name: 'follower',
      antigravity_account_policy: {
        mode: 'follow_local',
        excluded_account_ids: [],
        quota_threshold: 20,
      },
    }),
    makeServer({
      id: 'srv-linux',
      name: 'linux-dev',
      antigravity_account_policy: {
        mode: 'independent',
        excluded_account_ids: [],
        quota_threshold: 20,
      },
    }),
  ]);

  assert.equal(pools.length, 2);
  assert.equal(pools[0].colorIndex, 0);
  assert.equal(pools[1].colorIndex, 1);
  assert.equal(getRemotePoolSwitchKey('srv-mba', 'acc-1'), 'srv-mba::acc-1');

  assert.deepEqual(
    getEligibleRemotePoolsForAccount(pools, { id: 'acc-local-only' }).map((p) => p.serverId),
    ['srv-linux'],
  );
  assert.deepEqual(
    getEligibleRemotePoolsForAccount(pools, { id: 'acc-shared' }).map((p) => p.serverId),
    ['srv-mba', 'srv-linux'],
  );
  assert.deepEqual(
    getEligibleRemotePoolsForAccount(pools, { id: 'acc-shared', disabled: true }),
    [],
  );
  assert.deepEqual(
    getEligibleRemotePoolsForAccount(pools, { id: 'acc-shared', pending_oauth: true }),
    [],
  );
});
