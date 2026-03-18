# 配置说明

## Gateway 侧

- `BIND`: gateway 监听地址，默认 `127.0.0.1:1127`
- `APP_ID`, `APP_SECRET`: 飞书机器人凭据
- 飞书应用需包含 `cardkit:card:write`，否则 CardKit 卡片无法创建
- `CORE_BASE_URL`: Rust core 地址，默认 `http://127.0.0.1:7211`
- `CORE_INGEST_TOKEN`: 保护 `gateway -> core`
- `GATEWAY_EVENT_TOKEN`: 保护 `core -> gateway`
- `BRIDGE_INGEST_TOKEN`: 保护 `/internal/feishu/event`
- `BRIDGE_NOTIFY_TOKEN`: 保护 `/internal/notify`
- `FEISHU_AUTH_MODE`: `off | pair | allow_from | pair_or_allow_from`
- `PAIR_AUTH_TOKEN`: 配对口令
- `ALLOW_FROM_OPEN_IDS`: 逗号分隔白名单
- `PAIR_STORE_PATH`: 配对存储路径
- `FEISHU_DISABLE_WS=1`: 仅启动 HTTP，不连飞书 WebSocket
- `FEISHU_WS_WATCHDOG_INTERVAL_MS`: WebSocket 健康巡检间隔，默认 `30000`
- `FEISHU_WS_STALL_TIMEOUT_MS`: 当飞书 SDK 的下一次重连时间已过且仍未恢复时，判定为卡死并重建 WS client，默认 `180000`
- `FEISHU_WS_IDLE_RESTART_MS`: 长连接在该时间内没有收到任何原始事件时，主动重建 WS client，默认 `300000`
- `otterlink configure` 会交互式维护 `.run/feishu.env`
- Linux `systemd` 部署时建议统一放进 `/etc/otterlink/otterlink.env`
- macOS `launchd` 默认会读取 `.run/feishu.env`，也可通过 `ENV_FILE=... ./scripts/install-launchd.sh` 覆盖

## Rust 侧

- `CORE_BIND`: Rust core 监听地址，默认 `127.0.0.1:7211`
- `STATE_DB_PATH`: sqlite 文件路径
- `CLAUDE_HOME_DIR`: Claude 本地 session 根目录，默认 `~/.claude`
- `CODEX_HOME_DIR`: Codex 本地 session 根目录，默认 `~/.codex`
- `ACP_PROXY_URL`: 运行时默认代理地址；为空时会回退读取 `ALL_PROXY / HTTPS_PROXY / HTTP_PROXY`
- `CLAUDE_CODE_DEFAULT_PROXY_MODE`: `on | off`，控制 `proxy=default` 时 `claude_code` 的默认代理
- `CODEX_DEFAULT_PROXY_MODE`: `on | off`，控制 `proxy=default` 时 `codex` 的默认代理
- `TODO_EVENT_LOG_PATH`: todo 事件日志路径
- `RENDER_MIN_UPDATE_MS`: progress 卡片最小刷新间隔

## Agent Runtime

- `RUNTIME_MODE`: `acp | exec_json | acp_fallback`
- `ACP_ADAPTER`: `claude_code | codex`，默认 `claude_code`
- `ACP_AGENT_CMD`: 显式覆盖 ACP 启动命令
- `claude_code` 默认 launcher 为 `npx -y @zed-industries/claude-code-acp@0.16.2`
- `acp_fallback` 不再自动回退；如果主 runtime 缺失、初始化失败或启动失败，core 会直接返回错误
- `codex` 默认 launcher 会显式安装 `@zed-industries/codex-acp@0.9.2` 与 `@zed-industries/codex-acp-linux-x64@0.9.2`，避免 `npx` 漏装平台 binary
- `CODEX_BIN`: `codex` 可执行文件
- `CODEX_WORKDIR`: runtime 工作目录
- `CODEX_MODEL`: 可选模型名
- `CODEX_SKIP_GIT_REPO_CHECK`: 是否跳过 git 检查

说明：

1. `CODEX_WORKDIR` 现在只是默认 workspace。
2. 单个聊天可通过 control 命令切换到其他 workspace。
3. 若当前 runtime 已经建立 `runtime_session_ref`，再切 workspace 会创建一个新的 runtime instance 并切过去。
4. `/ot load [workspace]` 对 `claude_code` 会从 `CLAUDE_HOME_DIR/projects/<workspace-key>/` 导入。
5. `/ot load [workspace]` 优先走 ACP `session/list`；只有 agent 不支持时，`claude_code` 才回退扫描 `sessions-index.json` / `*.jsonl`。
6. `codex` 只有在 ACP `session/list` 不可用时，才回退读取 `CODEX_HOME_DIR/state_5.sqlite` 的 `threads` 表按 `cwd` 导入。
7. macOS 上 `/tmp` 与 `/private/tmp` 的路径别名会同时尝试。
8. `/ot use <claude|codex>` 只切换 agent，不会隐式新建 session。
9. 普通消息前需要显式 `/ot pick <short_id>` 或 `/ot new`。
10. `/ot cwd <path>` 支持绝对路径、`~/...`，以及相对当前服务工作目录的相对路径。
11. `/ot stop` 会对当前活动 turn 发停止请求；ACP runtime 走协议取消，`exec_json` 走本地进程终止。
12. `/ot proxy <default|on|off> [proxy_url]` 会更新当前选择器的代理模式。
13. `default` 下会读取 `CLAUDE_CODE_DEFAULT_PROXY_MODE` / `CODEX_DEFAULT_PROXY_MODE`。

## 控制台工具

推荐先执行：

```bash
./scripts/install-one-click.sh
otterlink configure
otterlink doctor
```

控制台工具会：

1. 写入 `.run/feishu.env`
2. 扫描 ACP runtime
3. 安装缺失的 `claude_code` / `codex` ACP
4. 若缺少 Rust / Node，则自动安装 Rust `1.94.0` 与 Node `22.22.1`
5. 统一执行本地启动/停止/状态检查

## Linux 推荐路径

- env 文件：`/etc/otterlink/otterlink.env`
- state db：`/var/lib/otterlink/state.db`
- pairing：`/var/lib/otterlink/pairings.json`
- todo log：`/var/lib/otterlink/todo-events.jsonl`
- workspace：`/var/lib/otterlink/workspace`

## 推荐本地组合

```bash
export BIND='127.0.0.1:1127'
export CORE_BIND='127.0.0.1:7211'
export CORE_BASE_URL='http://127.0.0.1:7211'
export GATEWAY_EVENT_URL='http://127.0.0.1:1127/internal/gateway/event'
export CORE_INGEST_TOKEN='bridge_ingest_local_20260307'
export GATEWAY_EVENT_TOKEN='gateway_event_local_20260307'
export FEISHU_AUTH_MODE='pair_or_allow_from'
```

建议同时显式配置 `BIND`、`CORE_BIND`、`CORE_BASE_URL`、`GATEWAY_EVENT_URL`，不要只改其中一个端口。
