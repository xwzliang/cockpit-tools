/** Retry failed quota reads independently of the normal refresh interval. */
export function quotaErrorRetryDue(
  errorTimestamp: number | undefined,
  lastAttemptMs: number | undefined,
  nowMs: number,
): boolean {
  if (errorTimestamp === undefined || !Number.isFinite(errorTimestamp)) return false;
  return nowMs - Math.max(errorTimestamp * 1000, lastAttemptMs ?? 0) >= 60_000;
}
