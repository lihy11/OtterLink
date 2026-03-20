# 安装部署

## 目标

推荐在 Linux 上以两个长期运行的进程部署：

1. Rust `core`
2. Node.js `gateway`

其中 gateway 当前只负责飞书接入、认证和消息转发；`/ot` 命令与普通消息统一由 core 处理。

推荐用 `systemd` 托管，原因：

- 进程异常退出后自动拉起
- 可用 `journalctl` 集中看日志
- 支持 `systemctl reload` 触发受控重启

运行后可通过飞书 `/ot stop` 停止当前 turn；ACP runtime 会走协议取消，不依赖 shell 注入 `Ctrl+C`。

## 依赖

推荐直接使用一键安装脚本，它会在缺失时自动安装项目当前固定版本：

- Rust `1.94.0`
- Node.js `22.22.1`

如果你选择手工准备环境，版本应与一键安装脚本保持一致，并至少提供：

```bash
cargo
node
npm
git
curl
tar
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
3. 安装 `otterlink` 控制台命令到 `~/.local/bin/otterlink`
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
sudo mkdir -p /etc/otterlink /var/lib/otterlink
sudo cp deploy/systemd/otterlink.env.example /etc/otterlink/otterlink.env
sudo chown -R "$USER":"$(id -gn)" /etc/otterlink /var/lib/otterlink
```

然后按实际环境修改 `/etc/otterlink/otterlink.env`。

若先走本地 CLI 配置，也可以直接把 `.run/feishu.env` 的内容整理后迁移到 `/etc/otterlink/otterlink.env`。

如果你需要在飞书里对 `codex` 执行 `/ot load`，记得把 `CODEX_HOME_DIR` 指向线上机器真实的 `~/.codex`。

## 安装 systemd 单元

```bash
sudo SERVICE_USER="$USER" \
  SERVICE_GROUP="$(id -gn)" \
  ENV_FILE=/etc/otterlink/otterlink.env \
  ./scripts/install-systemd.sh
```

## 启动

```bash
sudo systemctl start otterlink-core
sudo systemctl start otterlink-gateway
```

或：

```bash
sudo systemctl start otterlink.target
```

## 重载

当前重载语义是“受控退出 + systemd 自动拉起”：

```bash
sudo ./scripts/reload-systemd.sh
```

等价于：

```bash
sudo systemctl reload otterlink-core
sudo systemctl reload otterlink-gateway
```

适合以下场景：

- env 文件变更
- gateway 渲染逻辑变更后重新部署
- Rust binary 升级后无状态切换

## 日志

```bash
sudo journalctl -u otterlink-core -f
sudo journalctl -u otterlink-gateway -f
```

## 升级

```bash
git pull
cargo build --release
cd gateway && npm ci
sudo ./scripts/reload-systemd.sh
```

若使用源码目录下的本地 CLI 包装器，升级后不需要重新安装命令；`~/.local/bin/otterlink` 会继续指向当前仓库目录。
