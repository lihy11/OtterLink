# 配置说明

## Gateway 侧

- `BIND`: gateway 监听地址，默认 `127.0.0.1:3000`
- `APP_ID`, `APP_SECRET`: 飞书机器人凭据
- 飞书应用需包含 `cardkit:card:write`，否则 CardKit 卡片无法创建
- `CORE_BASE_URL`: Rust core 地址，默认 `http://127.0.0.1:3001`
- `CORE_INGEST_TOKEN`: 保护 `gateway -> core`
- `GATEWAY_EVENT_TOKEN`: 保护 `core -> gateway`
- `BRIDGE_INGEST_TOKEN`: 保护 `/internal/feishu/event`
- `BRIDGE_NOTIFY_TOKEN`: 保护 `/internal/notify`
- `FEISHU_AUTH_MODE`: `off | pair | allow_from | pair_or_allow_from`
- `PAIR_AUTH_TOKEN`: 配对口令
- `ALLOW_FROM_OPEN_IDS`: 逗号分隔白名单
- `PAIR_STORE_PATH`: 配对存储路径
- `FEISHU_DISABLE_WS=1`: 仅启动 HTTP，不连飞书 WebSocket

## Rust 侧

- `CORE_BIND`: Rust core 监听地址，默认 `127.0.0.1:3001`
- `STATE_DB_PATH`: sqlite 文件路径
- `CLAUDE_HOME_DIR`: Claude 本地 session 根目录，默认 `~/.claude`
- `TODO_EVENT_LOG_PATH`: todo 事件日志路径
- `RENDER_MIN_UPDATE_MS`: progress 卡片最小刷新间隔

## Agent Runtime

- `RUNTIME_MODE`: `acp | exec_json | acp_fallback`
- `ACP_ADAPTER`: `codex | claude_code`
- `ACP_AGENT_CMD`: 显式覆盖 ACP 启动命令
- `CODEX_BIN`: `codex` 可执行文件
- `CODEX_WORKDIR`: runtime 工作目录
- `CODEX_MODEL`: 可选模型名
- `CODEX_SKIP_GIT_REPO_CHECK`: 是否跳过 git 检查

说明：

1. `CODEX_WORKDIR` 现在只是默认 workspace。
2. 单个聊天可通过 control 命令切换到其他 workspace。
3. 若当前 runtime 已经建立 `runtime_session_ref`，再切 workspace 会创建一个新的 runtime instance 并切过去。
4. `/runtime load [workspace]` 会从 `CLAUDE_HOME_DIR/projects/<workspace-key>/` 导入 Claude 历史 session。
5. 如果目录里没有 `sessions-index.json`，core 会回退扫描 `*.jsonl`。
6. macOS 上 `/tmp` 与 `/private/tmp` 的路径别名会同时尝试。

## 推荐本地组合

```bash
export BIND='127.0.0.1:3000'
export CORE_BIND='127.0.0.1:3001'
export CORE_INGEST_TOKEN='bridge_ingest_local_20260307'
export GATEWAY_EVENT_TOKEN='gateway_event_local_20260307'
export FEISHU_AUTH_MODE='pair_or_allow_from'
```
