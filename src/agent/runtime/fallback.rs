use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::warn;

use crate::agent::runtime::{AgentRuntime, RuntimeCompletion, RuntimeEvent, RuntimeTurn, RuntimeTurnRequest};

pub struct FallbackRuntime {
    primary: Arc<dyn AgentRuntime>,
    fallback: Arc<dyn AgentRuntime>,
}

impl FallbackRuntime {
    pub fn new(primary: Arc<dyn AgentRuntime>, fallback: Arc<dyn AgentRuntime>) -> Self {
        Self { primary, fallback }
    }
}

#[async_trait]
impl AgentRuntime for FallbackRuntime {
    async fn start_turn(&self, request: RuntimeTurnRequest) -> Result<RuntimeTurn> {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let primary = self.primary.clone();
        let fallback = self.fallback.clone();
        let completion = tokio::spawn(async move {
            match run_runtime(primary.clone(), request.clone(), &events_tx).await {
                Ok(done) => Ok(done),
                Err(primary_err) => {
                    warn!(
                        "runtime {} failed, fallback to {}: {primary_err:?}",
                        primary.name(),
                        fallback.name()
                    );
                    run_runtime(fallback, request, &events_tx)
                        .await
                        .with_context(|| format!("fallback after {} failure", primary.name()))
                }
            }
        });

        Ok(RuntimeTurn {
            events: events_rx,
            completion,
        })
    }

    fn name(&self) -> &'static str {
        "fallback"
    }
}

async fn run_runtime(
    runtime: Arc<dyn AgentRuntime>,
    request: RuntimeTurnRequest,
    sink: &mpsc::UnboundedSender<RuntimeEvent>,
) -> Result<RuntimeCompletion> {
    let mut turn = runtime.start_turn(request).await?;
    while let Some(event) = turn.events.recv().await {
        let _ = sink.send(event);
    }
    turn.completion.await.context("join runtime task failed")?
}
