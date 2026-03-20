const test = require('node:test');
const assert = require('node:assert/strict');

const { GatewayService } = require('../src/service');
const { renderControlResponse, renderRuntimeHelp } = require('../src/commands');

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
      async reactToMessage(messageId, emojiType) {
        calls.reactions = calls.reactions || [];
        calls.reactions.push({ messageId, emojiType });
        return messageId;
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
          selector: {
            agent_kind: 'claude_code',
            workspace_path: '/tmp/demo',
            has_selected_runtime: true,
          },
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
  assert.equal(calls.reactions.length, 1);
  assert.deepEqual(calls.reactions[0], { messageId: 'om_1', emojiType: 'OK' });
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

test('duplicate inbound message is ignored after first handling', async () => {
  const { service, calls } = makeService();
  const payload = {
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_dup_1',
      chat_id: 'oc_dup_1',
      chat_type: 'p2p',
      content: JSON.stringify({ text: 'hello once' }),
    },
  };

  const first = await service.handleFeishuEvent(payload);
  const second = await service.handleFeishuEvent(payload);

  assert.equal(first.accepted, true);
  assert.deepEqual(second, { ignored: true, reason: 'duplicate_message' });
  assert.equal(calls.submits.length, 1);
  assert.equal(calls.reactions.length, 1);
});

test('progress slot sends a new plain text message for each intermediate update', async () => {
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
        title: 'Codex 持续运行中',
        theme: 'grey',
        wide_screen_mode: true,
        update_multi: true,
        blocks: [
          { kind: 'markdown', text: '**🔄 正在运行**' },
          { kind: 'markdown', text: '🔄 已经开始处理本轮请求' },
          { kind: 'divider' },
          { kind: 'markdown', text: '📌 **最近输出摘录**\n\nfirst' },
        ],
      },
    },
  });
  await service.handleCoreEvent({
    turn_id: turnId,
    slot: 'progress',
    message: {
      kind: 'card',
      card: {
        title: 'Codex 持续运行中',
        theme: 'grey',
        wide_screen_mode: true,
        update_multi: true,
        blocks: [
          { kind: 'markdown', text: '**🔄 正在运行**' },
          { kind: 'markdown', text: '🛠️ 仍有 5 个工具调用在运行' },
          { kind: 'divider' },
          { kind: 'markdown', text: '📌 **最近输出摘录**\n\nfirst\nsecond' },
        ],
      },
    },
  });

  assert.equal(calls.cardReplies, undefined);
  assert.equal(calls.cardUpdates, undefined);
  assert.equal(calls.replies.length, 2);
  assert.match(calls.replies[0].rendered.content.text, /first/);
  assert.match(calls.replies[1].rendered.content.text, /second/);
  assert.doesNotMatch(calls.replies[0].rendered.content.text, /正在运行|最近输出摘录|已经开始处理本轮请求/);
  assert.doesNotMatch(calls.replies[1].rendered.content.text, /仍有 5 个工具调用在运行|最近输出摘录/);
});

test('progress slot ignores status-only updates with no real content', async () => {
  const { service, calls } = makeService();
  await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_progress_empty',
      chat_id: 'oc_progress_empty',
      chat_type: 'group',
      content: JSON.stringify({ text: 'hello' }),
    },
  });

  const turnId = calls.submits[0].turn_id;
  const result = await service.handleCoreEvent({
    turn_id: turnId,
    slot: 'progress',
    message: {
      kind: 'card',
      card: {
        title: 'Codex 持续运行中',
        theme: 'grey',
        wide_screen_mode: true,
        update_multi: true,
        blocks: [
          { kind: 'markdown', text: '**🔄 正在运行**' },
          { kind: 'markdown', text: '🔄 已经开始处理本轮请求' },
        ],
      },
    },
  });

  assert.equal(result.reason, 'empty_progress');
  assert.equal(calls.replies.length, 0);
});

test('todo slot keeps updating the same cardkit card', async () => {
  const { service, calls } = makeService();
  await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_hb',
      chat_id: 'oc_hb',
      chat_type: 'p2p',
      content: JSON.stringify({ text: 'hello' }),
    },
  });

  const turnId = calls.submits[0].turn_id;
  await service.handleCoreEvent({
    turn_id: turnId,
    slot: 'todo',
    message: {
      kind: 'card',
      card: {
        title: 'Todo',
        theme: 'orange',
        wide_screen_mode: true,
        update_multi: true,
        blocks: [{ kind: 'markdown', text: 'working' }],
      },
    },
  });
  await service.handleCoreEvent({
    turn_id: turnId,
    slot: 'todo',
    message: {
      kind: 'card',
      card: {
        title: 'Todo',
        theme: 'orange',
        wide_screen_mode: true,
        update_multi: true,
        blocks: [{ kind: 'markdown', text: 'done' }],
      },
    },
  });

  assert.equal(calls.cardReplies.length, 1);
  assert.equal(calls.cardUpdates.length, 1);
  assert.match(calls.cardUpdates[0].card.body.elements[0].content, /done/);
});

