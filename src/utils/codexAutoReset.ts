import {
  CodexAccount,
  CodexResetCredit,
  isCodexAgentIdentityAccount,
  isCodexApiKeyAccount,
  isCodexPendingOAuthAccount,
} from '../types/codex';

const APP_PROFILE = (
  typeof import.meta !== 'undefined' && import.meta.env?.VITE_COCKPIT_TOOLS_PROFILE
    ? String(import.meta.env.VITE_COCKPIT_TOOLS_PROFILE)
    : ''
).trim();

const STORAGE_PROFILE_SUFFIX =
  APP_PROFILE && APP_PROFILE !== 'prod' ? `.${APP_PROFILE}` : '';

export const CODEX_AUTO_RESET_STORAGE_KEY = `agtools.codex.auto_reset_armed_accounts${STORAGE_PROFILE_SUFFIX}`;
export const CODEX_AUTO_RESET_EVENT_NAME = 'codex:auto-reset-armed-changed';

let inMemoryArmedSet: Set<string> | null = null;

function getLocalStorage(): Storage | null {
  if (typeof window !== 'undefined' && window.localStorage) {
    return window.localStorage;
  }
  if (typeof globalThis !== 'undefined' && (globalThis as unknown as { localStorage?: Storage }).localStorage) {
    return (globalThis as unknown as { localStorage?: Storage }).localStorage ?? null;
  }
  return null;
}

export function readArmedAccountIds(): Set<string> {
  const storage = getLocalStorage();
  if (!storage) {
    return inMemoryArmedSet ?? new Set();
  }

  try {
    const raw = storage.getItem(CODEX_AUTO_RESET_STORAGE_KEY);
    if (!raw) {
      return new Set();
    }
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed)) {
      return new Set(parsed.filter((item): item is string => typeof item === 'string' && item.length > 0));
    }
  } catch (e) {
    console.warn('[codexAutoReset] Failed to parse armed account ids from localStorage:', e);
  }
  return new Set();
}

function persistArmedAccountIds(armedSet: Set<string>): void {
  inMemoryArmedSet = new Set(armedSet);
  const storage = getLocalStorage();
  if (!storage) return;

  try {
    storage.setItem(CODEX_AUTO_RESET_STORAGE_KEY, JSON.stringify(Array.from(armedSet)));
  } catch (e) {
    console.warn('[codexAutoReset] Failed to persist armed account ids to localStorage:', e);
  }
}

function notifyArmedStateChanged(accountId: string, armed: boolean): void {
  if (typeof window !== 'undefined' && typeof window.dispatchEvent === 'function') {
    try {
      window.dispatchEvent(
        new CustomEvent(CODEX_AUTO_RESET_EVENT_NAME, {
          detail: { accountId, armed },
        }),
      );
    } catch {
      // Ignore if event dispatching is not supported
    }
  }
}

export function isCodexAutoResetArmed(accountId: string): boolean {
  if (!accountId) return false;
  const set = readArmedAccountIds();
  return set.has(accountId);
}

export function armCodexAutoReset(accountId: string): void {
  if (!accountId) return;
  const set = readArmedAccountIds();
  if (!set.has(accountId)) {
    set.add(accountId);
    persistArmedAccountIds(set);
    notifyArmedStateChanged(accountId, true);
  }
}

export function disarmCodexAutoReset(accountId: string): void {
  if (!accountId) return;
  const set = readArmedAccountIds();
  if (set.has(accountId)) {
    set.delete(accountId);
    persistArmedAccountIds(set);
    notifyArmedStateChanged(accountId, false);
  }
}

export function isAvailableResetCredit(credit: CodexResetCredit): boolean {
  const normalizedStatus = (credit.status || credit.raw_status || 'available')
    .trim()
    .toLowerCase();
  if (
    normalizedStatus === 'redeemed' ||
    normalizedStatus === 'used' ||
    normalizedStatus === 'consumed' ||
    normalizedStatus === 'expired'
  ) {
    return false;
  }
  return !(
    typeof credit.expires_at === 'number' &&
    Number.isFinite(credit.expires_at) &&
    credit.expires_at <= Math.floor(Date.now() / 1000)
  );
}

export function getCodexEarliestResetCreditExpiresAt(
  account: CodexAccount,
): number | null {
  const explicit = account.quota?.reset_credits_next_expires_at;
  if (typeof explicit === 'number' && Number.isFinite(explicit)) {
    return explicit;
  }

  const credits = Array.isArray(account.quota?.reset_credits)
    ? account.quota.reset_credits
    : [];

  const availableExpires = credits
    .filter(isAvailableResetCredit)
    .map((c) => c.expires_at)
    .filter(
      (v): v is number => typeof v === 'number' && Number.isFinite(v),
    )
    .sort((a, b) => a - b);

  return availableExpires[0] ?? null;
}

export function getCodexAvailableResetCreditsCount(
  account: CodexAccount,
): number {
  const direct = account.quota?.reset_credits_available;
  if (typeof direct === 'number' && Number.isFinite(direct)) {
    return direct;
  }
  const credits = Array.isArray(account.quota?.reset_credits)
    ? account.quota.reset_credits
    : [];
  return credits.filter(isAvailableResetCredit).length;
}

