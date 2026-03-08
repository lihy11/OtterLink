const test = require('node:test');
const assert = require('node:assert/strict');

const { FeishuClient } = require('../src/feishu/client');

test('fetches tenant token from live Feishu API', {
  skip: !(process.env.APP_ID && process.env.APP_SECRET),
}, async () => {
  const client = new FeishuClient({
    appId: process.env.APP_ID,
    appSecret: process.env.APP_SECRET,
  });
  const token = await client.getTenantAccessToken();
  assert.equal(typeof token, 'string');
  assert.ok(token.length > 20);
});
