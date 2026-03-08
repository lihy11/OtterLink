use agent_client_protocol::{ContentBlock, PlanEntryStatus, SessionUpdate, ToolCallStatus};
use serde_json::Value;

use crate::core::models::TodoEntry;

#[derive(Debug, Clone)]
pub enum AgentToolState {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub enum NormalizedAgentEvent {
    TurnStarted,
    TurnCompleted,
    RuntimeSessionReady(String),
    AssistantChunk(String),
    AssistantMessage(String),
    ToolState { tool_call_id: String, state: AgentToolState },
    PlanUpdated(Vec<TodoEntry>),
    Usage(Value),
}

pub fn normalize_acp_update(update: SessionUpdate) -> Vec<NormalizedAgentEvent> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
            ContentBlock::Text(text) => vec![NormalizedAgentEvent::AssistantChunk(text.text)],
            _ => Vec::new(),
        },
        SessionUpdate::ToolCall(call) => vec![NormalizedAgentEvent::ToolState {
            tool_call_id: call.tool_call_id.0.to_string(),
            state: map_tool_state(call.status),
        }],
        SessionUpdate::ToolCallUpdate(update) => update
            .fields
            .status
            .map(|status| NormalizedAgentEvent::ToolState {
                tool_call_id: update.tool_call_id.0.to_string(),
                state: map_tool_state(status),
            })
            .into_iter()
            .collect(),
        SessionUpdate::Plan(plan) => vec![NormalizedAgentEvent::PlanUpdated(
            plan.entries
                .iter()
                .map(|entry| TodoEntry {
                    content: entry.content.clone(),
                    status: match entry.status {
                        PlanEntryStatus::Completed => "completed".to_string(),
                        PlanEntryStatus::InProgress => "in_progress".to_string(),
                        PlanEntryStatus::Pending => "pending".to_string(),
                        _ => "pending".to_string(),
                    },
                })
                .collect(),
        )],
        _ => Vec::new(),
    }
}

pub fn normalize_exec_json_event(value: &Value) -> Vec<NormalizedAgentEvent> {
    let typ = value.get("type").and_then(Value::as_str).unwrap_or("");
    let mut events = Vec::new();

    match typ {
        "thread.started" => {
            if let Some(thread_id) = value.get("thread_id").and_then(Value::as_str) {
                events.push(NormalizedAgentEvent::RuntimeSessionReady(thread_id.to_string()));
            }
        }
        "turn.started" => events.push(NormalizedAgentEvent::TurnStarted),
        "item.started" => {
            if let Some(item) = value.get("item") {
                if item.get("type").and_then(Value::as_str) == Some("command_execution") {
                    let tool_call_id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("command_execution")
                        .to_string();
                    events.push(NormalizedAgentEvent::ToolState {
                        tool_call_id,
                        state: AgentToolState::InProgress,
                    });
                }
            }
        }
        "item.completed" => {
            if let Some(item) = value.get("item") {
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("unknown");
                if item_type == "agent_message" {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        events.push(NormalizedAgentEvent::AssistantMessage(text.to_string()));
                    }
                } else if item_type == "command_execution" {
                    let tool_call_id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("command_execution")
                        .to_string();
                    events.push(NormalizedAgentEvent::ToolState {
                        tool_call_id,
                        state: AgentToolState::Completed,
                    });
                }
            }
        }
        "turn.completed" => {
            events.push(NormalizedAgentEvent::TurnCompleted);
            events.push(NormalizedAgentEvent::Usage(
                value.get("usage").cloned().unwrap_or_else(|| serde_json::json!({})),
            ));
        }
        _ => {}
    }

    if let Some(todos) = extract_todos_from_event(value) {
        events.push(NormalizedAgentEvent::PlanUpdated(todos));
    }

    events
}

fn map_tool_state(status: ToolCallStatus) -> AgentToolState {
    match status {
        ToolCallStatus::Pending => AgentToolState::Pending,
        ToolCallStatus::InProgress => AgentToolState::InProgress,
        ToolCallStatus::Completed => AgentToolState::Completed,
        ToolCallStatus::Failed => AgentToolState::Failed,
        _ => AgentToolState::Pending,
    }
}

fn extract_todos_from_event(value: &Value) -> Option<Vec<TodoEntry>> {
    let mut candidates: Vec<&Value> = Vec::new();
    if let Some(item) = value.get("item") {
        if let Some(v) = item.get("todos") {
            candidates.push(v);
        }
        if let Some(v) = item.get("todo_list") {
            candidates.push(v);
        }
        if let Some(v) = item.get("output").and_then(|o| o.get("todos")) {
            candidates.push(v);
        }
        if let Some(v) = item.get("result").and_then(|o| o.get("todos")) {
            candidates.push(v);
        }
        if let Some(t) = item.get("type").and_then(Value::as_str) {
            if t.contains("todo") {
                if let Some(v) = item.get("todos") {
                    candidates.push(v);
                }
            }
        }
        if let Some(tool_name) = item.get("name").and_then(Value::as_str) {
            if tool_name.to_lowercase().contains("todo") {
                if let Some(v) = item.get("output").and_then(|o| o.get("todos")) {
                    candidates.push(v);
                }
            }
        }
    }
    if let Some(v) = value.get("todos") {
        candidates.push(v);
    }

    for candidate in candidates {
        if let Some(arr) = candidate.as_array() {
            let todos = arr
                .iter()
                .filter_map(|it| {
                    let content = it
                        .get("content")
                        .or_else(|| it.get("title"))
                        .or_else(|| it.get("text"))
                        .and_then(Value::as_str)?;
                    let status = it
                        .get("status")
                        .or_else(|| it.get("state"))
                        .and_then(Value::as_str)
                        .unwrap_or("pending");
                    Some(TodoEntry {
                        content: content.to_string(),
                        status: status.to_string(),
                    })
                })
                .collect::<Vec<_>>();
            if !todos.is_empty() {
                return Some(todos);
            }
        }
    }

    None
}
