# 飞书机器人 + ACP（Codex/Claude）会话编排规划文档

## 1. 目标与范围

### 1.1 目标
在服务器侧使用 Rust + ACP SDK 托管 Codex/Claude 会话，实现：
1. 飞书用户发消息 -> 服务端接收。
2. 服务端按会话映射规则路由到 ACP session。
3. 将消息转发给底层 Agent（Codex/Claude）。
4. 持续接收 Agent 中间输出（文本增量、工具调用、进度）。
5. 按渲染规则回写飞书（文本内容 + 进度更新）。

### 1.2 你确认的核心会话映射规则
1. `p2p`（私聊） -> 1 个 session。
2. `group`（群聊） -> 1 个 session。
3. 群聊中的 `thread`（话题） -> 从该群 session fork 一个子 session。

## 2. 关键概念与判定

### 2.1 入站消息类型判定（来自飞书接收消息事件）
1. `chat_type == p2p`：私聊。
2. `chat_type == group` 且无 `thread_id`：普通群消息。
3. 有 `thread_id`：话题消息。

### 2.2 会话作用域（Scope Key）
建议统一生成 `scope_key`，作为路由、并发锁、状态存储主键：
1. 私聊：`tenant:{tenant_key}:p2p:{chat_id}`
2. 群聊：`tenant:{tenant_key}:group:{chat_id}`
3. 话题：`tenant:{tenant_key}:group:{chat_id}:thread:{thread_id}`

## 3. 总体架构

## 3.1 组件划分
1. `feishu_webhook_ingress`：接收事件、验签、解密、去重。
2. `message_router`：按 `chat_type/thread_id` 计算 `scope_key`。
3. `session_registry`：管理 scope -> ACP session 映射。
4. `fork_manager`：基于群 session 建立话题子 session。
5. `acp_runtime`：封装 Codex/Claude 的 ACP 连接与 prompt 调用。
6. `update_normalizer`：将 ACP `session/update` 统一成内部事件。
7. `render_scheduler`：聚合、节流、幂等回写飞书。
8. `feishu_egress`：调用发送/更新消息接口。
9. `persistence`：存储会话、消息映射、turn、工具状态、审计日志。

### 3.2 数据流（高层）
1. 飞书回调 -> ingress。
2. ingress -> router（生成 `scope_key`）。
3. router -> registry（获取或创建 session）。
4. registry -> acp_runtime（发送 prompt）。
5. acp_runtime 更新流 -> normalizer。
6. normalizer -> scheduler。
7. scheduler -> feishu_egress（消息发送/更新）。

## 4. 会话生命周期设计

### 4.1 Session 状态机
`Idle -> Running -> Completed -> Idle`
异常支路：`Running -> Failed -> Idle`

### 4.2 单聊/群聊 session
1. 首条消息到达，创建 session。
2. 后续消息复用 session。
3. 闲置超时（如 24h）后可归档，下一条消息自动恢复或新建。

### 4.3 话题 fork session

#### 4.3.1 触发条件
满足：`chat_type == group` 且 `thread_id != null`。

#### 4.3.2 fork 语义
从群 session 派生话题子 session，子 session 继承父 session 的“可见上下文快照”，但后续对话隔离。

#### 4.3.3 推荐实现（应用层 fork）
若底层 ACP runtime 没有稳定“原生 fork API”，采用应用层 fork：
1. 读取父 session 最近 N 轮摘要（不是全量历史）。
2. 构造 `fork_bootstrap_prompt` 注入到新话题 session。
3. 建立父子关系：`parent_session_id`。
4. 后续 thread 仅写入子 session。

#### 4.3.4 fork 一致性
同一 `thread_id` 仅允许创建一次子 session：
1. 对 `scope_key(thread)` 做分布式锁。
2. 创建前二次检查（double-check）。

## 5. 路由与并发控制

### 5.1 路由算法（伪代码）
```text
on_message(event):
  if duplicated(event.event_id): return

  scope = classify(event.chat_type, event.thread_id, event.chat_id, event.tenant_key)

  lock(scope)
  session = registry.get(scope)
  if session == null:
    if scope is thread:
      parent = registry.get(group_scope(chat_id)) or registry.create(group_scope)
      session = fork_manager.fork_from_parent(parent, thread_scope)
    else:
      session = registry.create(scope)
  unlock(scope)

  enqueue_turn(session, event)
```

### 5.2 并发策略
1. 同一 `scope_key` 串行执行（防上下文穿插）。
2. 不同 `scope_key` 并行执行。
3. 话题与群根会话并行，互不阻塞。

## 6. 数据模型（建议）

