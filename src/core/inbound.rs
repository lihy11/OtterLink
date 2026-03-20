use serde::{Deserialize, Serialize};

use crate::core::models::OutboundMessage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreInboundRequest {
    pub session_key: String,
    #[serde(default)]
    pub parent_session_key: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreInboundResponse {
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub replies: Vec<OutboundMessage>,
    #[serde(default)]
    pub react_to_message: bool,
}
