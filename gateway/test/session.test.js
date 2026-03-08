const test = require('node:test');
const assert = require('node:assert/strict');

const { buildSessionRoute, normalizeFeishuEvent } = require('../src/feishu/session');

test('normalizeFeishuEvent extracts text and sender', () => {
  const event = normalizeFeishuEvent({
    sender: { sender_id: { open_id: 'ou_1' } },
    message: {
      message_id: 'om_1',
      chat_id: 'oc_1',
      chat_type: 'p2p',
      content: JSON.stringify({ text: 'hello' }),
    },
  });

  assert.equal(event.openId, 'ou_1');
  assert.equal(event.text, 'hello');
});

test('buildSessionRoute prefers thread scope over chat scope', () => {
  assert.deepEqual(
    buildSessionRoute({ openId: 'ou_1', chatId: 'oc_1', chatType: 'group', threadId: 'th_1' }),
    {
      sessionKey: 'feishu:thread:oc_1:th_1',
      parentSessionKey: 'feishu:chat:oc_1',
    },
  );
});
