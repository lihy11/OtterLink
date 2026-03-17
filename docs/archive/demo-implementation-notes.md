# Demo 实现说明

## 1. 当前实现结构

项目已经按三层拆开：

1. `src/agent`：把 ACP update / Codex JSONL 统一成 `NormalizedAgentEvent`，并通过 `AgentRuntime` 输出统一事件流
   - `src/agent/runtime/types.rs`
   - `src/agent/runtime/adapters/codex.rs`
   - `src/agent/runtime/adapters/claude_code.rs`
2. `src/core`：负责 session、turn、持久化、配对、标准消息 `OutboundMessage`
   - `src/core/ports.rs` 提供 `MessagingGateway` 抽象
3. `src/gateway`：负责 HTTP ingress 和飞书渲染/发送

## 2. 处理流程

1. `gateway/feishu/longconn/ws-bridge.js` 使用官方 Node SDK 建立长连接。
2. 接收 `im.message.receive_v1` 后转发到本地 `POST /internal/feishu/event`。
3. `src/gateway/http.rs` 负责 HTTP 接入、鉴权和请求转换。
4. `src/core/service.rs` 做去重、session resolve、scope 串行化和 turn 编排。
5. `src/agent/normalized.rs` 统一 ACP / CLI 事件结构。
6. `src/gateway/feishu/render.rs` 将标准消息映射成飞书卡片/文本。
7. `src/gateway/feishu/client.rs` 调用飞书 API 完成 reply/update/pin/send。

## 3. 代码位置

1. 应用装配：`src/app.rs`
2. 配置：`src/config.rs`
3. agent 标准化：`src/agent/normalized.rs`
4. 核心编排：`src/core/service.rs`
5. session 注册表：`src/core/registry.rs`
6. sqlite 持久化：`src/core/persistence.rs`
7. 飞书 HTTP / 出站：`src/gateway/http.rs`、`src/gateway/feishu/`
8. 飞书长连接桥：`gateway/feishu/longconn/ws-bridge.js`

## 4. 兼容性说明

1. 原有接口保持不变：`/internal/feishu/event`、`/internal/test/send`、`/internal/notify`
2. 原有运行模式保持不变：`exec_json` / `acp` / `acp_fallback`
3. 旧的 `codex_thread_id` 语义改成更通用的 `runtime_session_ref`
4. 飞书通知支持标准消息和原始 `content` 透传

## 5. 后续可扩展点

1. 为第二个 Agent runtime 新增 normalizer，而不改 core 逻辑
2. 为第二个 IM 平台新增 gateway，而不改 core 编排
3. 将 `CoreService` 到 gateway 的调用进一步抽象成 trait

## 6. 飞书入站认证

新增了用户级入站认证，防止非授权飞书用户直接驱动 agent：

1. `FEISHU_AUTH_MODE=pair`：只允许已配对用户。
2. `FEISHU_AUTH_MODE=allow_from`：只允许 `ALLOW_FROM_OPEN_IDS` 白名单。
3. `FEISHU_AUTH_MODE=pair_or_allow_from`：白名单或已配对均可。
4. `PAIR_AUTH_TOKEN`：配对口令，用户需发送 `配对 <口令>`。

当前鉴权发生在 `src/core/service.rs` 的 session resolve 之前，因此未授权消息不会进入 runtime。