test('card heartbeat prepends waiting marker to existing todo streaming card', async () => {
  const { service, calls } = makeService();
  await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_hb',
      chat_id: 'oc_hb',
      chat_type: 'p2p',
      content: JSON.stringify({ text: 'hello' }),
    },
  });

  const turnId = calls.submits[0].turn_id;
  await service.handleCoreEvent({
    turn_id: turnId,
    slot: 'todo',
    message: {
      kind: 'card',
      card: {
        title: 'Todo',
        theme: 'orange',
        wide_screen_mode: true,
        update_multi: true,
        blocks: [{ kind: 'markdown', text: 'working' }],
      },
    },
  });

  await service.sendCardHeartbeat(turnId, 'todo');

  assert.equal(calls.cardUpdates.length, 1);
  assert.match(calls.cardUpdates[0].card.body.elements[0].content, /正在等待-1/);
});

test('todo card update failure falls back to plain text without affecting progress or final delivery', async () => {
  const { service, calls } = makeService({
    feishuClient: {
      async replyToMessage(messageId, rendered) {
        calls.replies.push({ messageId, rendered });
        return `reply_${calls.replies.length}`;
      },
      async reactToMessage(messageId, emojiType) {
        calls.reactions = calls.reactions || [];
        calls.reactions.push({ messageId, emojiType });
        return messageId;
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
      async updateCard() {
        throw new Error('feishu request failed status=200 code=300309 msg=ErrMsg: streaming mode is closed; ');
      },
      async sendToOpenId() {
        return 'notify_1';
      },
      async sendCardKitToOpenId() {
        return { messageId: 'notify_1', cardId: 'card_notify_1' };
      },
    },
  });

  await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_fallback',
      chat_id: 'oc_fallback',
      chat_type: 'p2p',
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
        title: 'Codex 持续运行中',
        theme: 'grey',
        wide_screen_mode: true,
        update_multi: true,
        blocks: [
          { kind: 'markdown', text: '**🔄 正在运行**' },
          { kind: 'markdown', text: '🛠️ 正在执行工具调用，当前活跃 1 个' },
          { kind: 'divider' },
          { kind: 'markdown', text: '📌 **最近输出摘录**\n\nfirst card body' },
        ],
      },
    },
  });
  await service.handleCoreEvent({
    turn_id: turnId,
    slot: 'todo',
    message: {
      kind: 'card',
      card: {
        title: 'Todo',
        theme: 'orange',
        wide_screen_mode: true,
        update_multi: true,
        blocks: [{ kind: 'markdown', text: 'todo first body' }],
      },
    },
  });
  await service.handleCoreEvent({
    turn_id: turnId,
    slot: 'todo',
    message: {
      kind: 'card',
      card: {
        title: 'Todo',
        theme: 'orange',
        wide_screen_mode: true,
        update_multi: true,
        blocks: [{ kind: 'markdown', text: 'todo updated body' }],
      },
    },
  });
  await service.handleCoreEvent({
    turn_id: turnId,
    slot: 'final',
    message: {
      kind: 'card',
      card: {
        title: 'Final',
        theme: 'green',
        wide_screen_mode: true,
        update_multi: false,
        blocks: [{ kind: 'markdown', text: 'final body' }],
      },
    },
  });

  assert.equal(calls.replies.length, 3);
  assert.match(calls.replies[0].rendered.content.text, /first card body/);
  assert.match(calls.replies[1].rendered.content.text, /卡片更新失败/);
  assert.match(calls.replies[2].rendered.content.text, /todo updated body/);
  assert.equal(calls.cardReplies.length, 2);
  assert.match(calls.cardReplies[1].card.body.elements[0].content, /final body/);
});

