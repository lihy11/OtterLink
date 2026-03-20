# 数据模型

## SQLite

### `core_sessions`

- `session_key` 主键
- `session_id` 逻辑会话 id
- `parent_session_id` 可选
- `runtime_session_ref` agent runtime 的续跑引用
- `last_assistant_message` 最近一次最终输出
- `updated_at`

### `core_turns`

- `turn_id` 主键
- `session_id`
- `input_text`
- `status`: `queued | running | completed | failed`
- `final_text`
- `error_text`
- `created_at`
- `updated_at`

### `runtime_instances`

- `runtime_id` 主键
- `session_key`
- `label`
- `agent_kind`
- `workspace_path`
- `runtime_session_ref`
  对于导入的 Claude 历史会话，这里保存 Claude 本地 `sessionId`
- `tag`
  导入时保存如 `gitBranch` 之类的标签
- `prompt_preview`
  导入时保存首条 prompt 摘录，用于列表展示
- `last_assistant_message`
- `created_at`
- `updated_at`

### `conversation_bindings`

- `session_key` 主键
- `active_runtime_id`
- `updated_at`

## Gateway 内存态

### Pairing Store

当前持久化的是允许访问的 `open_id` 集合，JSON 结构为：

```json
{
  "open_ids": ["ou_xxx", "ou_yyy"]
}
```

兼容旧格式：如果文件是对象映射，gateway 会把 value 视为 paired `open_id`。

### Turn Context Map

gateway 进程内维护：

- `turn_id -> replyToMessageId`
- `slotMessageIds.progress`
- `slotMessageIds.todo`
- `slotMessageIds.final`
- `openId`
- `session route`

这个上下文目前只保存在内存中，服务重启后不会恢复。

### Session Queue Map

gateway 进程内按 `session_key` 维护串行队列，保证同一飞书会话里的入站事件按顺序转发到 Rust。

### Message Dedup Cache

gateway 进程内按 `message_id` 维护 TTL 去重缓存，用来忽略飞书重推或重复投递的同一条消息。

### Active Turn Map

core 进程内还维护：

- `session_key -> active turn cancel handle`

用于 `/ot stop` 取消当前运行中的 turn。这个状态只存在内存中，服务重启后不会恢复。



## 本地 Session 导入

`/ot load [workspace]` 会按当前 agent 导入历史 session：

- `claude_code`：首选 ACP `session/list`
- `codex`：当前历史 thread 优先通过 app-server `thread/list` / `thread/read` 导入
- `claude_code` 回退：`CLAUDE_HOME_DIR/projects/<workspace-key>/`
- `codex` 回退：`CODEX_HOME_DIR/state_5.sqlite` 的 `threads` 表

当前聊天还会保存一份 `runtime_selection`，记录当前选中的 `agent_kind / workspace_path / selected_runtime_id`。
当前实现同时把 `proxy_mode / proxy_url` 也保存在 `runtime_selection` 中，用于控制后续 runtime 启动时的代理注入。
`/ot ...` 命令的解析和普通消息是否进入 turn，也都由 Rust 基于这份选择器状态决定；gateway 不再本地解析 slash 命令。
当 `proxy_mode=default` 时，最终行为由 env 里的 `CLAUDE_CODE_DEFAULT_PROXY_MODE` / `CODEX_DEFAULT_PROXY_MODE` 决定。

- Claude 回退时优先读取 `sessions-index.json`
- Claude 没有索引时回退读取目录下的 `*.jsonl`
- Codex 回退时按 `threads.cwd` 与当前 workspace 匹配导入
- 导入后的 runtime 仍然存入 `runtime_instances`
- 当前聊天的 active runtime 不会因为导入而自动切换

## 本地配置文件

`.run/feishu.env` 现在也是正式的数据载体，默认由 `otterlink configure` 生成和维护，保存：

- 飞书连接参数与 `APP_ID/APP_SECRET`
- gateway/core token
- runtime 默认 agent 与 workspace
- 默认代理 URL
- `claude_code` / `codex` 各自的默认代理模式
