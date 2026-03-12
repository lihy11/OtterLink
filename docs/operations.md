# 运维说明

## 本地启动

推荐入口：

```bash
remoteagent start
```

首次部署建议先执行：

```bash
./scripts/install-one-click.sh
remoteagent configure
remoteagent doctor
```

`install-one-click.sh` 会在缺少依赖时自动安装 Rust `1.94.0` 和 Node `22.22.1`，并把 `~/.cargo/bin`、`~/.local/bin` 接入登录 shell。

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
sudo systemctl start remoteagent-core
sudo systemctl start remoteagent-gateway
```

查看状态：

```bash
sudo systemctl status remoteagent-core
sudo systemctl status remoteagent-gateway
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
sudo journalctl -u remoteagent-core -f
sudo journalctl -u remoteagent-gateway -f
```

## macOS launchd 运行

安装并启动：

```bash
./scripts/install-launchd.sh
```

查看状态：

```bash
launchctl print gui/$(id -u)/com.remoteagent.core
launchctl print gui/$(id -u)/com.remoteagent.gateway
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
remoteagent stop
./scripts/stop-longconn.sh
```

## 状态检查

```bash
remoteagent status
remoteagent doctor
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
- 如需验证 ACP `session/list` 是否可用，优先看 `rust.log` 中是否出现 `acp worker connected` 和后续 `list_sessions` 成功返回；只有 agent 不支持该能力时，才需要继续检查本地 `sqlite/jsonl` 回退来源。
- 如需验证 ACP turn 是否真正结束，优先看 `rust.log` 里的 `turn stop_reason:`。对 `codex-acp`，正常完成应为 `end_turn`；如果这里只有灰卡更新而没有 final 结果，先确认是否拿到了 `stop_reason=end_turn`。

## Runtime 控制自检

飞书里可直接发送：

```text
/runtime help
/runtime show
/runtime list
/runtime use codex
/runtime pick c06c9a5e
/runtime new
/runtime load
/runtime load /absolute/workspace
/runtime cwd ~/Desktop/InterestingPorjects/remoteagent/workspace
/runtime stop
/runtime proxy default
/runtime proxy on http://127.0.0.1:7890
/runtime proxy off
```

如果切换 agent 后没有看到候选会话，先检查：

1. `claude_code` 的目标 workspace 在 `CLAUDE_HOME_DIR/projects/` 下是否有对应目录。
2. 当前 ACP agent 是否声明了 `session/list` 和 `loadSession` 能力。
3. 只有 ACP `session/list` 不可用时，再检查 `claude_code` 的 `sessions-index.json` / `*.jsonl`，或 `codex` 的 `CODEX_HOME_DIR/state_5.sqlite`。
4. workspace 是否存在 `/tmp` 与 `/private/tmp` 这样的路径别名问题。
5. 还没有执行 `/runtime pick <short_id>` 或 `/runtime new` 时，普通消息不会进入 runtime。
6. `/runtime stop` 只停止当前 turn，不会切换已选 agent / cwd / session；对 ACP 会先走协议取消，再在超时后强制中断。
7. 如果 stop 之后 agent 还在请求权限，bridge 会直接返回 `Cancelled`，不会继续放行工具调用。
7. 如果 `codex` 需要联网但 ACP 无法访问外网，检查 `/runtime proxy` 当前模式，以及 `ACP_PROXY_URL` / `ALL_PROXY` 是否正确。

## 排障

1. core 无法启动
   检查 `CORE_BIND` 是否被占用，查看 `.run/rust.log`。
2. gateway 无法启动
   检查 `BIND`、`APP_ID`、`APP_SECRET`，查看 `.run/gateway.log`。
3. gateway 能启动但不收事件
   检查飞书机器人事件订阅和 `im.message.receive_v1` 权限。
4. Rust 有输出但飞书没回写
   检查 `GATEWAY_EVENT_TOKEN` 是否和 gateway 配置一致。
5. gateway 提交 turn 失败
   检查 `CORE_INGEST_TOKEN` 和 `CORE_BASE_URL`。
6. `systemctl reload` 后服务未恢复
   检查 `journalctl -u remoteagent-core -u remoteagent-gateway -n 200`，确认新 env 或新 binary 是否能正常启动。
7. CLI 配置后启动仍失败
   先执行 `remoteagent doctor`，确认 `APP_ID/APP_SECRET`、ACP 安装结果、PID 和 `healthz` 是否正常。

## 发布前检查

1. `cargo test`
2. `cd gateway && npm test`
3. 使用真实 `.run/feishu.env` 执行一次 live token smoke
4. 确认 `.run/` 和密钥未进入提交内容