test('final card delivery failure falls back to plain text instead of crashing', async () => {
  const { service, calls } = makeService({
    feishuClient: {
      async replyToMessage(messageId, rendered) {
        calls.replies.push({ messageId, rendered });
        return `reply_${calls.replies.length}`;
      },
      async reactToMessage(messageId, emojiType) {
        calls.reactions = calls.reactions || [];
        calls.reactions.push({ messageId, emojiType });
        return messageId;
      },
      async replyCardKitToMessage(messageId, card) {
        calls.cardReplies = calls.cardReplies || [];
        calls.cardReplies.push({ messageId, card });
        if (card.body.elements[0].content.includes('final body')) {
          throw new Error('feishu request failed status=504 code=0 msg=timeout');
        }
        return { messageId: `reply_${calls.cardReplies.length}`, cardId: `card_${calls.cardReplies.length}` };
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
  });

  await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_final_fallback',
      chat_id: 'oc_final_fallback',
      chat_type: 'p2p',
      content: JSON.stringify({ text: 'hello' }),
    },
  });

  const turnId = calls.submits[0].turn_id;
  await service.handleCoreEvent({
    turn_id: turnId,
    slot: 'final',
    message: {
      kind: 'card',
      card: {
        title: 'Final',
        theme: 'green',
        wide_screen_mode: true,
        update_multi: false,
        blocks: [{ kind: 'markdown', text: 'final body' }],
      },
    },
  });

  assert.equal(calls.cardReplies.length, 1);
  assert.equal(calls.replies.length, 2);
  assert.match(calls.replies[0].rendered.content.text, /卡片更新失败/);
  assert.match(calls.replies[1].rendered.content.text, /final body/);
});

test('ot control command is routed to core control api instead of normal turn api', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_5',
      chat_id: 'oc_5',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot list' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.submits.length, 0);
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.cardReplies.length, 1);
});

test('control commands are serialized within the same session', async () => {
  const callOrder = [];
  let releaseFirst = null;
  const waitForFirst = new Promise((resolve) => {
    releaseFirst = resolve;
  });
  const { service } = makeService({
    coreClient: {
      async submitTurn(turnRequest) {
        return { ok: true, turn_id: turnRequest.turn_id };
      },
      async controlSession(controlRequest) {
        callOrder.push(controlRequest.action);
        if (controlRequest.action === 'set_workspace') {
          await waitForFirst;
        }
        return {
          ok: true,
          message: 'runtime ok',
          selector: {
            agent_kind: 'codex',
            workspace_path: controlRequest.workspace_path || '/tmp/demo',
            has_selected_runtime: false,
          },
          active_runtime: null,
          runtimes: [],
        };
      },
    },
  });

  const first = service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_serial_cwd',
      chat_id: 'oc_serial',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot cwd /tmp/demo-cwd' }),
    },
  });
  const second = service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_serial_show',
      chat_id: 'oc_serial',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot show' }),
    },
  });

  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(callOrder, ['set_workspace']);

  releaseFirst();
  await first;
  await second;
  assert.deepEqual(callOrder, ['set_workspace', 'show_runtime']);
});

test('ot cwd is routed to core control api', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_cwd_typo',
      chat_id: 'oc_cwd_typo',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot cwd ~/MultiPerspectiveCloneEval' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.submits.length, 0);
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.controls[0].action, 'set_workspace');
  assert.equal(calls.controls[0].workspace_path, '~/MultiPerspectiveCloneEval');
});

test('ot proxy command accepts shorthand url without explicit on', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_proxy_short',
      chat_id: 'oc_proxy_short',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot proxy http://127.0.0.1:7890' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.submits.length, 0);
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.controls[0].action, 'set_proxy');
  assert.equal(calls.controls[0].proxy_mode, 'on');
  assert.equal(calls.controls[0].proxy_url, 'http://127.0.0.1:7890');
});

test('ot help command is handled in gateway without calling core', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_help',
      chat_id: 'oc_help',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot help' }),
    },
  });

  assert.equal(result.kind, 'control_help');
  assert.equal(calls.submits.length, 0);
  assert.equal((calls.controls || []).length, 0);
  assert.equal(calls.cardReplies.length, 1);
});

test('unknown ot subcommand is rejected in gateway and not forwarded to core or agent', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_bad_rt',
      chat_id: 'oc_bad_rt',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot hel' }),
    },
  });

  assert.equal(result.kind, 'control_invalid');
  assert.equal(calls.submits.length, 0);
  assert.equal((calls.controls || []).length, 0);
  assert.equal(calls.replies.length, 1);
  assert.match(JSON.stringify(calls.replies[0].rendered), /Runtime 命令错误/);
  assert.match(JSON.stringify(calls.replies[0].rendered), /ot help/);
});

test('ot use without agent argument is rejected in gateway', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_use_missing',
      chat_id: 'oc_use_missing',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot use' }),
    },
  });

  assert.equal(result.kind, 'control_invalid');
  assert.equal(calls.submits.length, 0);
  assert.equal((calls.controls || []).length, 0);
  assert.equal(calls.replies.length, 1);
  assert.match(JSON.stringify(calls.replies[0].rendered), /claude\|codex/);
});

test('ot load command is routed to core control api with workspace path', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_6',
      chat_id: 'oc_6',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot load /tmp/demo-workspace' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.submits.length, 0);
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.controls[0].action, 'load_runtimes');
  assert.equal(calls.controls[0].workspace_path, '/tmp/demo-workspace');
});

