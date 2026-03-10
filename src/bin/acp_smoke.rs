use std::process::Stdio;

use agent_client_protocol::{
    Agent, Client, ClientSideConnection, ContentBlock, Implementation, InitializeRequest,
    NewSessionRequest, PermissionOptionKind, PromptRequest, ProtocolVersion,
    RequestPermissionOutcome, RequestPermissionResponse, SelectedPermissionOutcome, SessionUpdate,
};
use anyhow::{anyhow, Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

struct SmokeClient {
    tx: mpsc::UnboundedSender<SessionUpdate>,
}

#[async_trait::async_trait(?Send)]
impl Client for SmokeClient {
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
        let _ = self.tx.send(args.update);
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cmd = std::env::args()
        .nth(1)
        .unwrap_or_else(|| {
            "npx -y -p @zed-industries/codex-acp@0.9.2 -p @zed-industries/codex-acp-linux-x64@0.9.2 codex-acp -c approval_policy=never -c sandbox_mode=\"danger-full-access\""
                .to_string()
        });
    let prompt = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "Say hello in one line.".to_string());

    let mut child = Command::new("sh");
    child
        .arg("-lc")
        .arg(cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = child.spawn().context("spawn ACP agent failed")?;

    let stdin = child.stdin.take().ok_or_else(|| anyhow!("missing stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("missing stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("missing stderr"))?;

    let mut stderr_lines = BufReader::new(stderr).lines();
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        while let Ok(Some(line)) = stderr_lines.next_line().await {
            buf.push_str(&line);
            buf.push('\n');
        }
        buf
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<SessionUpdate>();
    let local = tokio::task::LocalSet::new();

    let prompt_resp = local
        .run_until(async move {
            let client = SmokeClient { tx };
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
                    .client_info(Implementation::new("acp-smoke", "0.1.0")),
            )
            .await
            .map_err(|e| anyhow!("initialize failed: {e:?}"))?;

            let s = conn
                .new_session(NewSessionRequest::new(std::env::current_dir()?))
                .await
                .map_err(|e| anyhow!("new_session failed: {e:?}"))?;

            let prompt_fut = conn.prompt(PromptRequest::new(
                s.session_id,
                vec![ContentBlock::from(prompt)],
            ));
            tokio::pin!(prompt_fut);

            loop {
                tokio::select! {
                    upd = rx.recv() => {
                        match upd {
                            Some(SessionUpdate::AgentMessageChunk(chunk)) => {
                                println!("update.agent_message_chunk={:?}", chunk.content);
                            }
                            Some(SessionUpdate::ToolCall(call)) => {
                                println!("update.tool_call={} status={:?}", call.title, call.status);
                            }
                            Some(SessionUpdate::ToolCallUpdate(u)) => {
                                println!("update.tool_call_update={} status={:?}", u.tool_call_id.0, u.fields.status);
                            }
                            Some(SessionUpdate::Plan(plan)) => {
                                println!("update.plan.entries={}", plan.entries.len());
                            }
                            Some(_) => {}
                            None => return Err(anyhow!("updates channel closed")),
                        }
                    }
                    resp = &mut prompt_fut => {
                        let resp = resp.map_err(|e| anyhow!("prompt failed: {e:?}"))?;
                        break Ok::<_, anyhow::Error>(resp);
                    }
                }
            }
        })
        .await?;

    println!("prompt.stop_reason={:?}", prompt_resp.stop_reason);

    let _ = child.kill().await;
    let _ = child.wait().await;
    let stderr = stderr_task.await.unwrap_or_default();
    if !stderr.trim().is_empty() {
        println!("agent.stderr={}", stderr);
    }

    Ok(())
}
