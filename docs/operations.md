# 运维说明

## 本地启动

推荐入口：

```bash
otterlink start
```

首次部署建议先执行：

```bash
./scripts/install-one-click.sh
otterlink configure
otterlink doctor
```

`install-one-click.sh` 会在缺少依赖时自动安装 Rust `1.94.0` 和 Node `22.22.1`，并把 `~/.cargo/bin`、`~/.local/bin` 接入登录 shell。
本地启动脚本只负责拉起现有构建产物和依赖，不再在启动过程中执行 `npm install`。

兼容保留的原始入口：

```bash
source .run/feishu.env
./scripts/start-longconn.sh
```

启动约束：

1. 默认端口固定为 gateway `1127`、core `7211`，用来避开常见本地开发端口冲突。
2. 不要只设置 `BIND` 而漏掉 `CORE_BIND`，否则 core 可能回退到错误端口并与 gateway 冲突。
3. `scripts/run-core.sh` 和 `scripts/run-gateway.sh` 会自动加载 `.run/feishu.env`，并在缺省时回退到 `1127/7211`。
4. `scripts/start-longconn.sh` 现在直接复用这两个稳定入口，而不是再走一层临时 shell。

服务地址：

- gateway: `http://127.0.0.1:1127`
- core: `http://127.0.0.1:7211`

## Linux systemd 运行

线上建议使用：

```bash
sudo systemctl start otterlink-core
sudo systemctl start otterlink-gateway
```

查看状态：

```bash
sudo systemctl status otterlink-core
sudo systemctl status otterlink-gateway
```

受控重载：

```bash
sudo ./scripts/reload-systemd.sh
```

说明：

- `ExecReload` 会发送 `SIGHUP`
- core 和 gateway 收到后会优雅退出
- `Restart=always` 会让 `systemd` 自动拉起新进程

日志：

```bash
sudo journalctl -u otterlink-core -f
sudo journalctl -u otterlink-gateway -f
```

## macOS launchd 运行

安装并启动：

```bash
./scripts/install-launchd.sh
```

查看状态：

```bash
launchctl print gui/$(id -u)/com.otterlink.core
launchctl print gui/$(id -u)/com.otterlink.gateway
```

重载：

```bash
./scripts/reload-launchd.sh
```

日志：

```bash
tail -f .run/core.launchd.log
tail -f .run/gateway.launchd.log
```

## 停止

```bash
otterlink stop
./scripts/stop-longconn.sh
```

## 状态检查

```bash
otterlink status
otterlink doctor
```

## 测试

```bash
cargo test
cd gateway && npm test
source .run/feishu.env && cd gateway && node --test test/feishu-live.test.js
./scripts/test-local-acp.sh
```

说明：

- Rust 测试覆盖 session / turn / protocol 聚合。
- Node 测试覆盖 auth、pairing、session routing、render、gateway service。
- `feishu-live.test.js` 会真实调用飞书获取 tenant token。
- 如需验证 Claude 历史 session 导入，确认 `CLAUDE_HOME_DIR` 指向本机真实 `~/.claude` 或测试目录。
- 如需验证 `claude_code` 的 ACP `session/list` 是否可用，优先看 `rust.log` 中是否出现 `acp worker connected` 和后续 `list_sessions` 成功返回；只有 agent 不支持该能力时，才需要继续检查本地 `sqlite/jsonl` 回退来源。
- 如需验证 `codex` turn 是否真正结束，优先看 `rust.log` 里的 `turn stop_reason:` 和 `codex app-server` 相关日志；当前完成信号来自 app-server `turn/completed`。

## Runtime 控制自检

飞书里可直接发送：

```text
/ot help
/ot show
/ot list
/ot use codex
/ot pick c06c9a5e
/ot new
/ot load
/ot load /absolute/workspace
/ot cwd ~/Desktop/InterestingPorjects/otterlink/workspace
/ot stop
/ot proxy default
/ot proxy on http://127.0.0.1:7890
/ot proxy off
```

如果切换 agent 后没有看到候选会话，先检查：

