use std::sync::Arc;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::Value;
use tokio::{sync::{mpsc, watch}, task::JoinHandle};

use crate::{agent::normalized::NormalizedAgentEvent, config::Config};

pub const INTERRUPTED_ERROR_TEXT: &str = "runtime interrupted by user";
pub const LIST_SESSIONS_UNSUPPORTED_ERROR_TEXT: &str = "runtime session/list unsupported";

pub fn is_interrupted_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| cause.to_string().contains(INTERRUPTED_ERROR_TEXT))
}

pub fn is_list_sessions_unsupported_error(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.to_string().contains(LIST_SESSIONS_UNSUPPORTED_ERROR_TEXT))
}

#[derive(Clone)]
pub struct RuntimeTurnRequest {
    pub session_key: String,
    pub prompt: String,
    pub runtime_session_ref: Option<String>,
    pub agent_kind: Option<String>,
    pub workspace_path: Option<PathBuf>,
    pub proxy_mode: Option<String>,
    pub proxy_url: Option<String>,
}

#[derive(Clone)]
pub struct RuntimeSteerRequest {
    pub session_key: String,
    pub prompt: String,
    pub runtime_session_ref: String,
    pub runtime_turn_ref: String,
    pub agent_kind: Option<String>,
    pub workspace_path: Option<PathBuf>,
    pub proxy_mode: Option<String>,
    pub proxy_url: Option<String>,
}

#[derive(Clone)]
pub struct RuntimeSessionQuery {
    pub agent_kind: Option<String>,
    pub workspace_path: PathBuf,
    pub proxy_mode: Option<String>,
    pub proxy_url: Option<String>,
}

#[derive(Clone)]
pub struct RuntimeHistoryQuery {
    pub agent_kind: Option<String>,
    pub workspace_path: PathBuf,
    pub runtime_session_ref: String,
    pub proxy_mode: Option<String>,
    pub proxy_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSessionListing {
    pub runtime_session_ref: String,
    pub workspace_path: String,
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeHistoryTurn {
    pub user_text: String,
    pub assistant_text: String,
}

#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    Agent(NormalizedAgentEvent),
    TodoLog(Value),
}

#[derive(Debug, Default)]
pub struct RuntimeCompletion {
    pub stderr_summary: Option<String>,
    pub stop_reason: Option<String>,
}

#[derive(Clone)]
pub struct RuntimeCancelHandle {
    tx: watch::Sender<bool>,
}

impl RuntimeCancelHandle {
    pub fn new() -> (Self, watch::Receiver<bool>) {
        let (tx, rx) = watch::channel(false);
        (Self { tx }, rx)
    }

    pub fn cancel(&self) {
        let _ = self.tx.send(true);
    }
}

pub struct RuntimeTurn {
    pub events: mpsc::UnboundedReceiver<RuntimeEvent>,
    pub completion: JoinHandle<Result<RuntimeCompletion>>,
    pub cancel: RuntimeCancelHandle,
    pub runtime_session_ref: Option<String>,
    pub runtime_turn_ref: Option<String>,
}

#[async_trait]
pub trait AgentRuntime: Send + Sync {
    async fn start_turn(&self, request: RuntimeTurnRequest) -> Result<RuntimeTurn>;
    async fn steer_turn(&self, _request: RuntimeSteerRequest) -> Result<()> {
        Err(anyhow!("runtime steer unsupported"))
    }
    async fn list_sessions(&self, _query: RuntimeSessionQuery) -> Result<Vec<RuntimeSessionListing>> {
        Err(anyhow!(LIST_SESSIONS_UNSUPPORTED_ERROR_TEXT))
    }
    async fn load_history(
        &self,
        _query: RuntimeHistoryQuery,
    ) -> Result<Vec<RuntimeHistoryTurn>> {
        Err(anyhow!(LIST_SESSIONS_UNSUPPORTED_ERROR_TEXT))
    }
    fn name(&self) -> &'static str;
}

pub fn build_runtime(config: Arc<Config>) -> Arc<dyn AgentRuntime> {
    match config.runtime_mode.as_str() {
        "exec_json" => Arc::new(super::exec_json::ExecJsonRuntime::new(config)),
        "acp" => Arc::new(super::acp::AcpRuntime::new(config)),
        "codex_app_server" => Arc::new(super::codex_app_server::CodexAppServerRuntime::new(config)),
        "hybrid" | "acp_fallback" => Arc::new(super::router::RouterRuntime::new(config)),
        _ => Arc::new(super::fallback::FallbackRuntime::new(
            Arc::new(super::acp::AcpRuntime::new(config.clone())),
            Arc::new(super::exec_json::ExecJsonRuntime::new(config)),
        )),
    }
}
