import { test } from 'node:test';
import assert from 'node:assert/strict';
import { sourceMessage, freshnessThreshold } from '../src/source-state.ts';

test('unimplemented, unverified, error and unsupported have distinct user messages', () => {
  for (const chinese of [true, false]) {
    const messages = ['not_implemented', 'unverified', 'error', 'unsupported', 'warming_up', 'permission_required']
      .map(state => sourceMessage(state, chinese));
    assert.equal(new Set(messages).size, messages.length);
    assert.ok(messages.every(message => message.length > 0));
  }
});

test('freshness accommodates a 30 second background interval without premature staleness', () => {
  assert.equal(freshnessThreshold(1000), 15);
  assert.equal(freshnessThreshold(30000), 90);
  for (const value of [NaN, Infinity, -1, 0]) assert.equal(freshnessThreshold(value), 15);
});
