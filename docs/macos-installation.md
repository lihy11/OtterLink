# macOS 安装与服务化

## 目标

在 macOS 上推荐使用 `launchd`，而不是手工开两个终端或 `nohup`。

适合这个项目的原因：

- 以当前登录用户身份运行，能直接访问 `~/.claude`
- 也能直接访问 `~/.codex`
- 登录后自动拉起
- 进程退出后自动保活
- 可用 `launchctl kickstart -k` 做受控重启

运行后可通过飞书 `/ot stop` 停止当前 turn；ACP runtime 会走协议取消，不依赖终端前台信号。

## 一键安装并启动

推荐先用控制台工具生成 `.run/feishu.env`：

```bash
./scripts/install-one-click.sh
otterlink configure
```

然后执行：

```bash
./scripts/install-launchd.sh
```

如果 env 文件不在默认位置：

```bash
ENV_FILE=/absolute/path/to/otterlink.env ./scripts/install-launchd.sh
```

env 文件里至少应同时定义：

```bash
export BIND='127.0.0.1:1127'
export CORE_BIND='127.0.0.1:7211'
export CORE_BASE_URL='http://127.0.0.1:7211'
export GATEWAY_EVENT_URL='http://127.0.0.1:1127/internal/gateway/event'
```

它会完成：

1. 渲染两个 plist 到 `~/Library/LaunchAgents/`
2. `bootstrap` 两个服务
3. 立即 `kickstart`

## 管理命令

查看状态：

```bash
launchctl print gui/$(id -u)/com.otterlink.core
launchctl print gui/$(id -u)/com.otterlink.gateway
```

重载：

```bash
./scripts/reload-launchd.sh
```

停止并卸载：

```bash
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.otterlink.core.plist
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.otterlink.gateway.plist
```

## 日志

```bash
tail -f .run/core.launchd.log
tail -f .run/gateway.launchd.log
```

## 说明

1. `launchd` 没有 `systemd EnvironmentFile` 那种机制，所以这里用了包装脚本：
   - `scripts/launchd-core.sh`
   - `scripts/launchd-gateway.sh`
2. 它们会先加载 env，再启动真正的进程。
3. macOS 上重载语义是 `kickstart -k`，也就是受控重启，不是进程内热更新。