### 6.1 `sessions`
1. `id`（uuid）
2. `scope_key`（unique）
3. `scope_type`（p2p/group/thread）
4. `tenant_key`
5. `chat_id`
6. `thread_id`（nullable）
7. `parent_session_id`（nullable）
8. `runtime`（codex/claude）
9. `runtime_session_ref`（ACP 侧 ID）
10. `status`（idle/running/failed/archived）
11. `context_summary`（text/json）
12. `created_at/updated_at`

### 6.2 `turns`
1. `id`
2. `session_id`
3. `feishu_message_id`（入站消息 ID）
4. `user_input`
5. `assistant_output_final`
6. `status`
7. `started_at/completed_at`

### 6.3 `tool_calls`
1. `id`
2. `turn_id`
3. `tool_call_id`（ACP）
4. `title/kind/status`
5. `content_snapshot`
6. `started_at/completed_at`

### 6.4 `feishu_render_bindings`
1. `id`
2. `turn_id`
3. `target_chat_id`
4. `target_thread_id`（nullable）
5. `output_message_id`（机器人回写消息 ID）
6. `last_render_hash`
7. `last_render_at`

### 6.5 `event_dedup`
1. `event_id`（unique）
2. `source`
3. `expire_at`

## 7. ACP 集成策略

### 7.1 Runtime 抽象
定义统一 trait：
1. `start_session(scope_meta) -> RuntimeSessionRef`
2. `fork_session(parent_ref, fork_payload) -> RuntimeSessionRef`（可选）
3. `send_prompt(session_ref, prompt)`
4. `subscribe_updates(session_ref) -> Stream<AgentUpdate>`
5. `cancel_turn(session_ref, turn_id)`

### 7.2 更新事件标准化
将 ACP `session/update` 统一成内部事件：
1. `TextDelta`
2. `PlanUpdated`
3. `ToolCallStarted`
4. `ToolCallUpdated`
5. `TurnCompleted`
6. `TurnFailed`

## 8. 飞书渲染与节流策略

### 8.1 输出结构
1. 主回答区：模型文本（增量）。
2. 进度区：当前阶段、预计耗时、状态。
3. 工具区：工具调用列表（运行中/完成/失败）。

### 8.2 渲染频率控制
1. 文本增量：300-800ms 合并后更新一次。
2. 工具状态：状态变化立即更新。
3. 最终完成：强制刷新一次完整结果。

### 8.3 幂等渲染
1. 对渲染结果做 hash。
2. hash 未变化则跳过 API 调用。
3. API 失败按指数退避重试。

## 9. 错误处理与恢复

### 9.1 常见失败
1. ACP 进程异常退出。
2. 飞书回写限流/网络失败。
3. 同 scope 并发消息导致乱序。
4. fork 重复创建。

### 9.2 恢复策略
1. ACP 崩溃自动重连，新 turn 可继续，当前 turn 标记失败并提示重试。
2. 回写失败进入重试队列，超过阈值降级为简版文本。
3. scope 串行队列保证顺序。
4. `scope_key + lock + unique index` 保证 fork 幂等。

## 10. 安全与合规

1. 飞书回调：签名校验 + 时间戳校验 + 可选解密。
2. Token 最小权限原则。
3. 关键日志脱敏（消息正文、token、用户标识）。
4. 审计日志保留（事件接收、会话路由、模型输出摘要、回写结果）。

## 11. 观测指标（必须）

1. 入站 QPS、去重命中率。
2. session 创建数、fork 数、活跃会话数。
3. 首 token 延迟、整轮完成时延。
4. 飞书回写成功率、重试率、限流率。
5. ACP 运行错误率、工具调用失败率。

## 12. 里程碑

### M1（最小闭环）
1. 私聊 + 群聊单 session 路由。
2. 单轮问答与文本增量回写。
3. 基础去重、串行执行、错误重试。

### M2（话题 fork）
1. thread 判定与子 session 创建。
2. 父会话摘要注入子会话。
3. 话题独立上下文与独立输出。

### M3（生产化）
1. 工具调用进度可视化。
2. 可观测性、告警、灰度发布。
3. Codex/Claude runtime 热切换。

## 13. 开发实现清单（可直接开工）

1. 建库表：`sessions/turns/tool_calls/feishu_render_bindings/event_dedup`。
2. 完成 `scope_key` 路由器与单元测试。
3. 完成 `session_registry`（含锁、幂等创建）。
4. 完成 `fork_manager`（父摘要生成 + 子 session 初始化）。
5. 接 ACP runtime，打通 `send_prompt + updates`。
6. 接飞书回写通道，落地 scheduler 节流。
7. 打通端到端压测与故障演练。

## 14. 关键决策记录（本次确认）

1. 私聊维度：1 chat 1 session。
2. 群聊维度：1 chat 1 session。
3. 群话题维度：从群 session fork 话题子 session。
4. 群与话题上下文隔离，父子只在 fork 时单向继承。

