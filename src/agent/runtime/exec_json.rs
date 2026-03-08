use std::process::Stdio;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::mpsc,
};

use crate::{
    agent::{
        normalized::{normalize_exec_json_event, NormalizedAgentEvent},
        runtime::{AgentRuntime, RuntimeCompletion, RuntimeEvent, RuntimeTurn, RuntimeTurnRequest},
    },
    config::Config,
};

pub struct ExecJsonRuntime {
    config: Arc<Config>,
}

impl ExecJsonRuntime {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }

    fn build_command(&self, runtime_session_ref: Option<&str>, prompt: &str) -> Command {
        let mut cmd = Command::new(&self.config.codex_bin);

        if let Some(runtime_ref) = runtime_session_ref {
            cmd.arg("exec").arg("resume");
            cmd.arg("--yolo");
            if self.config.codex_skip_git_repo_check {
                cmd.arg("--skip-git-repo-check");
            }
            cmd.arg("--json");
            if let Some(model) = self.config.codex_model.as_ref() {
                cmd.arg("-m").arg(model);
            }
            cmd.arg(runtime_ref);
            cmd.arg(prompt);
        } else {
            cmd.arg("exec");
            cmd.arg("--yolo");
            if self.config.codex_skip_git_repo_check {
                cmd.arg("--skip-git-repo-check");
            }
            cmd.arg("--json");
            if let Some(model) = self.config.codex_model.as_ref() {
                cmd.arg("-m").arg(model);
            }
            cmd.arg(prompt);
        }

        cmd
    }
}

#[async_trait]
impl AgentRuntime for ExecJsonRuntime {
    async fn start_turn(&self, request: RuntimeTurnRequest) -> Result<RuntimeTurn> {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let config = self.config.clone();
        let completion = tokio::spawn(async move {
            let workspace_path = request
                .workspace_path
                .clone()
                .unwrap_or_else(|| config.codex_workdir.clone());
            let mut cmd = ExecJsonRuntime { config: config.clone() }
                .build_command(request.runtime_session_ref.as_deref(), &request.prompt);

            cmd.current_dir(workspace_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(Stdio::null())
                .kill_on_drop(true);

            let mut child = cmd.spawn().context("failed to spawn codex process")?;
            let stdout = child.stdout.take().ok_or_else(|| anyhow!("missing stdout"))?;
            let stderr = child.stderr.take().ok_or_else(|| anyhow!("missing stderr"))?;

            let mut out_lines = BufReader::new(stdout).lines();
            let mut err_lines = BufReader::new(stderr).lines();
            let stderr_task = tokio::spawn(async move {
                let mut stderr_buf = String::new();
                while let Ok(Some(line)) = err_lines.next_line().await {
                    stderr_buf.push_str(&line);
                    stderr_buf.push('\n');
                }
                stderr_buf
            });

            while let Some(line) = out_lines
                .next_line()
                .await
                .context("read codex stdout failed")?
            {
                let line_trimmed = line.trim();
                if line_trimmed.is_empty() {
                    continue;
                }

                let value: Value = match serde_json::from_str(line_trimmed) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let events = normalize_exec_json_event(&value);
                let has_todo_update = events
                    .iter()
                    .any(|event| matches!(event, NormalizedAgentEvent::PlanUpdated(_)));
                for event in events {
                    let _ = events_tx.send(RuntimeEvent::Agent(event));
                }

                if has_todo_update {
                    let _ = events_tx.send(RuntimeEvent::TodoLog(json!(value)));
                }
            }

            let status = child.wait().await.context("wait codex process failed")?;
            let stderr_text = stderr_task.await.unwrap_or_default();
            if !status.success() {
                return Err(anyhow!(
                    "Codex 执行失败，status={}，stderr={}.",
                    status,
                    crate::core::support::shorten(&stderr_text, 500)
                ));
            }

            Ok(RuntimeCompletion {
                stderr_summary: (!stderr_text.trim().is_empty()).then_some(stderr_text),
            })
        });

        Ok(RuntimeTurn {
            events: events_rx,
            completion,
        })
    }

    fn name(&self) -> &'static str {
        "exec_json"
    }
}
