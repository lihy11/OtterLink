# API 示例

## Gateway -> Core

```bash
curl -X POST http://127.0.0.1:7211/internal/core/turn \
  -H 'content-type: application/json' \
  -H "x-core-ingest-token: $CORE_INGEST_TOKEN" \
  -d '{
    "turn_id": "turn_demo_1",
    "session_key": "feishu:p2p:ou_xxx",
    "text": "请总结当前仓库"
  }'
```

```bash
curl -X POST http://127.0.0.1:7211/internal/core/control \
  -H 'content-type: application/json' \
  -H \"x-core-ingest-token: $CORE_INGEST_TOKEN\" \
  -d '{
    \"session_key\": \"feishu:p2p:ou_xxx\",
    \"action\": \"load_runtimes\",
    \"workspace_path\": \"/Users/haiyangli/Desktop/InterestingPorjects/remoteagent/workspace\"
  }'
```

```bash
curl -X POST http://127.0.0.1:7211/internal/core/control \
  -H 'content-type: application/json' \
  -H \"x-core-ingest-token: $CORE_INGEST_TOKEN\" \
  -d '{
    \"session_key\": \"feishu:p2p:ou_xxx\",
    \"action\": \"create_runtime\",
    \"label\": \"claude-alt\",
    \"agent_kind\": \"claude_code\",
    \"workspace_path\": \"/Users/haiyangli/Desktop/InterestingPorjects/remoteagent\"
  }'
```

## Core -> Gateway Event

```bash
curl -X POST http://127.0.0.1:1127/internal/gateway/event \
  -H 'content-type: application/json' \
  -H "x-gateway-event-token: $GATEWAY_EVENT_TOKEN" \
  -d '{
    "turn_id": "turn_demo_1",
    "slot": "progress",
    "message": {
      "kind": "card",
      "card": {
        "title": "Turn Progress",
        "theme": "blue",
        "wide_screen_mode": true,
        "update_multi": true,
        "blocks": [{"kind": "markdown", "text": "处理中"}]
      }
    }
  }'
```

说明：

- `progress` 示例里的 `card` 只是 core 到 gateway 的标准事件形态
- 新逻辑里 core 更推荐直接发送 `kind=text` 的 `progress` 中间消息；这些消息应当已经在 core 内部完成 chunk 聚合，而不是 token 级碎片
- turn 被接受时，gateway 还会额外对用户原消息调用一次消息表情回复接口，不再发送单独的“开始运行”文本

## 注入飞书事件到 Gateway

```bash
curl -X POST http://127.0.0.1:1127/internal/feishu/event \
  -H 'content-type: application/json' \
  -H "x-bridge-token: $BRIDGE_INGEST_TOKEN" \
  -d '{
    "sender": {"sender_id": {"open_id": "ou_demo"}},
    "message": {
      "message_id": "om_demo",
      "chat_id": "oc_demo",
      "chat_type": "p2p",
      "content": "{\"text\":\"配对 your-token\"}"
    }
  }'
```

## 主动通知

```bash
curl -X POST http://127.0.0.1:1127/internal/notify \
  -H 'content-type: application/json' \
  -H "x-notify-token: $BRIDGE_NOTIFY_TOKEN" \
  -d '{"text": "hello from gateway notify", "open_id": "ou_xxx"}'
```

## 脚本

```bash
./scripts/start-longconn.sh
./scripts/stop-longconn.sh
./scripts/send-notify.sh 'hello'
./scripts/test-local-acp.sh
```

## 飞书中的控制命令

```text
/runtime help
/runtime show
/runtime list
/runtime load
/runtime load /Users/haiyangli/Desktop/InterestingPorjects/remoteagent
/runtime use codex
/runtime pick c06c9a5e
/runtime new claude-alt
/runtime cwd ~/Desktop/InterestingPorjects/remoteagent/workspace
/runtime stop
/runtime proxy default
/runtime proxy on http://127.0.0.1:7890
/runtime proxy off
会话 帮助
```

说明：

1. `/runtime load` 会优先调用 ACP `session/list` 按当前 `agent + cwd` 列出会话。
2. 只有 agent 不支持 `session/list` 时，`claude_code` 才回退到 `CLAUDE_HOME_DIR/projects/...`，`codex` 才回退到 `CODEX_HOME_DIR/state_5.sqlite`。
3. `list/show/load` 的飞书返回会渲染为 Markdown 表格，表头摘要只显示当前 `Agent` 和 `CWD`。
4. `/runtime pick` 如果命中已有 session，除了控制结果表格外，还会额外回一张 `历史概览` 卡片，展示裁剪后的最近 5 轮 `user / assistant` 对话。
5. `/runtime stop` 会停止当前正在运行的 turn；ACP 会发 `session/cancel`，`exec_json` 会终止本地进程。
6. `/runtime proxy` 会控制后续 ACP/exec 启动时是否注入代理环境变量。
