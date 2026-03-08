use std::process::Stdio;
use std::sync::Arc;

use agent_client_protocol::{
    Agent, Client as AcpClientTrait, ClientSideConnection, ContentBlock, Implementation,
    InitializeRequest, NewSessionRequest, PermissionOptionKind, PromptRequest, ProtocolVersion,
    RequestPermissionOutcome, RequestPermissionResponse, SelectedPermissionOutcome, SessionUpdate,
    SetSessionModeRequest,
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    sync::mpsc,
};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::{
    agent::{
        normalized::normalize_acp_update,
        runtime::{adapters::{self, AcpAdapterSpec}, AgentRuntime, RuntimeCompletion, RuntimeEvent, RuntimeTurn, RuntimeTurnRequest},
    },
    config::Config,
};

pub struct AcpRuntimeProcess {
    pub child: Child,
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub stderr_task: tokio::task::JoinHandle<String>,
    pub adapter: AcpAdapterSpec,
}

pub struct AcpRuntime {
    config: Arc<Config>,
}

impl AcpRuntime {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }

    async fn spawn(&self, request: &RuntimeTurnRequest) -> Result<AcpRuntimeProcess> {
        let adapter_id = request
            .agent_kind
            .as_deref()
            .unwrap_or(&self.config.acp_adapter);
        let adapter = adapters::for_id(adapter_id)?;
        let acp_agent_cmd = if adapter_id == self.config.acp_adapter {
            self.config
                .acp_agent_cmd
                .clone()
                .unwrap_or_else(|| adapter.default_command.to_string())
        } else {
            adapter.default_command.to_string()
        };
        let workdir = request
            .workspace_path
            .clone()
            .unwrap_or_else(|| self.config.codex_workdir.clone());

        let mut child = Command::new("sh");
        child
            .arg("-lc")
            .arg(acp_agent_cmd)
            .current_dir(workdir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = child.spawn().context("failed to spawn ACP agent process")?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("missing ACP agent stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("missing ACP agent stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("missing ACP agent stderr"))?;

        Ok(AcpRuntimeProcess {
            child,
            stdin,
            stdout,
            stderr_task: spawn_stderr_task(stderr),
            adapter,
        })
    }
}

#[async_trait]
impl AgentRuntime for AcpRuntime {
    async fn start_turn(&self, request: RuntimeTurnRequest) -> Result<RuntimeTurn> {
        let process = self.spawn(&request).await?;
        let cwd = request
            .workspace_path
            .clone()
            .unwrap_or_else(|| self.config.codex_workdir.clone());
        let (events_tx, events_rx) = mpsc::unbounded_channel();

        let completion = tokio::task::spawn_blocking(move || -> Result<RuntimeCompletion> {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("build acp runtime failed")?;

            rt.block_on(async move {
                let AcpRuntimeProcess {
                    mut child,
                    stdin,
                    stdout,
                    stderr_task,
                    adapter,
                } = process;
                let (updates_tx, mut updates_rx) = mpsc::unbounded_channel::<SessionUpdate>();
                let prompt = request.prompt;
                let local = tokio::task::LocalSet::new();

                let run_result = local
                    .run_until(async move {
                        let client = AcpBridgeClient { updates_tx };
                        let (conn, io_task) = ClientSideConnection::new(
                            client,
                            stdin.compat_write(),
                            stdout.compat(),
                            |fut| {
                                tokio::task::spawn_local(fut);
                            },
                        );

                        tokio::task::spawn_local(async move {
                            let _ = io_task.await;
                        });

                        conn.initialize(
                            InitializeRequest::new(ProtocolVersion::LATEST)
                                .client_info(Implementation::new("feishu-acp-bridge", "0.1.0")),
                        )
                        .await
                        .map_err(|e| anyhow!("acp initialize failed: {e:?}"))?;

                        let new_session = conn
                            .new_session(NewSessionRequest::new(&cwd))
                            .await
                            .map_err(|e| anyhow!("acp new_session failed: {e:?}"))?;
                        let _ = events_tx.send(RuntimeEvent::Agent(
                            crate::agent::normalized::NormalizedAgentEvent::RuntimeSessionReady(
                                new_session.session_id.to_string(),
                            ),
                        ));

                        if let Some(session_mode) = adapter.session_mode {
                            let _ = conn
                                .set_session_mode(SetSessionModeRequest::new(
                                    new_session.session_id.to_string(),
                                    session_mode,
                                ))
                                .await;
                        }

                        let mut prompt_fut = Box::pin(conn.prompt(PromptRequest::new(
                            new_session.session_id.to_string(),
                            vec![ContentBlock::from(prompt)],
                        )));

                        loop {
                            tokio::select! {
                                maybe_upd = updates_rx.recv() => {
                                    if let Some(upd) = maybe_upd {
                                        for event in normalize_acp_update(upd) {
                                            let _ = events_tx.send(RuntimeEvent::Agent(event));
                                        }
                                    } else {
                                        return Err(anyhow!("acp updates channel closed before prompt completed"));
                                    }
                                }
                                resp = &mut prompt_fut => {
                                    resp.map_err(|e| anyhow!("acp prompt failed: {e:?}"))?;
                                    while let Ok(upd) = updates_rx.try_recv() {
                                        for event in normalize_acp_update(upd) {
                                            let _ = events_tx.send(RuntimeEvent::Agent(event));
                                        }
                                    }
                                    break Ok::<(), anyhow::Error>(());
                                }
                            }
                        }
                    })
                    .await;

                let _ = child.kill().await;
                let _ = child.wait().await;
                let stderr_text = stderr_task.await.unwrap_or_default();
                run_result?;
                Ok(RuntimeCompletion {
                    stderr_summary: (!stderr_text.trim().is_empty()).then_some(stderr_text),
                })
            })
        });

        Ok(RuntimeTurn {
            events: events_rx,
            completion,
        })
    }

    fn name(&self) -> &'static str {
        "acp"
    }
}

fn spawn_stderr_task(stderr: ChildStderr) -> tokio::task::JoinHandle<String> {
    tokio::spawn(async move {
        let mut stderr_lines = BufReader::new(stderr).lines();
        let mut buf = String::new();
        while let Ok(Some(line)) = stderr_lines.next_line().await {
            buf.push_str(&line);
            buf.push('\n');
        }
        buf
    })
}

pub struct AcpBridgeClient {
    pub updates_tx: mpsc::UnboundedSender<SessionUpdate>,
}

#[async_trait::async_trait(?Send)]
impl AcpClientTrait for AcpBridgeClient {
    async fn request_permission(
        &self,
        args: agent_client_protocol::RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        let selected = args
            .options
            .iter()
            .find(|o| {
                matches!(
                    o.kind,
                    PermissionOptionKind::AllowAlways | PermissionOptionKind::AllowOnce
                )
            })
            .or_else(|| args.options.first())
            .map(|o| o.option_id.clone())
            .unwrap_or_else(|| "allow-once".into());

        Ok(RequestPermissionResponse::new(
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(selected)),
        ))
    }

    async fn session_notification(
        &self,
        args: agent_client_protocol::SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        let _ = self.updates_tx.send(args.update);
        Ok(())
    }
}
