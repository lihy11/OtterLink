# 接口说明

## Gateway -> Core

### `POST /internal/core/inbound`

Gateway 统一把认证成功后的文本消息转发到这个入口。Rust core 负责判断它是：

- `/ot ...` 控制命令
- 普通 agent turn
- 需要立即返回的帮助或错误消息

Header:

- `x-core-ingest-token: <CORE_INGEST_TOKEN>` 可选但推荐

Request:

```json
{
  "session_key": "feishu:thread:oc_xxx:th_xxx",
  "parent_session_key": "feishu:chat:oc_xxx",
  "text": "/ot show"
}
```

Response:

```json
{
  "turn_id": null,
  "replies": [
    {
      "kind": "card",
      "card": {
        "title": "Runtime 控制",
        "theme": "grey",
        "wide_screen_mode": true,
        "update_multi": false,
        "blocks": [{ "kind": "markdown", "text": "..." }]
      }
    }
  ],
  "react_to_message": false
}
```

普通消息会返回：

```json
{
  "turn_id": "turn_xxx",
  "replies": [],
  "react_to_message": true
}
```

### `POST /internal/core/turn`

保留给内部测试和底层调用。生产消息入口优先使用 `/internal/core/inbound`。

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

保留给内部测试和底层调用。`/ot ...` 在生产链路中不再由 gateway 解析，而是由 Rust 在 `/internal/core/inbound` 内部转成 control request。

Request:

```json
{
  "session_key": "feishu:thread:oc_xxx:th_xxx",
  "action": "load_runtimes",
  "workspace_path": "/Users/haiyangli/Desktop/InterestingPorjects/otterlink/workspace"
}
```

Response:

```json
{
  "ok": true,
  "message": "已切换到 `codex`，请从下方选择会话，或执行 `/ot new`。",
  "selector": {
    "agent_kind": "codex",
    "workspace_path": "/Users/.../otterlink",
    "has_selected_runtime": false,
    "proxy_mode": "default",
    "proxy_url": null
  },
  "active_runtime": null,
  "runtimes": [],
  "history_overview": null
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

- turn 被 core 接受后，gateway 会优先对用户原消息补一个飞书表情回复，表示任务已启动
- `progress` 槽位现在优先接收 core 发来的 assistant-message 级文本；core 会先把 chunk 聚合成完整中间消息，再交给 gateway 发送
- `progress` 普通消息只保留真实中间文本增量，不再包含 `Codex 持续运行中`、`正在运行`、`最近输出摘录`、工具调用统计等包装信息
- `text / post / raw` 仍走普通 `im/v1/messages` 消息接口
- `todo` / `final` 等卡片槽位继续走 Feishu `CardKit`：
  1. `POST /open-apis/cardkit/v1/cards`
  2. `POST /open-apis/im/v1/messages/...` 引用 `card_id`
  3. `PUT /open-apis/cardkit/v1/cards/{card_id}/elements/content/content`
  4. `PATCH /open-apis/cardkit/v1/cards/{card_id}/settings`
- 消息表情回复走 `POST /open-apis/im/v1/messages/{message_id}/reactions`
- 若 `todo` / `final` 卡片发送或更新失败，gateway 会自动回退为普通文本消息继续发送
- 若 runtime 没有单独给出最终消息，core 只会把最后一段 assistant 文本作为 `final`，不会把整轮 `progress` 全量拼进最终结果，也不会把同一段最终文本重复发送到 `progress` 和 `final`

## Gateway Local APIs

### `GET /healthz`
返回 `{ "ok": true }`

### `POST /internal/feishu/event`
用于 SDK 或本地调试把飞书事件送入 gateway。

### `POST /internal/notify`
向显式 `open_id` 或默认已配对用户发送主动消息。

## Local CLI Interface

源码部署后的本地控制台入口为：

```bash
otterlink configure
otterlink install-acp all --if-missing
otterlink doctor
otterlink start
otterlink stop
otterlink status
```

说明：

- `configure` 负责写 `.run/feishu.env`
- `install-acp` 负责扫描/安装 `claude_code` 与 `codex` ACP runtime
- `start/stop` 仍复用仓库脚本，但由统一 CLI 传入 `OTTERLINK_ENV_FILE`

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

- `/ot help`
- `/ot show`
- `/ot list`
- `/ot load [workspace]`
- `/ot use <claude|codex>`
- `/ot pick <runtime_id|runtime_session_ref 前缀|label>`
- `/ot new <label>`
- `/ot cwd <path>`
- `/ot stop`
- `/ot proxy <default|on|off> [proxy_url]`

`/ot use <claude|codex>` 只切换当前 agent，并自动加载当前 `cwd` 下的候选会话。
`/ot pick` 才会显式选定会话。
`/ot pick` 成功后，如果 ACP `session/load` 提供了历史回放，gateway 会额外发送一张 `历史概览` 卡片，显示裁剪后的最近 5 轮 `- user / - assistant` 对话。
`/ot stop` 会停止当前正在运行的 turn；ACP 会先发送 `session/cancel` 并等待 `cancelled` 收尾，超时后才强制中断；`exec_json` 直接终止本地进程。
`/ot proxy` 会更新当前选择器的代理策略，影响后续 ACP/exec 启动时注入的 `HTTP_PROXY / HTTPS_PROXY / ALL_PROXY`。
`/ot load` 会优先按当前 agent 调用 ACP `session/list`：

- `claude_code`: 不支持 `session/list` 时，回退到 `CLAUDE_HOME_DIR/projects/<workspace-key>/`
- `codex`: 不支持 `session/list` 时，回退到 `CODEX_HOME_DIR/state_5.sqlite` 的 `threads` 表按 `cwd` 过滤

ACP 真正恢复历史时会调用 `session/load`。如果 agent 没有声明 `loadSession` 能力，core 会直接报错，不会静默创建新会话。
取消时，ACP client 会先发 `session/cancel`，随后对任何新的 `session/request_permission` 返回 `Cancelled`。
ACP 单轮是否结束，以 `session/prompt` 的 `PromptResponse.stop_reason` 为准。`codex-acp` 的正常收尾是 `end_turn`；`cancelled / max_tokens / max_turn_requests / refusal` 也都会被记录到 runtime completion。
ACP worker 使用持久连接，`initialize` 在 worker 建立时只执行一次，不再每轮重启 agent 进程。

`/ot help`、拼错的 `/ot` 子命令和缺参数情况，都由 Rust 在 `/internal/core/inbound` 里直接返回即时回复，不进入普通 agent turn。
优先使用 `sessions-index.json`；如果索引不存在，会回退扫描 `*.jsonl` 头部元数据。
控制结果会渲染为 Markdown 表格卡片。
表格前只保留当前 `Agent / CWD / Proxy / Session` 摘要，表格列为 `状态 / Tag / 短ID / Prompt`。
`历史概览` 则单独使用一张灰色卡片，避免把历史回放混进当前 turn 的灰卡或绿卡。

统一事件包括：

- `TurnStarted`
- `TurnCompleted`
- `RuntimeSessionReady`
- `AssistantChunk`
- `AssistantMessage`
- `ToolState`
- `PlanUpdated`
- `Usage`
