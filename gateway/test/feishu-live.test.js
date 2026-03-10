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

test('diagnoses whether delayed CardKit streaming updates time out', {
  skip: !(
    process.env.APP_ID &&
    process.env.APP_SECRET &&
    process.env.FEISHU_TEST_OPEN_ID &&
    process.env.FEISHU_STREAM_TIMEOUT_WAIT_MS
  ),
  timeout: Number(process.env.FEISHU_STREAM_TIMEOUT_WAIT_MS || 0) + 30_000,
}, async (t) => {
  const waitMs = Number(process.env.FEISHU_STREAM_TIMEOUT_WAIT_MS);
  assert.ok(Number.isFinite(waitMs) && waitMs > 0, 'FEISHU_STREAM_TIMEOUT_WAIT_MS must be a positive number');

  const client = new FeishuClient({
    appId: process.env.APP_ID,
    appSecret: process.env.APP_SECRET,
  });

  const card = {
    schema: '2.0',
    config: {
      streaming_mode: true,
      summary: {
        content: 'Streaming timeout diagnostic',
      },
      streaming_config: {
        print_frequency_ms: { default: 80 },
        print_step: { default: 4 },
      },
    },
    body: {
      elements: [
        {
          tag: 'markdown',
          element_id: 'content',
          content: [
            '**Streaming Timeout Diagnostic**',
            '',
            `Created at: ${new Date().toISOString()}`,
            `Planned delay: ${waitMs} ms`,
            '',
            'Phase: initial send',
          ].join('\n'),
        },
      ],
    },
  };

  const sent = await client.sendCardKitToOpenId(process.env.FEISHU_TEST_OPEN_ID, card);
  assert.ok(sent.cardId, 'cardId should exist');
  console.log(JSON.stringify({
    phase: 'sent',
    cardId: sent.cardId,
    messageId: sent.messageId,
    waitMs,
  }));

  await t.test(`sleep ${waitMs}ms before update`, async () => {
    await new Promise((resolve) => setTimeout(resolve, waitMs));
  });

  let outcome = 'updated';
  let errorMessage = '';
  try {
    await client.updateCard(sent.cardId, {
      ...card,
      body: {
        elements: [
          {
            tag: 'markdown',
            element_id: 'content',
            content: [
              '**Streaming Timeout Diagnostic**',
              '',
              `Created at: ${new Date().toISOString()}`,
              `Delayed update after: ${waitMs} ms`,
              '',
              'Phase: delayed update',
            ].join('\n'),
          },
        ],
      },
    }, 3);
  } catch (error) {
    outcome = /card streaming timeout/i.test(String(error && error.message))
      ? 'streaming_timeout'
      : 'other_error';
    errorMessage = String(error && error.message);
  }

  console.log(JSON.stringify({
    phase: 'delayed_update',
    cardId: sent.cardId,
    messageId: sent.messageId,
    waitMs,
    outcome,
    errorMessage,
  }));

  assert.ok(
    ['updated', 'streaming_timeout', 'other_error'].includes(outcome),
    `unexpected outcome: ${outcome}`,
  );
});
