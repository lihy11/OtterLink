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

test('runtime help command is handled in gateway without calling core', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_help',
      chat_id: 'oc_help',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/runtime help' }),
    },
  });

  assert.equal(result.kind, 'control_help');
  assert.equal(calls.submits.length, 0);
  assert.equal((calls.controls || []).length, 0);
  assert.equal(calls.cardReplies.length, 1);
});

test('unknown runtime subcommand is rejected in gateway and not forwarded to core or agent', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_bad_rt',
      chat_id: 'oc_bad_rt',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/runtime hel' }),
    },
  });

  assert.equal(result.kind, 'control_invalid');
  assert.equal(calls.submits.length, 0);
  assert.equal((calls.controls || []).length, 0);
  assert.equal(calls.replies.length, 1);
  assert.match(JSON.stringify(calls.replies[0].rendered), /Runtime 命令错误/);
  assert.match(JSON.stringify(calls.replies[0].rendered), /runtime help/);
});

test('runtime use without agent argument is rejected in gateway', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_use_missing',
      chat_id: 'oc_use_missing',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/runtime use' }),
    },
  });

  assert.equal(result.kind, 'control_invalid');
  assert.equal(calls.submits.length, 0);
  assert.equal((calls.controls || []).length, 0);
  assert.equal(calls.replies.length, 1);
  assert.match(JSON.stringify(calls.replies[0].rendered), /claude\|codex/);
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

test('runtime cwd command is routed to core control api as set_workspace', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_cwd',
      chat_id: 'oc_cwd',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/runtime cwd /tmp/demo-cwd' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.submits.length, 0);
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.controls[0].action, 'set_workspace');
  assert.equal(calls.controls[0].workspace_path, '/tmp/demo-cwd');
});

test('runtime use command is routed to core control api as use_agent', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_use',
      chat_id: 'oc_use',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/runtime use codex' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.controls[0].action, 'use_agent');
  assert.equal(calls.controls[0].agent_kind, 'codex');
});

test('runtime proxy command is routed to core control api as set_proxy', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_proxy',
      chat_id: 'oc_proxy',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/runtime proxy on http://127.0.0.1:7890' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.controls[0].action, 'set_proxy');
  assert.equal(calls.controls[0].proxy_mode, 'on');
  assert.equal(calls.controls[0].proxy_url, 'http://127.0.0.1:7890');
});

test('runtime stop command is routed to core control api as stop_runtime', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_stop',
      chat_id: 'oc_stop',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/runtime stop' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.controls[0].action, 'stop_runtime');
});

test('runtime pick command is routed to core control api as switch_runtime', async () => {
  const { service, calls } = makeService();
  const result = await service.handleFeishuEvent({
    sender: { sender_id: { open_id: 'ou_allow' } },
    message: {
      message_id: 'om_pick',
      chat_id: 'oc_pick',
      chat_type: 'p2p',
      content: JSON.stringify({ text: '/runtime pick c06c9a5e' }),
    },
  });

  assert.equal(result.kind, 'control');
  assert.equal(calls.controls.length, 1);
  assert.equal(calls.controls[0].action, 'switch_runtime');
  assert.equal(calls.controls[0].runtime_selector, 'c06c9a5e');
});

test('runtime pick sends a second history overview card when core returns session history', async () => {
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
      content: JSON.stringify({ text: '/runtime pick sess-his' }),
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
        throw new Error('core submit failed status=400 error=请先执行 /runtime pick <short_id> 或 /runtime new');
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
  assert.match(JSON.stringify(calls.replies[0].rendered), /runtime pick/);
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

test('renderRuntimeHelp lists supported runtime commands', async () => {
  const message = renderRuntimeHelp();
  assert.equal(message.kind, 'card');
  const body = JSON.stringify(message.card.blocks);
  assert.match(body, /runtime help/);
  assert.match(body, /runtime list/);
  assert.match(body, /runtime use/);
  assert.match(body, /runtime pick/);
  assert.match(body, /runtime cwd/);
  assert.match(body, /runtime proxy/);
  assert.match(body, /会话 帮助/);
});
