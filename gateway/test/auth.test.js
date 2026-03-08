const test = require('node:test');
const assert = require('node:assert/strict');

const { isAuthorized, parsePairCommand, validatePairRequest } = require('../src/feishu/auth');

function config(overrides = {}) {
  return {
    feishuAuthMode: 'off',
    pairAuthToken: '',
    allowFromOpenIds: new Set(),
    ...overrides,
  };
}

const pairings = {
  isPaired(openId) {
    return openId === 'ou_paired';
  },
};

test('parsePairCommand supports chinese and slash forms', () => {
  assert.equal(parsePairCommand('配对 abc'), 'abc');
  assert.equal(parsePairCommand('/pair abc'), 'abc');
  assert.equal(parsePairCommand('hello'), null);
});

test('isAuthorized supports pair_or_allow_from', () => {
  const cfg = config({ feishuAuthMode: 'pair_or_allow_from', allowFromOpenIds: new Set(['ou_allow']) });
  assert.equal(isAuthorized(cfg, pairings, 'ou_allow'), true);
  assert.equal(isAuthorized(cfg, pairings, 'ou_paired'), true);
  assert.equal(isAuthorized(cfg, pairings, 'ou_other'), false);
});

test('validatePairRequest enforces pair token', () => {
  const cfg = config({ feishuAuthMode: 'pair', pairAuthToken: 'secret' });
  assert.deepEqual(validatePairRequest(cfg, 'ou_user', 'secret'), { ok: true });
  assert.equal(validatePairRequest(cfg, 'ou_user', 'bad').ok, false);
});
