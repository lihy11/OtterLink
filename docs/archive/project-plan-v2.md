# 项目规划文档（详细版）

## 1. 项目目标

构建一个“飞书机器人 <-> ACP Agent（Codex/Claude）”桥接服务，支持：
1. 接收飞书消息事件。
2. 根据 `p2p/group/thread` 路由到对应会话。
3. 话题消息从群会话 fork 子会话。
4. 把用户输入送入底层 Agent。
5. 持续把中间输出（文本增量、进度、工具状态）回写飞书。

## 2. 官方约束与设计依据

### 2.1 认证策略（必须）
1. 自建应用通过 `app_id + app_secret` 调用：
`POST https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal`
2. 返回 `tenant_access_token` 与 `expire`（秒）。
3. token 最大有效期 2 小时，剩余有效期小于 30 分钟时可刷新。

### 2.2 消息接收与分类
1. 事件：`im.message.receive_v1`。
2. `chat_type` 可区分 `p2p` / `group`。
3. `thread_id` 存在表示话题消息；缺失表示非话题。
4. 官方建议幂等用 `message_id`，不要依赖 `event_id`。

### 2.3 回复消息
1. 接口：`POST /open-apis/im/v1/messages/:message_id/reply`
2. 认证：`Authorization: Bearer tenant_access_token`
3. 发送文本时：`msg_type=text`，`content` 为 JSON 字符串。

## 3. 会话映射规则（已确定）

1. `p2p` -> 每个私聊 1 个 session。
2. `group` 且无 `thread_id` -> 每个群 1 个 session。
3. `group` 且有 `thread_id` -> 话题 session（由对应群 session fork）。

## 4. Session 模型与 fork 语义

### 4.1 作用域键
1. 私聊：`tenant:{tenant_key}:p2p:{chat_id}`
2. 群：`tenant:{tenant_key}:group:{chat_id}`
3. 话题：`tenant:{tenant_key}:group:{chat_id}:thread:{thread_id}`

### 4.2 fork 行为
1. 收到 thread 消息时先确保群 session 存在。
2. 若 thread session 不存在，则创建并记录 `parent_session_id=group_session_id`。
3. thread session 后续仅处理该 thread 消息，不写回父 session 上下文。

## 5. 服务模块设计

1. `webhook_ingress`：接收回调、URL 验证、token 校验、去重。
2. `router`：根据 `chat_type/thread_id` 计算 scope。
3. `session_registry`：scope 到 session 的查询/创建。
4. `fork_manager`：处理 thread session 派生。
5. `agent_runtime`：封装 ACP client（Codex/Claude）。
6. `update_normalizer`：ACP 更新转内部标准事件。
7. `render_scheduler`：节流和合并回写。
8. `feishu_client`：认证 + reply/send/update API。

## 6. 交互时序

1. 飞书推送消息事件到 webhook。
2. 服务端解析 `message_id/chat_type/thread_id`。
3. 通过 scope 获取 session（必要时 fork）。
4. 立刻回一条“已接收/处理中”。
5. 将 prompt 发给 ACP。
6. 订阅 ACP update：
   - 文本 chunk -> 回写文本区
   - tool_call -> 回写进度区
   - completed -> 回写最终答案

## 7. Demo 范围定义（当前状态）

当前 Demo 已实现：
1. `app_id/app_secret` 自动获取并缓存 `tenant_access_token`。
2. 接收 `im.message.receive_v1`。
3. 实现 `p2p/group/thread` 路由与 thread fork。
4. 通过 `reply` 接口回写 Codex 中间进度和最终文本。
5. 真实执行 `codex exec --json` / `codex exec resume --json`。

暂未实现（下一阶段）：
1. 严格 ACP transport 适配（当前为 Codex CLI JSONL 事件流适配）。
2. 持久化（当前内存态）。
3. 分布式锁与多副本一致性。

## 8. 里程碑

### M1（已覆盖 Demo）
1. 认证可用。
2. webhook 可收消息。
3. 路由和 fork 逻辑可观察。
4. 文本回写链路可用。

### M2（接 ACP）
1. 接入 ACP SDK。
2. 将 `session/update` 映射为飞书渲染事件。
3. 增加 turn 状态与失败恢复。

### M3（生产化）
1. 数据库持久化。
2. 指标与告警。
3. 灰度与压测。

## 9. 关键风险

1. webhook 重复投递导致重复执行。
2. 同 scope 并发消息上下文交叉。
3. 飞书回写限流导致中间输出丢失。
4. token 刷新策略不正确导致 401。

## 10. 风险缓解

1. 使用 `message_id` 去重。
2. scope 串行队列。
3. 渲染节流 + 重试队列。
4. token 缓存并在剩余有效期小于 30 分钟刷新。

## 11. 官方文档链接

1. 自建应用获取 tenant_access_token：
https://open.feishu.cn/document/server-docs/authentication-management/access-token/tenant_access_token_internal
2. 接收消息事件：
https://open.feishu.cn/document/server-docs/im-v1/message/events/receive
3. 回复消息：
https://open.feishu.cn/document/server-docs/im-v1/message/reply
4. 话题概述：
https://open.feishu.cn/document/server-docs/im-v1/message/thread-introduction
