const test = require('node:test');
const assert = require('node:assert/strict');

const { GatewayService } = require('../src/service');
const { renderControlResponse } = require('../src/commands');

function makeService(overrides = {}) {
  const calls = {
    replies: [],
    updates: [],
    submits: [],
    paired: [],
  };
  const service = new GatewayService({
    config: {
      feishuAuthMode: 'pair_or_allow_from',
      pairAuthToken: 'secret',
      allowFromOpenIds: new Set(['ou_allow']),
    },
    feishuClient: {
      async replyToMessage(messageId, rendered) {
        calls.replies.push({ messageId, rendered });
        return `reply_${calls.replies.length}`;
      },
      async replyCardKitToMessage(messageId, card) {
        calls.cardReplies = calls.cardReplies || [];
        calls.cardReplies.push({ messageId, card });
        return { messageId: `reply_${(calls.cardReplies || []).length}`, cardId: `card_${(calls.cardReplies || []).length}` };
      },
      async updateMessage(messageId, rendered) {
        calls.updates.push({ messageId, rendered });
        return messageId;
      },
      async updateCard(cardId, card, sequence) {
        calls.cardUpdates = calls.cardUpdates || [];
        calls.cardUpdates.push({ cardId, card, sequence });
        return cardId;
      },
      async sendToOpenId() {
        return 'notify_1';
      },
      async sendCardKitToOpenId() {
        return { messageId: 'notify_1', cardId: 'card_notify_1' };
      },
    },
    coreClient: {
      async submitTurn(turnRequest) {
        calls.submits.push(turnRequest);
        return { ok: true, turn_id: turnRequest.turn_id };
      },
      async controlSession(controlRequest) {
        calls.controls = calls.controls || [];
        calls.controls.push(controlRequest);
        return {
          ok: true,
          message: 'runtime ok',
          active_runtime: {
            runtime_id: 'rt_1',
            label: 'claude-main',
            agent_kind: 'claude_code',
            workspace_path: '/tmp/demo',
            tag: 'master',
            prompt_preview: 'inspect repository status',
            has_runtime_session_ref: false,
            is_active: true,
          },
          runtimes: [],
        };
      },
    },
    pairings: {
      isPaired(openId) {
        return openId === 'ou_paired';
      },
      async pair(openId) {
        calls.paired.push(openId);
      },
      firstPaired() {
        return 'ou_paired';
      },
    },
    ...overrides,
  });
  return { service, calls };
}

test('authorized inbound message is forwarded to core with computed session keys', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_1',
      chat_id: 'oc_1',
      chat_type: 'group',
      thread_id: 'th_1',
      content: JSON.stringify({ text: 'hello' }),
    },
  });

  assert.equal(result.accepted, true);
  assert.equal(calls.submits.length, 1);
  assert.equal(calls.submits[0].session_key, 'feishu:thread:oc_1:th_1');
  assert.equal(calls.submits[0].parent_session_key, 'feishu:chat:oc_1');
});

test('unauthorized p2p sender gets hint and is not forwarded', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_other' } },
    message: {
      message_id: 'om_2',
      chat_id: 'oc_2',
      chat_type: 'p2p',
      content: JSON.stringify({ text: 'hello' }),
    },
  });

  assert.equal(result.reason, 'unauthorized');
  assert.equal(calls.submits.length, 0);
  assert.equal(calls.replies.length, 1);
});

test('pair command updates pair store', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_new' } },
    message: {
      message_id: 'om_3',
      chat_id: 'oc_3',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '配对 secret' }),
    },
  });

  assert.equal(result.paired, true);
  assert.deepEqual(calls.paired, ['ou_new']);
  assert.equal(calls.replies.length, 1);
});

test('core event updates existing cardkit card for the same slot', async () => {
  const { service, calls } = makeService();
  await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_4',
      chat_id: 'oc_4',
      chat_type: 'group',
      content: JSON.stringify({ text: 'hello' }),
    },
  });

  const turnId = calls.submits[0].turn_id;
  await service.handleCoreEvent({
    turn_id: turnId,
    slot: 'progress',
    message: {
      kind: 'card',
      card: {
        title: 'Progress',
        theme: 'blue',
        wide_screen_mode: true,
        update_multi: true,
        blocks: [{ kind: 'markdown', text: 'first' }],
      },
    },
  });
  await service.handleCoreEvent({
    turn_id: turnId,
    slot: 'progress',
    message: {
      kind: 'card',
      card: {
        title: 'Progress',
        theme: 'blue',
        wide_screen_mode: true,
        update_multi: true,
        blocks: [{ kind: 'markdown', text: 'second' }],
      },
    },
  });

  assert.equal(calls.cardReplies.length, 1);
  assert.equal(calls.cardUpdates.length, 1);
});

test('runtime control command is routed to core control api instead of normal turn api', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_5',
      chat_id: 'oc_5',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/runtime list' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.submits.length, 0);
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.cardReplies.length, 1);
});

test('runtime load command is routed to core control api with workspace path', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_6',
      chat_id: 'oc_6',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/runtime load /tmp/demo-workspace' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.submits.length, 0);
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.controls[0].action, 'load_runtimes');
  assert.equal(calls.controls[0].workspace_path, '/tmp/demo-workspace');
});

test('renderControlResponse builds a card with runtime rows', async () => {
  const message = renderControlResponse({
    ok: true,
    message: 'loaded',
    active_runtime: {
      runtime_id: 'rt_1234567890',
      label: 'claude-main',
      agent_kind: 'claude_code',
      workspace_path: '/tmp/demo',
      runtime_session_ref: 'c06c9a5e-b64c-4637-b28b-d424d0ddd754',
      tag: 'master',
      prompt_preview: '这是测试进程',
      has_runtime_session_ref: true,
      is_active: true,
    },
    runtimes: [
      {
        runtime_id: 'rt_1234567890',
        label: 'claude-main',
        agent_kind: 'claude_code',
        workspace_path: '/tmp/demo',
        runtime_session_ref: 'c06c9a5e-b64c-4637-b28b-d424d0ddd754',
        tag: 'master',
        prompt_preview: '这是测试进程',
        has_runtime_session_ref: true,
        is_active: true,
      },
    ],
  });

  assert.equal(message.kind, 'card');
  assert.equal(message.card.title, 'Runtime 控制');
  const body = JSON.stringify(message.card.blocks);
  assert.match(body, /当前运行会话/);
  assert.match(body, /c06c9a5e/);
  assert.match(body, /这是测试进程/);
});
