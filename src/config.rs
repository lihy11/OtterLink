use std::{env, net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};

use crate::agent::runtime::adapters;

#[derive(Clone)]
pub struct Config {
    pub core_bind: SocketAddr,
    pub core_ingest_token: Option<String>,
    pub gateway_event_url: String,
    pub gateway_event_token: Option<String>,
    pub state_db_path: PathBuf,
    pub claude_home_dir: PathBuf,
    pub codex_bin: String,
    pub codex_workdir: PathBuf,
    pub codex_model: Option<String>,
    pub codex_skip_git_repo_check: bool,
    pub runtime_mode: String,
    pub acp_adapter: String,
    pub acp_agent_cmd: Option<String>,
    pub render_min_update_ms: u64,
    pub todo_event_log_path: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let core_bind = env::var("CORE_BIND")
            .or_else(|_| env::var("BIND"))
            .unwrap_or_else(|_| "127.0.0.1:3001".to_string())
            .parse()
            .context("invalid CORE_BIND, expected host:port")?;
        let core_ingest_token = env::var("CORE_INGEST_TOKEN")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let gateway_event_url = env::var("GATEWAY_EVENT_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:3000/internal/gateway/event".to_string());
        let gateway_event_token = env::var("GATEWAY_EVENT_TOKEN")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let state_db_path = env::var("STATE_DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".run/state.db"));
        let claude_home_dir = env::var("CLAUDE_HOME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                env::var("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(".claude")
            });
        let codex_bin = env::var("CODEX_BIN").unwrap_or_else(|_| "codex".to_string());
        let codex_workdir = env::var("CODEX_WORKDIR").map(PathBuf::from).unwrap_or(
            env::current_dir()
                .context("failed to get current dir")?
                .join("workspace"),
        );
        let codex_model = env::var("CODEX_MODEL").ok();
        let codex_skip_git_repo_check = env::var("CODEX_SKIP_GIT_REPO_CHECK")
            .ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(true);
        let runtime_mode = env::var("RUNTIME_MODE").unwrap_or_else(|_| "acp_fallback".to_string());
        let acp_adapter = env::var("ACP_ADAPTER").unwrap_or_else(|_| "codex".to_string());
        let acp_agent_cmd = env::var("ACP_AGENT_CMD")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(Some)
            .unwrap_or(Some(adapters::default_command(&acp_adapter)?));
        let render_min_update_ms = env::var("RENDER_MIN_UPDATE_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(700);
        let todo_event_log_path = env::var("TODO_EVENT_LOG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".run/todo-events.jsonl"));

        Ok(Self {
            core_bind,
            core_ingest_token,
            gateway_event_url,
            gateway_event_token,
            state_db_path,
            claude_home_dir,
            codex_bin,
            codex_workdir,
            codex_model,
            codex_skip_git_repo_check,
            runtime_mode,
            acp_adapter,
            acp_agent_cmd,
            render_min_update_ms,
            todo_event_log_path,
        })
    }
}
