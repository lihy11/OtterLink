use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};
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
        let (cancel, mut cancel_rx) = crate::agent::runtime::RuntimeCancelHandle::new();
        let primary = self.primary.clone();
        let fallback = self.fallback.clone();
        let current_cancel = Arc::new(Mutex::new(None::<crate::agent::runtime::RuntimeCancelHandle>));
        let current_cancel_for_signal = current_cancel.clone();
        tokio::spawn(async move {
            if cancel_rx.changed().await.is_ok() && *cancel_rx.borrow() {
                if let Some(handle) = current_cancel_for_signal.lock().await.clone() {
                    handle.cancel();
                }
            }
        });
        let completion = tokio::spawn(async move {
            match run_runtime(primary.clone(), request.clone(), &events_tx, current_cancel.clone()).await {
                Ok(done) => Ok(done),
                Err(primary_err) => {
                    if crate::agent::runtime::is_interrupted_error(&primary_err) {
                        return Err(primary_err);
                    }
                    let _ = fallback;
                    let agent_kind = request.agent_kind.as_deref().unwrap_or("unknown").to_string();
                    Err(anyhow!(
                        "runtime {} failed and fallback is disabled for agent `{}`: {primary_err}",
                        primary.name(),
                        agent_kind,
                    ))
                }
            }
        });

        Ok(RuntimeTurn {
            events: events_rx,
            completion,
            cancel,
            runtime_session_ref: None,
            runtime_turn_ref: None,
        })
    }

    async fn list_sessions(
        &self,
        query: crate::agent::runtime::RuntimeSessionQuery,
    ) -> Result<Vec<crate::agent::runtime::RuntimeSessionListing>> {
        match self.primary.list_sessions(query.clone()).await {
            Ok(sessions) => Ok(sessions),
            Err(primary_err) => {
                if crate::agent::runtime::is_list_sessions_unsupported_error(&primary_err) {
                    return self.fallback.list_sessions(query).await;
                }
                Err(primary_err)
            }
        }
    }

    async fn load_history(
        &self,
        query: crate::agent::runtime::RuntimeHistoryQuery,
    ) -> Result<Vec<crate::agent::runtime::RuntimeHistoryTurn>> {
        match self.primary.load_history(query.clone()).await {
            Ok(history) => Ok(history),
            Err(primary_err) => {
                if crate::agent::runtime::is_list_sessions_unsupported_error(&primary_err) {
                    return self.fallback.load_history(query).await;
                }
                Err(primary_err)
            }
        }
    }

    fn name(&self) -> &'static str {
        "fallback"
    }
}

async fn run_runtime(
    runtime: Arc<dyn AgentRuntime>,
    request: RuntimeTurnRequest,
    sink: &mpsc::UnboundedSender<RuntimeEvent>,
    current_cancel: Arc<Mutex<Option<crate::agent::runtime::RuntimeCancelHandle>>>,
) -> Result<RuntimeCompletion> {
    let mut turn = runtime.start_turn(request).await?;
    *current_cancel.lock().await = Some(turn.cancel.clone());
    while let Some(event) = turn.events.recv().await {
        let _ = sink.send(event);
    }
    *current_cancel.lock().await = None;
    turn.completion.await.context("join runtime task failed")?
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use anyhow::{anyhow, Result};
    use async_trait::async_trait;
    use tokio::sync::mpsc;

    use super::*;

    #[derive(Clone)]
    struct CountingRuntime {
        starts: Arc<AtomicUsize>,
        interrupted: bool,
        fail_message: Option<&'static str>,
    }

    #[async_trait]
    impl AgentRuntime for CountingRuntime {
        async fn start_turn(&self, _request: RuntimeTurnRequest) -> Result<RuntimeTurn> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            let (_tx, rx) = mpsc::unbounded_channel();
            let (cancel, _cancel_rx) = crate::agent::runtime::RuntimeCancelHandle::new();
            let interrupted = self.interrupted;
            let fail_message = self.fail_message;
            let completion = tokio::spawn(async move {
                if interrupted {
                    Err(anyhow!(crate::agent::runtime::INTERRUPTED_ERROR_TEXT))
                } else if let Some(message) = fail_message {
                    Err(anyhow!(message))
                } else {
                    Ok(RuntimeCompletion::default())
                }
            });
            Ok(RuntimeTurn {
                events: rx,
                completion,
                cancel,
                runtime_session_ref: None,
                runtime_turn_ref: None,
            })
        }

        fn name(&self) -> &'static str {
            "counting"
        }
    }

    #[tokio::test]
    async fn interrupted_primary_does_not_fallback() {
        let primary_starts = Arc::new(AtomicUsize::new(0));
        let fallback_starts = Arc::new(AtomicUsize::new(0));
        let runtime = FallbackRuntime::new(
            Arc::new(CountingRuntime {
                starts: primary_starts.clone(),
                interrupted: true,
                fail_message: None,
            }),
            Arc::new(CountingRuntime {
                starts: fallback_starts.clone(),
                interrupted: false,
                fail_message: None,
            }),
        );

        let turn = runtime
            .start_turn(RuntimeTurnRequest {
                session_key: "fallback:codex".to_string(),
                prompt: "hello".to_string(),
                runtime_session_ref: None,
                agent_kind: Some("codex".to_string()),
                workspace_path: None,
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();

        let result = turn.completion.await.unwrap();
        assert!(result.is_err());
        assert_eq!(primary_starts.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_starts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn primary_error_does_not_fallback_for_claude_code() {
        let primary_starts = Arc::new(AtomicUsize::new(0));
        let fallback_starts = Arc::new(AtomicUsize::new(0));
        let runtime = FallbackRuntime::new(
            Arc::new(CountingRuntime {
                starts: primary_starts.clone(),
                interrupted: false,
                fail_message: Some("acp initialize failed"),
            }),
            Arc::new(CountingRuntime {
                starts: fallback_starts.clone(),
                interrupted: false,
                fail_message: None,
            }),
        );

        let turn = runtime
            .start_turn(RuntimeTurnRequest {
                session_key: "fallback:claude".to_string(),
                prompt: "hello".to_string(),
                runtime_session_ref: None,
                agent_kind: Some("claude_code".to_string()),
                workspace_path: None,
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();

        let result = turn.completion.await.unwrap();
        assert!(result.is_err());
        let error_text = result.unwrap_err().to_string();
        assert!(error_text.contains("fallback is disabled"));
        assert_eq!(primary_starts.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_starts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn primary_error_does_not_fallback_for_codex() {
        let primary_starts = Arc::new(AtomicUsize::new(0));
        let fallback_starts = Arc::new(AtomicUsize::new(0));
        let runtime = FallbackRuntime::new(
            Arc::new(CountingRuntime {
                starts: primary_starts.clone(),
                interrupted: false,
                fail_message: Some("exec_json bootstrap failed"),
            }),
            Arc::new(CountingRuntime {
                starts: fallback_starts.clone(),
                interrupted: false,
                fail_message: None,
            }),
        );

        let turn = runtime
            .start_turn(RuntimeTurnRequest {
                session_key: "fallback:codex2".to_string(),
                prompt: "hello".to_string(),
                runtime_session_ref: None,
                agent_kind: Some("codex".to_string()),
                workspace_path: None,
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();

        let result = turn.completion.await.unwrap();
        assert!(result.is_err());
        let error_text = result.unwrap_err().to_string();
        assert!(error_text.contains("fallback is disabled"));
        assert_eq!(primary_starts.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_starts.load(Ordering::SeqCst), 0);
    }
}
