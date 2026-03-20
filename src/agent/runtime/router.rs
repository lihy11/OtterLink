use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::{
    agent::runtime::{
        acp::AcpRuntime, AgentRuntime, RuntimeHistoryQuery, RuntimeHistoryTurn,
        RuntimeSessionListing, RuntimeSessionQuery, RuntimeSteerRequest, RuntimeTurn,
        RuntimeTurnRequest,
    },
    config::Config,
};

use super::codex_app_server::CodexAppServerRuntime;

pub struct RouterRuntime {
    claude_runtime: Arc<AcpRuntime>,
    codex_runtime: Arc<CodexAppServerRuntime>,
}

impl RouterRuntime {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            claude_runtime: Arc::new(AcpRuntime::new(config.clone())),
            codex_runtime: Arc::new(CodexAppServerRuntime::new(config)),
        }
    }

    fn runtime_for_agent(&self, agent_kind: Option<&str>) -> Arc<dyn AgentRuntime> {
        match agent_kind.unwrap_or("claude_code") {
            "codex" => self.codex_runtime.clone(),
            _ => self.claude_runtime.clone(),
        }
    }
}

#[async_trait]
impl AgentRuntime for RouterRuntime {
    async fn start_turn(&self, request: RuntimeTurnRequest) -> Result<RuntimeTurn> {
        self.runtime_for_agent(request.agent_kind.as_deref())
            .start_turn(request)
            .await
    }

    async fn steer_turn(&self, request: RuntimeSteerRequest) -> Result<()> {
        self.runtime_for_agent(request.agent_kind.as_deref())
            .steer_turn(request)
            .await
    }

    async fn list_sessions(&self, query: RuntimeSessionQuery) -> Result<Vec<RuntimeSessionListing>> {
        self.runtime_for_agent(query.agent_kind.as_deref())
            .list_sessions(query)
            .await
    }

    async fn load_history(&self, query: RuntimeHistoryQuery) -> Result<Vec<RuntimeHistoryTurn>> {
        self.runtime_for_agent(query.agent_kind.as_deref())
            .load_history(query)
            .await
    }

    fn name(&self) -> &'static str {
        "router"
    }
}
