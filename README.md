# remoteagent

`remoteagent` is split into two runtime boundaries:

1. Rust `agent + core`
2. Node.js `gateway`

The gateway owns Feishu integration, pairing, allow-list auth, session-key calculation, and message rendering. The Rust service owns local agent runtime execution, session persistence, turn orchestration, and standardized outbound events.

## Current Architecture

- `src/agent/`: ACP / `codex exec --json` runtimes and event normalization.
- `src/core/`: session registry, sqlite persistence, prompt assembly, turn state, and outbound message generation.
- `src/api/`: `POST /internal/core/turn` ingress from the gateway.
- `gateway/`: Feishu gateway service, official SDK/WebSocket client, auth/pairing, and Feishu message delivery.
- `docs/`: synchronized design and operator documentation.

## Message Boundary

Gateway -> Rust sends only trusted turn requests:

```json
{
  "turn_id": "turn_xxx",
  "session_key": "feishu:thread:oc_xxx:th_xxx",
  "parent_session_key": "feishu:chat:oc_xxx",
  "text": "请继续这个话题"
}
```

Rust -> Gateway emits standardized slot updates:

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

Feishu card delivery now uses `CardKit`:

1. gateway creates a `cardkit/v1/cards` entity
2. sends an `interactive` message that references `card_id`
3. updates card content through `cardkit` element/settings APIs

## Local Start

```bash
source .run/feishu.env
./scripts/start-longconn.sh
```

Stop:

```bash
./scripts/stop-longconn.sh
```

## Test Commands

```bash
cargo test
cd gateway && npm test
source .run/feishu.env && cd gateway && node --test test/feishu-live.test.js
./scripts/test-local-acp.sh
```

The live Feishu smoke only verifies official API auth/token acquisition unless you provide a real paired recipient for delivery tests.

## Runtime 控制命令

在飞书里可直接发送：

```text
/runtime show
/runtime list
/runtime load
/runtime load /absolute/path
/runtime new my-claude
/runtime use my-claude
/runtime use c06c9a5e
/workspace set /absolute/path
```

中文别名也支持：

```text
会话 查看
会话 列表
会话 加载
会话 加载 /absolute/path
会话 新建 my-claude
会话 切换 my-claude
会话 切换 c06c9a5e
工作区 设置 /absolute/path
```

这些命令不会进入普通 agent turn，而是先走 core control API，更新当前聊天对应的 active runtime。
控制结果会以飞书卡片分行展示 `Agent / Tag / 会话 / 短ID / Prompt`。

## Key Environment Variables

- `BIND`: gateway bind address, default `127.0.0.1:3000`
- `CORE_BIND`: Rust core bind address, default `127.0.0.1:3001`
- `CORE_INGEST_TOKEN`: protects `gateway -> core`
- `GATEWAY_EVENT_TOKEN`: protects `core -> gateway`
- `APP_ID`, `APP_SECRET`: Feishu bot credentials
- `FEISHU_AUTH_MODE`: `off | pair | allow_from | pair_or_allow_from`
- `PAIR_AUTH_TOKEN`, `ALLOW_FROM_OPEN_IDS`, `PAIR_STORE_PATH`
- `RUNTIME_MODE`: `acp | exec_json | acp_fallback`
- `ACP_ADAPTER`: `codex | claude_code`
- `CLAUDE_HOME_DIR`: Claude 本地 session 根目录，默认 `~/.claude`

See [docs/README.md](/Users/haiyangli/Desktop/InterestingPorjects/remoteagent/docs/README.md) for the full documentation set.
