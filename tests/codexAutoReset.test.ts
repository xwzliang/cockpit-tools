import assert from 'node:assert/strict';
import { beforeEach, describe, it } from 'node:test';
import {
  armCodexAutoReset,
  CODEX_AUTO_RESET_STORAGE_KEY,
  disarmCodexAutoReset,
  getCodexAvailableResetCreditsCount,
  getCodexEarliestResetCreditExpiresAt,
  getCodexWeeklyQuotaPercentage,
  isCodexAutoResetArmed,
  readArmedAccountIds,
  shouldTriggerCodexAutoReset,
  executeCodexAutoResetIfEligible,
} from '../src/utils/codexAutoReset.ts';
import { CodexAccount } from '../src/types/codex.ts';

// Mock localStorage for test environment
class MockLocalStorage {
  private store = new Map<string, string>();
  getItem(key: string): string | null {
    return this.store.get(key) ?? null;
  }
  setItem(key: string, value: string): void {
    this.store.set(key, value);
  }
  removeItem(key: string): void {
    this.store.delete(key);
  }
  clear(): void {
    this.store.clear();
  }
}

describe('codexAutoReset helpers', () => {
  beforeEach(() => {
    const mockStorage = new MockLocalStorage();
    (globalThis as unknown as { localStorage: MockLocalStorage }).localStorage = mockStorage;
    mockStorage.clear();
  });

  it('arms and disarms account IDs in localStorage', () => {
    assert.equal(isCodexAutoResetArmed('acc-1'), false);
    armCodexAutoReset('acc-1');
    assert.equal(isCodexAutoResetArmed('acc-1'), true);

    const armed = readArmedAccountIds();
    assert.equal(armed.has('acc-1'), true);
    assert.equal(armed.size, 1);

    armCodexAutoReset('acc-2');
    assert.equal(isCodexAutoResetArmed('acc-2'), true);
    assert.equal(readArmedAccountIds().size, 2);

    disarmCodexAutoReset('acc-1');
    assert.equal(isCodexAutoResetArmed('acc-1'), false);
    assert.equal(isCodexAutoResetArmed('acc-2'), true);
    assert.equal(readArmedAccountIds().size, 1);

    disarmCodexAutoReset('acc-2');
    assert.equal(isCodexAutoResetArmed('acc-2'), false);
    assert.equal(readArmedAccountIds().size, 0);
  });

  it('calculates available reset credits count', () => {
    const nowSec = Math.floor(Date.now() / 1000);
    const accountWithDirect = {
      id: 'acc-1',
      quota: {
        reset_credits_available: 3,
        reset_credits: [],
      },
    } as unknown as CodexAccount;
    assert.equal(getCodexAvailableResetCreditsCount(accountWithDirect), 3);

    const accountWithCredits = {
      id: 'acc-2',
      quota: {
        reset_credits: [
          { id: '1', status: 'available', expires_at: nowSec + 3600 },
          { id: '2', status: 'redeemed', expires_at: nowSec + 3600 },
          { id: '3', status: 'available', expires_at: nowSec - 100 }, // expired
          { id: '4', status: 'available', expires_at: nowSec + 7200 },
        ],
      },
    } as unknown as CodexAccount;
    assert.equal(getCodexAvailableResetCreditsCount(accountWithCredits), 2);
  });

  it('finds earliest reset credit expiration time', () => {
    const nowSec = Math.floor(Date.now() / 1000);
    const accountExplicit = {
      id: 'acc-1',
      quota: {
        reset_credits_next_expires_at: nowSec + 500,
        reset_credits: [],
      },
    } as unknown as CodexAccount;
    assert.equal(getCodexEarliestResetCreditExpiresAt(accountExplicit), nowSec + 500);

    const accountFromList = {
      id: 'acc-2',
      quota: {
        reset_credits: [
          { id: '1', status: 'available', expires_at: nowSec + 9000 },
          { id: '2', status: 'available', expires_at: nowSec + 1000 },
          { id: '3', status: 'redeemed', expires_at: nowSec + 200 }, // redeemed, ignored
          { id: '4', status: 'available', expires_at: nowSec + 5000 },
        ],
      },
    } as unknown as CodexAccount;
    assert.equal(getCodexEarliestResetCreditExpiresAt(accountFromList), nowSec + 1000);
  });

  it('triggers auto-reset when condition (weekly_percentage <= 1) is met and account is armed', () => {
    armCodexAutoReset('acc-target');

    const baseAccount: CodexAccount = {
      id: 'acc-target',
      email: 'test@example.com',
      tokens: { access_token: 'tok' },
      created_at: Date.now(),
      last_used: Date.now(),
      quota: {
        hourly_percentage: 50,
        weekly_percentage: 1, // 1% remaining -> should trigger
        reset_credits_available: 2,
      },
    };

    assert.equal(shouldTriggerCodexAutoReset(baseAccount), true);

    // 0% remaining -> should trigger
    const zeroRemaining = {
      ...baseAccount,
      quota: { ...baseAccount.quota!, weekly_percentage: 0 },
    };
    assert.equal(shouldTriggerCodexAutoReset(zeroRemaining), true);

    // 0.5% remaining -> should trigger
    const pointFiveRemaining = {
      ...baseAccount,
      quota: { ...baseAccount.quota!, weekly_percentage: 0.5 },
    };
    assert.equal(shouldTriggerCodexAutoReset(pointFiveRemaining), true);

    // 1.01% or 2% remaining -> should NOT trigger
    const twoRemaining = {
      ...baseAccount,
      quota: { ...baseAccount.quota!, weekly_percentage: 2 },
    };
    assert.equal(shouldTriggerCodexAutoReset(twoRemaining), false);

    // 0 reset credits -> should NOT trigger
    const noCredits = {
      ...baseAccount,
      quota: { ...baseAccount.quota!, reset_credits_available: 0 },
    };
    assert.equal(shouldTriggerCodexAutoReset(noCredits), false);

    // Unarmed account -> should NOT trigger
    disarmCodexAutoReset('acc-target');
    assert.equal(shouldTriggerCodexAutoReset(baseAccount), false);

    // API Key account -> should NOT trigger
    armCodexAutoReset('acc-apikey');
    const apiKeyAccount = {
      ...baseAccount,
      id: 'acc-apikey',
      auth_mode: 'apikey',
      openai_api_key: 'sk-1234',
    };
    assert.equal(shouldTriggerCodexAutoReset(apiKeyAccount), false);

    // Pro account with single weekly window (e.g. Pro 5X in user scenario)
    armCodexAutoReset('acc-pro-single-window');
    const proSingleWindowAccount: CodexAccount = {
      id: 'acc-pro-single-window',
      email: 'pro@example.com',
      tokens: { access_token: 'tok' },
      plan_type: 'pro',
      created_at: Date.now(),
      last_used: Date.now(),
      quota: {
        hourly_percentage: 1, // 1% weekly in primary window
        hourly_window_minutes: 10080, // 7 days = 10080 mins
        hourly_window_present: true,
        weekly_percentage: 0,
        weekly_window_present: false, // marked false because backend placed weekly in primary
        reset_credits_available: 2,
      },
    };
    assert.equal(getCodexWeeklyQuotaPercentage(proSingleWindowAccount), 1);
    assert.equal(shouldTriggerCodexAutoReset(proSingleWindowAccount), true);

    // Floating percentage that renders as 1% on UI (e.g. 1.2%)
    armCodexAutoReset('acc-floating');
    const floatingOnePointTwo = {
      ...baseAccount,
      id: 'acc-floating',
      quota: { ...baseAccount.quota!, weekly_percentage: 1.2 },
    };
    assert.equal(getCodexWeeklyQuotaPercentage(floatingOnePointTwo), 1);
    assert.equal(shouldTriggerCodexAutoReset(floatingOnePointTwo), true);

    // 5h-only account with 1% remaining -> should NOT trigger because 5h is NOT weekly
    armCodexAutoReset('acc-five-hour-only');
    const fiveHourOnlyAccount: CodexAccount = {
      id: 'acc-five-hour-only',
      email: 'fivehour@example.com',
      tokens: { access_token: 'tok' },
      created_at: Date.now(),
      last_used: Date.now(),
      quota: {
        hourly_percentage: 1,
        hourly_window_minutes: 300, // 5 hours
        hourly_window_present: true,
        weekly_percentage: 0,
        weekly_window_present: false,
        reset_credits_available: 2,
      },
    };
    assert.equal(getCodexWeeklyQuotaPercentage(fiveHourOnlyAccount), null);
    assert.equal(shouldTriggerCodexAutoReset(fiveHourOnlyAccount), false);

    // Pending OAuth account -> should NOT trigger
    armCodexAutoReset('acc-pending');
    const pendingAccount = {
      ...baseAccount,
      id: 'acc-pending',
      authorization_status: 'pending',
    };
    assert.equal(shouldTriggerCodexAutoReset(pendingAccount), false);
  });

  it('executes auto-reset in a single-shot manner and immediately unchecks/disarms', async () => {
    armCodexAutoReset('acc-exec');
    assert.equal(isCodexAutoResetArmed('acc-exec'), true);

    const eligibleAccount: CodexAccount = {
      id: 'acc-exec',
      email: 'user@example.com',
      tokens: { access_token: 'tok' },
      created_at: Date.now(),
      last_used: Date.now(),
      quota: {
        hourly_percentage: 10,
        weekly_percentage: 1, // <= 1%
        reset_credits_available: 2,
      },
    };

    let consumeCalls = 0;
    let refreshCalls = 0;
    const executed = await executeCodexAutoResetIfEligible([eligibleAccount], {
      consumeCredit: async (id) => {
        consumeCalls += 1;
        assert.equal(id, 'acc-exec');
      },
      refreshQuota: async (id) => {
        refreshCalls += 1;
        assert.equal(id, 'acc-exec');
      },
    });

    assert.deepEqual(executed, ['acc-exec']);
    assert.equal(consumeCalls, 1);
    assert.equal(refreshCalls, 1);

    // CRITICAL: Account must now be disarmed / unchecked!
    assert.equal(isCodexAutoResetArmed('acc-exec'), false);

    // Calling it again must NOT execute (single-shot guarantee)
    const secondPass = await executeCodexAutoResetIfEligible([eligibleAccount], {
      consumeCredit: async () => {
        consumeCalls += 1;
      },
    });
    assert.deepEqual(secondPass, []);
    assert.equal(consumeCalls, 1);
  });
});
