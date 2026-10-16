import assert from 'node:assert/strict';
import test from 'node:test';
import { quotaErrorRetryDue } from './quotaErrorRetry';

test('failed inactive accounts become eligible after one minute', () => {
  assert.equal(quotaErrorRetryDue(100, undefined, 159_999), false);
  assert.equal(quotaErrorRetryDue(100, undefined, 160_000), true);
});
test('successful reads and invalid timestamps are not retried', () => {
  assert.equal(quotaErrorRetryDue(undefined, 100, 900_000), false);
  assert.equal(quotaErrorRetryDue(NaN, undefined, 900_000), false);
});
test('recent attempts and newer failures prevent rapid repeated requests', () => {
  assert.equal(quotaErrorRetryDue(100, 150_000, 180_000), false);
  assert.equal(quotaErrorRetryDue(170, 150_000, 210_000), false);
  assert.equal(quotaErrorRetryDue(170, 150_000, 230_000), true);
});
