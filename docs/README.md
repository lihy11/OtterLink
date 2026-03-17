# 文档索引

以下文档描述当前实现，不是历史草案。

## 当前命名

- 中文名：`水獭`
- 英文名：`OtterLink`
- CLI：`otterlink`
- 命名含义：强调“常驻、本地、轻巧、中继连接”，对应本项目把飞书消息与本地 agent runtime 连接起来的定位

## 主文档

1. `architecture.md`
   系统边界、运行时拆分、主数据流。
2. `design.md`
   关键设计决策和消息生命周期。
3. `interfaces.md`
   Rust core API、JS gateway API、标准协议，以及 runtime 导入来源。
4. `configuration.md`
   运行配置、认证配置、runtime 配置。
5. `data-model.md`
   SQLite 和 gateway 内存状态模型。
6. `operations.md`
   启动、停止、测试、排障。
7. `installation.md`
   Linux 安装、构建、`systemd` 部署和重载。
8. `macos-installation.md`
   macOS `launchd` 安装、启动和重载。
9. `api-examples.md`
   典型请求、响应和脚本示例。
10. `acp.md`
   ACP 功能与官方文档的逐项映射。

最新运行控制补充包括：`/ot stop`、ACP 协议取消、ACP `session/list`、代理注入策略，以及本地 `otterlink` 控制台工具与一键安装脚本。

## 归档资料

历史方案、计划和实现笔记已移动到 `archive/`，仅作为背景参考，不应覆盖主文档：

- `archive/three-layer-architecture.md`
- `archive/demo-implementation-notes.md`
- `archive/project-plan-v2.md`
- `archive/feishu-acp-session-architecture-plan.md`
- `archive/task-list.md`

## 阅读顺序

1. `architecture.md`
2. `design.md`
3. `interfaces.md`
4. `configuration.md`
5. `data-model.md`
6. `operations.md`
7. `installation.md`
8. `macos-installation.md`
9. `api-examples.md`
10. `acp.md`

## 部署资源

- `../deploy/systemd/otterlink-core.service`
- `../deploy/systemd/otterlink-gateway.service`
- `../deploy/systemd/otterlink.target`
- `../deploy/systemd/otterlink.env.example`
- `../deploy/launchd/com.otterlink.core.plist`
- `../deploy/launchd/com.otterlink.gateway.plist`
- `../scripts/install-one-click.sh`
- `../scripts/otterlink-cli.js`
