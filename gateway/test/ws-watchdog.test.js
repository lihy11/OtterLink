const test = require('node:test');
const assert = require('node:assert/strict');

const { shouldRestartWsClient, shouldRestartIdleWsClient } = require('../src/feishu/ws-watchdog');

test('shouldRestartWsClient returns false without reconnect schedule', () => {
  assert.equal(shouldRestartWsClient({ lastConnectTime: 0, nextConnectTime: 0 }, 1000, 300), false);
});

test('shouldRestartWsClient returns false while reconnect is still within grace window', () => {
  assert.equal(shouldRestartWsClient({ lastConnectTime: 0, nextConnectTime: 900 }, 1000, 300), false);
});

test('shouldRestartWsClient returns true when reconnect schedule is overdue and no later success happened', () => {
  assert.equal(shouldRestartWsClient({ lastConnectTime: 0, nextConnectTime: 500 }, 1000, 300), true);
});

test('shouldRestartWsClient returns false after a later successful connection', () => {
  assert.equal(shouldRestartWsClient({ lastConnectTime: 1200, nextConnectTime: 500 }, 1500, 300), false);
});

test('shouldRestartIdleWsClient returns false for missing timestamp', () => {
  assert.equal(shouldRestartIdleWsClient(0, 1000, 300), false);
});

test('shouldRestartIdleWsClient returns false before idle timeout', () => {
  assert.equal(shouldRestartIdleWsClient(800, 1000, 300), false);
});

test('shouldRestartIdleWsClient returns true after idle timeout', () => {
  assert.equal(shouldRestartIdleWsClient(600, 1000, 300), true);
});
