# ACP Reference Map

本项目当前实际使用的 ACP 功能，必须与官方文档对齐。下面这张表按“代码位置 -> ACP 能力 -> 官方文档”整理，后续如果 ACP 行为变化，先更新这里，再改代码。

| 能力 | 当前代码 | 说明 | 官方文档 |
| --- | --- | --- | --- |
| `initialize` | `src/agent/runtime/acp.rs` | worker 建立长连接后只初始化一次，不再每轮重复初始化 | https://agentclientprotocol.com/protocol/initialization |
| `session/new` | `src/agent/runtime/acp.rs` | 当前 runtime 没有 `runtime_session_ref` 时显式新建 session | https://agentclientprotocol.com/protocol/session-setup |
| `session/load` | `src/agent/runtime/acp.rs` | 已选历史 session 时显式恢复，不再静默回退新建 | https://agentclientprotocol.com/protocol/session-setup#loading-sessions |
| `session/list` | `src/agent/runtime/acp.rs`, `src/core/service.rs` | `/runtime load` 优先走 ACP 列出当前 `agent + cwd` 的候选会话；agent 不支持时才回退本地发现 | https://agentclientprotocol.com/protocol/session-list |
| `session/prompt` | `src/agent/runtime/acp.rs` | 单轮执行入口，turn 完成以 `PromptResponse.stop_reason` 为准 | https://agentclientprotocol.com/protocol/prompt-turn |
| `PromptResponse.stop_reason` | `src/agent/runtime/acp.rs`, `src/core/service.rs` | 正常完成记录为 `end_turn`，取消为 `cancelled`，其它结束原因单独记录 | https://agentclientprotocol.com/protocol/prompt-turn#stop-reasons |
| `session/cancel` | `src/agent/runtime/acp.rs`, `src/core/service.rs` | `/runtime stop` 先发协议取消，再等待 prompt 以 `cancelled` 收尾 | https://agentclientprotocol.com/protocol/prompt-turn#cancellation |
| `session/update` | `src/agent/runtime/acp.rs`, `src/agent/normalized.rs` | 接收增量文本、工具状态、计划更新，并转成统一事件 | https://agentclientprotocol.com/protocol/prompt-turn |
| `session/request_permission` | `src/agent/runtime/acp.rs` | 默认优先 `AllowAlways/AllowOnce`；收到 stop 后统一返回 `Cancelled` | https://agentclientprotocol.com/protocol/tool-calls#requesting-permission |
| `session/set_mode` | `src/agent/runtime/acp.rs` | 只在 agent 明确声明该 mode 可用时调用 | https://agentclientprotocol.com/protocol/session-modes |

## 当前实现策略

1. ACP worker 按 `agent + cwd + proxy` 维度持久化，避免每轮重启 agent 进程。
2. 单个 worker 当前按顺序处理命令，保证同一 ACP 连接内的 `session/update` 与 `session/prompt` 收尾语义清晰。
3. `codex-acp` 的正常收尾以 `end_turn` 为准，不依赖某个单独的 update 事件。
4. `session/load` 期间 agent 可能会按协议回放历史 `session/update`；这些回放只用于恢复状态，不能被当作当前 turn 的新输出。
5. 当前实现会把 `session/load` 的历史回放缓存成最近对话摘要；当用户执行 `/runtime pick` 选中已有 session 时，gateway 会额外发送一张 `历史概览` 卡片，显示裁剪后的最近 5 轮 `user / assistant` 对话。
6. `session/list` 是当前 runtime/session 导入的首选路径；本地 sqlite/jsonl 发现只作为不支持该能力时的兼容回退。
7. 当前还没有实现 ACP `authenticate` 交互流；如果某个 agent 将来要求显式认证，需要按官方认证文档补齐。
