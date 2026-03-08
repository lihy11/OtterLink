# API 示例

## Gateway -> Core

```bash
curl -X POST http://127.0.0.1:3001/internal/core/turn \
  -H 'content-type: application/json' \
  -H "x-core-ingest-token: $CORE_INGEST_TOKEN" \
  -d '{
    "turn_id": "turn_demo_1",
    "session_key": "feishu:p2p:ou_xxx",
    "text": "请总结当前仓库"
  }'
```

```bash
curl -X POST http://127.0.0.1:3001/internal/core/control \
  -H 'content-type: application/json' \
  -H \"x-core-ingest-token: $CORE_INGEST_TOKEN\" \
  -d '{
    \"session_key\": \"feishu:p2p:ou_xxx\",
    \"action\": \"load_runtimes\",
    \"workspace_path\": \"/Users/haiyangli/Desktop/InterestingPorjects/remoteagent/workspace\"
  }'
```

```bash
curl -X POST http://127.0.0.1:3001/internal/core/control \
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
curl -X POST http://127.0.0.1:3000/internal/gateway/event \
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

## 注入飞书事件到 Gateway

```bash
curl -X POST http://127.0.0.1:3000/internal/feishu/event \
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
curl -X POST http://127.0.0.1:3000/internal/notify \
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
/runtime show
/runtime list
/runtime load
/runtime load /Users/haiyangli/Desktop/InterestingPorjects/remoteagent
/runtime new claude-alt
/runtime use claude-alt
/runtime use c06c9a5e
/workspace set /Users/haiyangli/Desktop/InterestingPorjects/remoteagent/workspace
```
