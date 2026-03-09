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
- `deploy/systemd/`: Linux `systemd` service templates and env example.
- `deploy/launchd/`: macOS `launchd` agent templates.
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

## Linux Service Deployment

线上建议直接使用 `systemd` 托管，不要再用 `nohup`。

```bash
cargo build --release
cd gateway && npm ci

sudo mkdir -p /etc/remoteagent /var/lib/remoteagent
sudo cp deploy/systemd/remoteagent.env.example /etc/remoteagent/remoteagent.env

sudo SERVICE_USER="$USER" \
  SERVICE_GROUP="$(id -gn)" \
  ENV_FILE=/etc/remoteagent/remoteagent.env \
  ./scripts/install-systemd.sh

sudo systemctl start remoteagent-core remoteagent-gateway
```

重载：

```bash
sudo ./scripts/reload-systemd.sh
```

`reload` 当前语义是发送 `SIGHUP`，服务会优雅退出并由 `systemd` 自动拉起新进程，适合 env 和 binary 更新后的切换。

## macOS LaunchAgent

macOS 建议使用 `launchd`，直接一键安装并启动：

```bash
./scripts/install-launchd.sh
```

如果 env 文件不在默认的 `.run/feishu.env`：

```bash
ENV_FILE=/absolute/path/to/remoteagent.env ./scripts/install-launchd.sh
```

重载：

```bash
./scripts/reload-launchd.sh
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
/runtime help
/runtime show
/runtime list
/runtime load
/runtime load /absolute/path
/runtime use codex
/runtime pick c06c9a5e
/runtime new my-claude
/runtime cwd ~/Desktop/InterestingPorjects/remoteagent/workspace
/runtime stop
/runtime proxy default
/runtime proxy on http://127.0.0.1:7890
/runtime proxy off
```

中文别名也支持：

```text
会话 帮助
会话 查看
会话 列表
会话 加载
会话 加载 /absolute/path
会话 切换 codex
会话 选择 c06c9a5e
会话 新建 my-claude
会话 工作区 ~/Desktop/InterestingPorjects/remoteagent/workspace
会话 停止
```

这些命令不会进入普通 agent turn，而是先走 core control API，更新当前聊天里的 runtime 选择器。
`/runtime help` 和 `会话 帮助` 由 gateway 本地处理，用于展示当前支持的命令清单。
`/runtime use <claude|codex>` 只切换 agent 并加载当前 `cwd` 下的候选会话，不会隐式新建会话。
`/runtime load [workspace]` 优先走 ACP `session/list`，按当前 `agent + cwd` 列出候选会话；如果 agent 不声明 `session/list` 能力，`claude_code` 才回退读取 `CLAUDE_HOME_DIR/projects/<workspace-key>/`，`codex` 才回退读取 `CODEX_HOME_DIR/state_5.sqlite`。ACP 真正恢复历史时会走协议 `session/load`；如果 agent 不声明 `loadSession` 能力，core 会直接报错，不会静默退回新会话。
`/runtime pick <short_id>` 在切换到已有 session 后，会额外发送一张 `历史概览` 卡片。该卡片来自 ACP `session/load` 时的历史回放，只展示最近 5 轮对话，并对 user / assistant 文本做首行裁剪。
`/runtime stop` 会停止当前正在运行的 turn。ACP runtime 会先发送协议 `session/cancel`，等待 prompt 以 `cancelled` 收尾，超时后才强制中断；同时对后续 `session/request_permission` 返回 `Cancelled`。正常结束以 `session/prompt` 的 `stop_reason=end_turn` 为准；`max_tokens / max_turn_requests / refusal / cancelled` 也会被单独记录。`exec_json` 兜底使用本地进程终止。
`/runtime proxy <default|on|off> [proxy_url]` 用于控制 ACP/exec 启动时的代理注入。默认策略是 `codex=on`、`claude_code=off`。
切换后需要显式执行 `/runtime pick <short_id>` 或 `/runtime new`，普通消息才会进入 runtime。
控制结果会以飞书卡片里的 Markdown 表格展示。
表格前只展示当前 `Agent` 和 `CWD` 摘要，表格列为 `状态 / Tag / 短ID / Prompt`。

## Key Environment Variables

- `BIND`: gateway bind address, default `127.0.0.1:1127`
- `CORE_BIND`: Rust core bind address, default `127.0.0.1:7211`
- `CORE_INGEST_TOKEN`: protects `gateway -> core`
- `GATEWAY_EVENT_TOKEN`: protects `core -> gateway`
- `APP_ID`, `APP_SECRET`: Feishu bot credentials
- `FEISHU_AUTH_MODE`: `off | pair | allow_from | pair_or_allow_from`
- `PAIR_AUTH_TOKEN`, `ALLOW_FROM_OPEN_IDS`, `PAIR_STORE_PATH`
- `RUNTIME_MODE`: `acp | exec_json | acp_fallback`
- `ACP_ADAPTER`: `claude_code | codex`，默认 `claude_code`
- `CLAUDE_HOME_DIR`: Claude 本地 session 根目录，默认 `~/.claude`
- `CODEX_HOME_DIR`: Codex 本地 session 根目录，默认 `~/.codex`
- `ACP_PROXY_URL`: 运行时默认代理地址；也会回退读取 `ALL_PROXY / HTTPS_PROXY / HTTP_PROXY`

See [docs/README.md](/Users/haiyangli/Desktop/InterestingPorjects/remoteagent/docs/README.md) for the full documentation set. ACP-specific protocol mapping is in [docs/acp.md](/Users/haiyangli/Desktop/InterestingPorjects/remoteagent/docs/acp.md). Linux deployment details are in [docs/installation.md](/Users/haiyangli/Desktop/InterestingPorjects/remoteagent/docs/installation.md), and macOS deployment details are in [docs/macos-installation.md](/Users/haiyangli/Desktop/InterestingPorjects/remoteagent/docs/macos-installation.md).