1. `claude_code` 的目标 workspace 在 `CLAUDE_HOME_DIR/projects/` 下是否有对应目录。
2. `claude_code` 当前 ACP agent 是否声明了 `session/list` 和 `loadSession` 能力。
3. `codex` 当前会优先通过 app-server `thread/list` / `thread/read` 读取会话和最近历史；只有运行时能力不可用时才回退检查 `CODEX_HOME_DIR/state_5.sqlite`。`claude_code` 仍会检查 `sessions-index.json` / `*.jsonl` 回退来源。
4. workspace 是否存在 `/tmp` 与 `/private/tmp` 这样的路径别名问题。
5. 还没有执行 `/ot pick <short_id>` 或 `/ot new` 时，普通消息不会进入 runtime。
6. `/ot stop` 只停止当前 turn，不会切换已选 agent / cwd / session；对 ACP 会先走协议取消，再在超时后强制中断。
7. 如果 stop 之后 agent 还在请求权限，bridge 会直接返回 `Cancelled`，不会继续放行工具调用。
8. 如果 `codex` 需要联网但 app-server 无法访问外网，检查 `/ot proxy` 当前模式，以及 `ACP_PROXY_URL` / `ALL_PROXY` 是否正确。
9. 如果 `codex` 运行中发送普通文本消息后返回“已将补充消息发送给当前 Codex 任务”，这是预期行为：Rust 已把该消息转成 app-server `turn/steer`，不会新建下一轮队列 turn。
10. 当前 `codex app-server` 运行策略固定为 `approvalPolicy=never` 且 `danger full access`；如果仍看到审批相关报错，应优先怀疑协议参数漂移或上游 CLI 版本变化，而不是当前配置未开启自动执行。

## 排障

1. core 无法启动
   检查 `CORE_BIND` 是否被占用，查看 `.run/rust.log`。
2. gateway 无法启动
   检查 `BIND`、`APP_ID`、`APP_SECRET`，查看 `.run/gateway.log`。
3. gateway 能启动但不收事件
   检查飞书机器人事件订阅和 `im.message.receive_v1` 权限。
   若 `.run/gateway.log` 出现 `getaddrinfo EAI_AGAIN open.feishu.cn` 且后续没有新的 inbound event，说明长连接重连失败；当前 gateway 会在重连计划超时后自动重建 WS client。
   `FEISHU_WS_IDLE_RESTART_MS` 默认关闭。空闲本身不是故障，不能把“没有新的 `ws raw event`”当成长连接失效的直接证据；否则会把正常空闲连接误判为故障并反复重建。
   当前 gateway 会按 `message_id` 做进程内去重；如果同一条飞书事件被重推，日志里会出现 `duplicate feishu event ignored`。
   若飞书聊天里看到某条 `/ot ...` 没有生效，先按下面顺序检查：
   - 有 `ws raw event`：说明飞书 SDK 已经收到事件
   - 有 `inbound feishu event`：说明 gateway 业务入口已收到事件
   - 有 `replied message`：说明 gateway 已成功调用飞书发送回复
   - 若缺少 `ws raw event`，问题在 gateway 之前，常见原因是长连接静默失联、网络/DNS 抖动，或同一飞书应用存在其他长连接客户端竞争消息
   - 若 `show` 返回旧 workspace，而前面刚发过 `cwd`，优先检查对应 `cwd` 是否真的有 `ws raw event`；如果没有，说明不是状态覆盖，而是那条命令根本没投递到当前实例
4. Rust 有输出但飞书没回写
   检查 `GATEWAY_EVENT_TOKEN` 是否和 gateway 配置一致。
5. gateway 提交 turn 失败
   检查 `CORE_INGEST_TOKEN` 和 `CORE_BASE_URL`。当前 `/ot` 控制命令和普通消息都会统一转发到 Rust `/internal/core/inbound`，所以 core 不可用时二者会一起失效。
6. `systemctl reload` 后服务未恢复
   检查 `journalctl -u otterlink-core -u otterlink-gateway -n 200`，确认新 env 或新 binary 是否能正常启动。
7. CLI 配置后启动仍失败
   先执行 `otterlink doctor`，确认 `APP_ID/APP_SECRET`、ACP 安装结果、PID 和 `healthz` 是否正常。

## 发布前检查

1. `cargo test`
2. `cd gateway && npm test`
3. 使用真实 `.run/feishu.env` 执行一次 live token smoke
4. 确认 `.run/` 和密钥未进入提交内容
