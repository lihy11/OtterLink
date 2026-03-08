# 运维说明

## 本地启动

```bash
source .run/feishu.env
./scripts/start-longconn.sh
```

服务地址：

- gateway: `http://127.0.0.1:3000`
- core: `http://127.0.0.1:3001`

## 停止

```bash
./scripts/stop-longconn.sh
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

## Runtime 控制自检

飞书里可直接发送：

```text
/runtime show
/runtime list
/runtime load
/runtime load /absolute/workspace
```

如果导入后仍只看到默认 runtime，先检查：

1. 目标 workspace 在 `CLAUDE_HOME_DIR/projects/` 下是否有对应目录。
2. 该目录是否包含 `sessions-index.json` 或 `*.jsonl`。
3. workspace 是否存在 `/tmp` 与 `/private/tmp` 这样的路径别名问题。

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

## 发布前检查

1. `cargo test`
2. `cd gateway && npm test`
3. 使用真实 `.run/feishu.env` 执行一次 live token smoke
4. 确认 `.run/` 和密钥未进入提交内容
