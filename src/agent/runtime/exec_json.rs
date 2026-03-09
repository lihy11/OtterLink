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
        let (cancel, mut cancel_rx) = crate::agent::runtime::RuntimeCancelHandle::new();
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
            apply_proxy_env(
                &mut cmd,
                request.proxy_mode.as_deref(),
                request.proxy_url.as_deref().or(config.acp_proxy_url.as_deref()),
                request.agent_kind.as_deref().unwrap_or("codex"),
            );

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

            let mut interrupted = false;
            loop {
                tokio::select! {
                    changed = cancel_rx.changed() => {
                        if changed.is_ok() && *cancel_rx.borrow() {
                            interrupted = true;
                            let _ = child.start_kill();
                            break;
                        }
                    }
                    maybe_line = out_lines.next_line() => {
                        let Some(line) = maybe_line.context("read codex stdout failed")? else {
                            break;
                        };
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
                }
            }

            let status = child.wait().await.context("wait codex process failed")?;
            let stderr_text = stderr_task.await.unwrap_or_default();
            if interrupted {
                return Err(anyhow!(crate::agent::runtime::INTERRUPTED_ERROR_TEXT));
            }
            if !status.success() {
                return Err(anyhow!(
                    "Codex 执行失败，status={}，stderr={}.",
                    status,
                    crate::core::support::shorten(&stderr_text, 500)
                ));
            }

            Ok(RuntimeCompletion {
                stderr_summary: (!stderr_text.trim().is_empty()).then_some(stderr_text),
                stop_reason: Some("end_turn".to_string()),
            })
        });

        Ok(RuntimeTurn {
            events: events_rx,
            completion,
            cancel,
        })
    }

    fn name(&self) -> &'static str {
        "exec_json"
    }
}

fn apply_proxy_env(
    cmd: &mut Command,
    proxy_mode: Option<&str>,
    proxy_url: Option<&str>,
    agent_kind: &str,
) {
    let effective_mode = match proxy_mode.unwrap_or("default") {
        "on" => "on",
        "off" => "off",
        _ if agent_kind == "codex" => "on",
        _ => "off",
    };

    match effective_mode {
        "off" => {
            for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"] {
                cmd.env_remove(key);
            }
        }
        "on" => {
            if let Some(value) = proxy_url.filter(|value| !value.trim().is_empty()) {
                for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"] {
                    cmd.env(key, value);
                }
            }
        }
        _ => {}
    }
}
