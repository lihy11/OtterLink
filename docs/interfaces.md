# 接口说明

## Gateway -> Core

### `POST /internal/core/turn`

Header:

- `x-core-ingest-token: <CORE_INGEST_TOKEN>` 可选但推荐

Request:

```json
{
  "turn_id": "turn_xxx",
  "session_key": "feishu:thread:oc_xxx:th_xxx",
  "parent_session_key": "feishu:chat:oc_xxx",
  "text": "继续这个话题"
}
```

Response:

```json
{
  "ok": true,
  "turn_id": "turn_xxx"
}
```

### `POST /internal/core/control`

用于查看、导入、切换 runtime，或调整 workspace。

Request:

```json
{
  "session_key": "feishu:thread:oc_xxx:th_xxx",
  "action": "load_runtimes",
  "workspace_path": "/Users/haiyangli/Desktop/InterestingPorjects/remoteagent/workspace"
}
```

Response:

```json
{
  "ok": true,
  "message": "当前 workspace 已切换到 `/Users/.../remoteagent`。",
  "active_runtime": {
    "runtime_id": "rt_xxx",
    "label": "claude_code-remoteagent",
    "agent_kind": "claude_code",
    "workspace_path": "/Users/.../remoteagent",
    "runtime_session_ref": "c06c9a5e-b64c-4637-b28b-d424d0ddd754",
    "tag": "master",
    "prompt_preview": "这是测试进程，你规划一下...",
    "has_runtime_session_ref": true,
    "is_active": true
  },
  "runtimes": []
}
```

## Core -> Gateway

### `POST /internal/gateway/event`

Header:

- `x-gateway-event-token: <GATEWAY_EVENT_TOKEN>` 可选但推荐

Request:

```json
{
  "turn_id": "turn_xxx",
  "slot": "progress",
  "message": {
    "kind": "card",
    "card": {
      "title": "Turn Progress",
      "theme": "blue",
      "wide_screen_mode": true,
      "update_multi": true,
      "blocks": [{ "kind": "markdown", "text": "处理中" }]
    }
  }
}
```

说明：

- `text / post / raw` 仍走普通 `im/v1/messages` 消息接口
- `card` 现在走 Feishu `CardKit`：
  1. `POST /open-apis/cardkit/v1/cards`
  2. `POST /open-apis/im/v1/messages/...` 引用 `card_id`
  3. `PUT /open-apis/cardkit/v1/cards/{card_id}/elements/content/content`
  4. `PATCH /open-apis/cardkit/v1/cards/{card_id}/settings`

## Gateway Local APIs

### `GET /healthz`
返回 `{ "ok": true }`

### `POST /internal/feishu/event`
用于 SDK 或本地调试把飞书事件送入 gateway。

### `POST /internal/notify`
向显式 `open_id` 或默认已配对用户发送主动消息。

## 标准消息模型

### `OutboundMessage`

- `text`
- `post`
- `card`
- `raw`

### `StandardCard`

- `title`
- `theme`: `blue | green | red | wathet`
- `wide_screen_mode`
- `update_multi`
- `blocks`: `markdown | divider`

## 运行时接口

Rust core 通过 `AgentRuntime` 统一调用 agent。

- 输入：`RuntimeTurnRequest { prompt, runtime_session_ref, agent_kind, workspace_path }`
- 输出：`RuntimeTurn { events, completion }`

## Runtime 控制命令

Gateway 当前支持：

- `/runtime show`
- `/runtime list`
- `/runtime load [workspace]`
- `/runtime new <label>`
- `/runtime use <runtime_id|runtime_session_ref 前缀|label>`
- `/workspace set <absolute_path>`

`/runtime load` 会从 `CLAUDE_HOME_DIR/projects/<workspace-key>/` 读取 Claude 本地 session。
优先使用 `sessions-index.json`；如果索引不存在，会回退扫描 `*.jsonl` 头部元数据。
控制结果会渲染为分行卡片，逐条列出 `Agent / Tag / 会话 / 短ID / Prompt`。

统一事件包括：

- `TurnStarted`
- `TurnCompleted`
- `RuntimeSessionReady`
- `AssistantChunk`
- `AssistantMessage`
- `ToolState`
- `PlanUpdated`
- `Usage`
