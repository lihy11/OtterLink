use std::sync::Arc;
use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{agent::normalized::NormalizedAgentEvent, config::Config};

#[derive(Clone)]
pub struct RuntimeTurnRequest {
    pub prompt: String,
    pub runtime_session_ref: Option<String>,
    pub agent_kind: Option<String>,
    pub workspace_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    Agent(NormalizedAgentEvent),
    TodoLog(Value),
}

#[derive(Debug, Default)]
pub struct RuntimeCompletion {
    pub stderr_summary: Option<String>,
}

pub struct RuntimeTurn {
    pub events: mpsc::UnboundedReceiver<RuntimeEvent>,
    pub completion: JoinHandle<Result<RuntimeCompletion>>,
}

#[async_trait]
pub trait AgentRuntime: Send + Sync {
    async fn start_turn(&self, request: RuntimeTurnRequest) -> Result<RuntimeTurn>;
    fn name(&self) -> &'static str;
}

pub fn build_runtime(config: Arc<Config>) -> Arc<dyn AgentRuntime> {
    match config.runtime_mode.as_str() {
        "exec_json" => Arc::new(super::exec_json::ExecJsonRuntime::new(config)),
        "acp" => Arc::new(super::acp::AcpRuntime::new(config)),
        _ => Arc::new(super::fallback::FallbackRuntime::new(
            Arc::new(super::acp::AcpRuntime::new(config.clone())),
            Arc::new(super::exec_json::ExecJsonRuntime::new(config)),
        )),
    }
}