test('ot cwd command is routed to core control api as set_workspace', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_cwd',
      chat_id: 'oc_cwd',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot cwd /tmp/demo-cwd' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.submits.length, 0);
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.controls[0].action, 'set_workspace');
  assert.equal(calls.controls[0].workspace_path, '/tmp/demo-cwd');
});

test('ot use command is routed to core control api as use_agent', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_use',
      chat_id: 'oc_use',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot use codex' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.controls[0].action, 'use_agent');
  assert.equal(calls.controls[0].agent_kind, 'codex');
});

test('ot proxy command is routed to core control api as set_proxy', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_proxy',
      chat_id: 'oc_proxy',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot proxy on http://127.0.0.1:7890' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.controls[0].action, 'set_proxy');
  assert.equal(calls.controls[0].proxy_mode, 'on');
  assert.equal(calls.controls[0].proxy_url, 'http://127.0.0.1:7890');
});

test('ot stop command is routed to core control api as stop_runtime', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_stop',
      chat_id: 'oc_stop',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot stop' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.controls[0].action, 'stop_runtime');
});

test('ot pick command is routed to core control api as switch_runtime', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_pick',
      chat_id: 'oc_pick',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot pick c06c9a5e' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.controls[0].action, 'switch_runtime');
  assert.equal(calls.controls[0].runtime_selector, 'c06c9a5e');
});

test('ot pick sends a second history overview card when core returns session history', async () => {
  const { service, calls } = makeService({
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
          message: '已切换到 `sess-history`。',
          selector: {
            agent_kind: 'codex',
            workspace_path: '/tmp/demo',
            has_selected_runtime: true,
            proxy_mode: 'default',
            proxy_url: null,
          },
          active_runtime: {
            runtime_id: 'rt_history',
            runtime_session_ref: 'sess-history',
            label: 'codex-demo',
            agent_kind: 'codex',
            workspace_path: '/tmp/demo',
            tag: 'main',
            prompt_preview: 'latest task',
            has_runtime_session_ref: true,
            is_active: true,
          },
          runtimes: [],
          history_overview: {
            runtime_session_ref: 'sess-history',
            turns: [
              { user_text: '先检查配置', assistant_text: '已经检查配置' },
              { user_text: '再看看日志', assistant_text: '日志里没有新的异常' },
            ],
          },
        };
      },
    },
  });
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_pick_history',
      chat_id: 'oc_pick_history',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/ot pick sess-his' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.cardReplies.length, 2);
  const historyPayload = JSON.stringify(calls.cardReplies[1].card.body.elements);
  assert.match(historyPayload, /历史概览/);
  assert.match(historyPayload, /user:/);
  assert.match(historyPayload, /assistant:/);
});

test('turn rejection from core is replied back to feishu user', async () => {
  const { service, calls } = makeService({
    coreClient: {
      async submitTurn() {
        throw new Error('core submit failed status=400 error=请先执行 /ot pick <short_id> 或 /ot new');
      },
      async controlSession() {
        throw new Error('should not be called');
      },
    },
  });

  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_turn_reject',
      chat_id: 'oc_turn_reject',
      chat_type: 'p2p',
      content: JSON.stringify({ text: 'hello' }),
    },
  });

  assert.equal(result.reason, 'turn_rejected');
  assert.equal(calls.replies.length, 1);
  assert.match(JSON.stringify(calls.replies[0].rendered), /ot pick/);
});

test('renderControlResponse builds a card with runtime rows', async () => {
  const message = renderControlResponse({
    ok: true,
    message: 'loaded',
    selector: {
      agent_kind: 'claude_code',
      workspace_path: '/tmp/demo',
      has_selected_runtime: true,
      proxy_mode: 'default',
      proxy_url: 'http://127.0.0.1:7890',
    },
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
  assert.match(body, /当前选择器/);
  assert.match(body, /Proxy:/);
  assert.match(body, /当前已选会话/);
  assert.match(body, /状态 \\| Tag \\| 短ID \\| Prompt/);
  assert.match(body, /c06c9a5e/);
  assert.match(body, /这是测试进程/);
  assert.match(body, /Agent: `claude_code`/);
  assert.match(body, /CWD: `\/tmp\/demo`/);
});

test('renderRuntimeHelp lists supported ot commands', async () => {
  const message = renderRuntimeHelp();
  assert.equal(message.kind, 'card');
  const body = JSON.stringify(message.card.blocks);
  assert.match(body, /ot help/);
  assert.match(body, /ot list/);
  assert.match(body, /ot use/);
  assert.match(body, /ot pick/);
  assert.match(body, /ot cwd/);
  assert.match(body, /ot proxy/);
  assert.match(body, /会话 帮助/);
});