/**
 * Checks whether an account meets the trigger condition for auto-reset:
 * 1. Account is armed.
 * 2. Not an API key account or agent identity account.
 * 3. Has at least 1 available reset credit.
 * 4. Weekly window is present (or not explicitly marked false).
 * 5. Weekly quota percentage is finite and <= 1 (meaning <= 1% remaining).
 */
export function shouldTriggerCodexAutoReset(
  account: CodexAccount,
  armedSet?: Set<string>,
): boolean {
  if (!account?.id) return false;
  if (
    isCodexApiKeyAccount(account) ||
    isCodexAgentIdentityAccount(account) ||
    isCodexPendingOAuthAccount(account)
  ) {
    return false;
  }

  const armed = armedSet ? armedSet.has(account.id) : isCodexAutoResetArmed(account.id);
  if (!armed) {
    return false;
  }

  const availableCount = getCodexAvailableResetCreditsCount(account);
  if (availableCount <= 0) {
    return false;
  }

  if (account.quota?.weekly_window_present === false) {
    return false;
  }

  const weeklyPercentage = account.quota?.weekly_percentage;
  if (typeof weeklyPercentage !== 'number' || !Number.isFinite(weeklyPercentage)) {
    return false;
  }

  return weeklyPercentage <= 1;
}

export const CODEX_AUTO_RESET_EXECUTED_EVENT_NAME = 'codex:auto-reset-executed';

export function subscribeCodexAutoReset(listener: () => void): () => void {
  if (typeof window === 'undefined' || typeof window.addEventListener !== 'function') {
    return () => {};
  }
  const handler = () => listener();
  window.addEventListener(CODEX_AUTO_RESET_EVENT_NAME, handler);
  window.addEventListener('storage', handler);
  return () => {
    window.removeEventListener(CODEX_AUTO_RESET_EVENT_NAME, handler);
    window.removeEventListener('storage', handler);
  };
}

export function subscribeCodexAutoResetExecuted(
  listener: (detail: { accountId: string; accountName: string; success: boolean; error?: string }) => void,
): () => void {
  if (typeof window === 'undefined' || typeof window.addEventListener !== 'function') {
    return () => {};
  }
  const handler = (event: Event) => {
    const customEvent = event as CustomEvent<{
      accountId: string;
      accountName: string;
      success: boolean;
      error?: string;
    }>;
    if (customEvent.detail) {
      listener(customEvent.detail);
    }
  };
  window.addEventListener(CODEX_AUTO_RESET_EXECUTED_EVENT_NAME, handler);
  return () => {
    window.removeEventListener(CODEX_AUTO_RESET_EXECUTED_EVENT_NAME, handler);
  };
}

export interface CodexAutoResetExecutionOptions {
  consumeCredit?: (accountId: string) => Promise<void>;
  refreshQuota?: (accountId: string) => Promise<unknown>;
  onExecuted?: (account: CodexAccount) => void;
  onError?: (account: CodexAccount, error: unknown) => void;
}

const activeAutoResetInFlight = new Set<string>();

export async function executeCodexAutoResetIfEligible(
  accounts: CodexAccount[],
  options: CodexAutoResetExecutionOptions = {},
): Promise<string[]> {
  const armedSet = readArmedAccountIds();
  if (armedSet.size === 0) return [];

  const candidates = accounts.filter((acc) => shouldTriggerCodexAutoReset(acc, armedSet));
  const executedIds: string[] = [];

  for (const account of candidates) {
    if (activeAutoResetInFlight.has(account.id)) {
      continue;
    }
    activeAutoResetInFlight.add(account.id);
    // Crucial: immediately disarm before API call to ensure single-shot
    disarmCodexAutoReset(account.id);

    const accountName = account.email || account.id;

    try {
      if (options.consumeCredit) {
        await options.consumeCredit(account.id);
      } else {
        const { consumeCodexResetCredit } = await import('../services/codexService');
        await consumeCodexResetCredit(account.id);
      }

      executedIds.push(account.id);
      options.onExecuted?.(account);

      if (typeof window !== 'undefined' && typeof window.dispatchEvent === 'function') {
        window.dispatchEvent(
          new CustomEvent(CODEX_AUTO_RESET_EXECUTED_EVENT_NAME, {
            detail: {
              accountId: account.id,
              accountName,
              success: true,
            },
          }),
        );
      }

      try {
        if (options.refreshQuota) {
          await options.refreshQuota(account.id);
        } else {
          const { refreshCodexQuota } = await import('../services/codexService');
          await refreshCodexQuota(account.id);
        }
      } catch (refreshErr) {
        console.warn('[codexAutoReset] Quota refresh after auto-reset failed:', refreshErr);
      }
    } catch (error) {
      console.error('[codexAutoReset] Failed to consume auto-reset credit for', account.id, error);
      const errorMessage = String(error).replace(/^Error:\s*/, '');
      options.onError?.(account, error);

      if (typeof window !== 'undefined' && typeof window.dispatchEvent === 'function') {
        window.dispatchEvent(
          new CustomEvent(CODEX_AUTO_RESET_EXECUTED_EVENT_NAME, {
            detail: {
              accountId: account.id,
              accountName,
              success: false,
              error: errorMessage,
            },
          }),
        );
      }
    } finally {
      activeAutoResetInFlight.delete(account.id);
    }
  }

  return executedIds;
}
