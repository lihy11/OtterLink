use serde::{Deserialize, Serialize};

use crate::core::models::OutboundMessage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreTurnRequest {
    pub turn_id: String,
    pub session_key: String,
    #[serde(default)]
    pub parent_session_key: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreTurnAccepted {
    pub ok: bool,
    pub turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreControlRequest {
    pub session_key: String,
    #[serde(default)]
    pub parent_session_key: Option<String>,
    pub action: ControlAction,
    #[serde(default)]
    pub runtime_selector: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub agent_kind: Option<String>,
    #[serde(default)]
    pub proxy_mode: Option<String>,
    #[serde(default)]
    pub proxy_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreControlResponse {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub selector: Option<RuntimeSelectorSummary>,
    #[serde(default)]
    pub active_runtime: Option<RuntimeSummary>,
    #[serde(default)]
    pub runtimes: Vec<RuntimeSummary>,
    #[serde(default)]
    pub history_overview: Option<RuntimeHistoryOverview>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlAction {
    ShowRuntime,
    ListRuntimes,
    LoadRuntimes,
    UseAgent,
    CreateRuntime,
    SwitchRuntime,
    SetWorkspace,
    SetProxy,
    StopRuntime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSelectorSummary {
    pub agent_kind: String,
    pub workspace_path: String,
    pub has_selected_runtime: bool,
    pub proxy_mode: String,
    #[serde(default)]
    pub proxy_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSummary {
    pub runtime_id: String,
    pub label: String,
    pub agent_kind: String,
    pub workspace_path: String,
    #[serde(default)]
    pub runtime_session_ref: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub prompt_preview: Option<String>,
    pub has_runtime_session_ref: bool,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeHistoryOverview {
    pub runtime_session_ref: String,
    pub turns: Vec<RuntimeHistoryTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeHistoryTurn {
    pub user_text: String,
    pub assistant_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutboundSlot {
    Progress,
    Todo,
    Final,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreOutboundEvent {
    pub turn_id: String,
    pub slot: OutboundSlot,
    pub message: OutboundMessage,
}
