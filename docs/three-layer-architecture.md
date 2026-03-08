# 三层重构说明

## 目标

当前项目已按三层职责重组：

1. `agent` 层：统一接入不同 Agent runtime，并把原始输出标准化。
2. `core` 层：负责 session、turn、配对、路由、消息编排等核心逻辑。
3. `gateway` 层：负责接收/发送外部平台消息，并把 core 的标准消息渲染成平台友好格式。

## 目录

```text
src/
  agent/
    normalized.rs         # ACP / Codex JSONL -> 统一事件
    runtime/
      acp.rs              # ACP runtime 进程与 client
      exec_json.rs        # Codex CLI runtime
      adapters/
        codex.rs
        claude_code.rs
  core/
    models.rs             # 标准消息模型、入站请求模型
    pairing.rs            # tenant_key -> open_id
    persistence.rs        # sqlite 持久化
    ports.rs              # Core 对外部网关的抽象接口
    registry.rs           # scope -> session
    service.rs            # turn 编排与核心流程
    support.rs            # 通用工具函数
  gateway/
    http.rs               # HTTP ingress
    feishu/
      client.rs           # 飞书 API 发送/更新/置顶
      render.rs           # 标准消息 -> 飞书 text/post/card
  app.rs                  # 应用装配
  config.rs               # 配置加载
  main.rs                 # 薄入口

gateway/
  feishu/
    longconn/
      ws-bridge.js        # 飞书长连接桥接
```

## 层间边界

### 1. agent 层

输入：core 给出的 prompt、runtime 配置、已有 session 引用。  
输出：`NormalizedAgentEvent`，例如：

- `RuntimeSessionReady`
- `AssistantChunk`
- `AssistantMessage`
- `ToolState`
- `PlanUpdated`
- `Usage`

这样不同 agent 的原始事件差异，优先在这一层收敛。`ACP_ADAPTER` 的差异也通过 `runtime/adapters` 插件目录收敛。

### 2. core 层

负责：

- 根据 `p2p/group/thread` 解析 scope
- 在进入 runtime 前做入站用户认证（pair / allow_from）
- 创建和复用 session
- thread 从 group fork
- turn 串行化
- 持久化 session / turn bindings / dedup
- 把 agent 统一事件聚合成标准消息 `OutboundMessage`

core 不直接拼飞书卡片 JSON，只生产标准消息。core 通过 `MessagingGateway` trait 调用外部平台。

### 3. gateway 层

负责：

- 接收 Feishu long-connection 转发事件
- 认证飞书 API
- 将 `OutboundMessage` 渲染成飞书 `text/post/interactive`
- 利用飞书特性提升阅读体验，例如进度卡片、Todo 卡片、最终结果卡片、置顶

后续新增企业微信、Slack、Telegram 时，应只新增 gateway 适配器，而不是修改 core 编排。

## 当前状态

这次重构的重点是“职责拆分”和“接口方向收敛”，行为尽量保持不变：

1. 现有 HTTP 接口保持不变。
2. 现有 ACP / `codex exec --json` 兼容模式保持不变。
3. 飞书长连接仍由 Node SDK 处理，但目录归到 `gateway/feishu/longconn/`。
4. SQLite 表结构保留，字段 `codex_thread_id` 改为更通用的 `runtime_session_ref`。

## 后续建议

1. 把 `MessagingGateway` 继续细分成 ingress / egress 两类 trait。
2. 继续给 `AgentRuntime` 增加更细的取消、超时、中断控制。
3. 为 `NormalizedAgentEvent` 和 `OutboundMessage` 加单元测试。
4. 当接入第二个 IM 平台时，再把 `gateway` 抽成独立进程或 JS 服务。
