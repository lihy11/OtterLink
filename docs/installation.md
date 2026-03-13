# 安装部署

## 目标

推荐在 Linux 上以两个长期运行的进程部署：

1. Rust `core`
2. Node.js `gateway`

推荐用 `systemd` 托管，原因：

- 进程异常退出后自动拉起
- 可用 `journalctl` 集中看日志
- 支持 `systemctl reload` 触发受控重启

运行后可通过飞书 `/runtime stop` 停止当前 turn；ACP runtime 会走协议取消，不依赖 shell 注入 `Ctrl+C`。

## 依赖

```bash
rustup toolchain install stable
curl -fsSL https://deb.nodesource.com/setup_20.x | bash -
apt-get install -y nodejs build-essential pkg-config libssl-dev
```

需要提前准备：

- 飞书应用 `APP_ID` / `APP_SECRET`
- 本机可执行的 `claude` / `codex` / ACP agent
- 真实 `~/.claude`，如果要导入 Claude 历史 session

## 一键安装

源码克隆完成后，推荐直接执行：

```bash
./scripts/install-one-click.sh
```

它会：

1. 编译 Rust release binary
2. 安装 gateway 的 npm 依赖
3. 安装 `remoteagent` 控制台命令到 `~/.local/bin/remoteagent`
4. 扫描并预装缺失的 `claude_code` / `codex` ACP runtime
5. 如果当前机器缺少 Rust 或 Node，则自动安装 Rust `1.94.0` 与 Node `22.22.1`
6. 在交互式终端中可继续进入 `otterlink configure` 和 `otterlink start`

说明：

- 一键安装负责依赖和构建
- `otterlink start` / `scripts/start-longconn.sh` 只负责启动，不再在运行时补装依赖

## 构建

```bash
cargo build --release
cd gateway && npm ci
```

## 环境文件

复制：

```bash
sudo mkdir -p /etc/remoteagent /var/lib/remoteagent
sudo cp deploy/systemd/remoteagent.env.example /etc/remoteagent/remoteagent.env
sudo chown -R "$USER":"$(id -gn)" /etc/remoteagent /var/lib/remoteagent
```

然后按实际环境修改 `/etc/remoteagent/remoteagent.env`。

若先走本地 CLI 配置，也可以直接把 `.run/feishu.env` 的内容整理后迁移到 `/etc/remoteagent/remoteagent.env`。

如果你需要在飞书里对 `codex` 执行 `/runtime load`，记得把 `CODEX_HOME_DIR` 指向线上机器真实的 `~/.codex`。

## 安装 systemd 单元

```bash
sudo SERVICE_USER="$USER" \
  SERVICE_GROUP="$(id -gn)" \
  ENV_FILE=/etc/remoteagent/remoteagent.env \
  ./scripts/install-systemd.sh
```

## 启动

```bash
sudo systemctl start remoteagent-core
sudo systemctl start remoteagent-gateway
```

或：

```bash
sudo systemctl start remoteagent.target
```

## 重载

当前重载语义是“受控退出 + systemd 自动拉起”：

```bash
sudo ./scripts/reload-systemd.sh
```

等价于：

```bash
sudo systemctl reload remoteagent-core
sudo systemctl reload remoteagent-gateway
```

适合以下场景：

- env 文件变更
- gateway 渲染逻辑变更后重新部署
- Rust binary 升级后无状态切换

## 日志

```bash
sudo journalctl -u remoteagent-core -f
sudo journalctl -u remoteagent-gateway -f
```

## 升级

```bash
git pull
cargo build --release
cd gateway && npm ci
sudo ./scripts/reload-systemd.sh
```

若使用源码目录下的本地 CLI 包装器，升级后不需要重新安装命令；`~/.local/bin/remoteagent` 会继续指向当前仓库目录。
