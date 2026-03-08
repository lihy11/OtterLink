use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::{error, info};

use crate::{
    agent::{
        normalized::NormalizedAgentEvent,
        runtime::{AgentRuntime, RuntimeEvent, RuntimeTurnRequest},
    },
    config::Config,
    core::{
        message_builder::{card_message, text_message},
        models::{CardBlock, CardTheme, OutboundMessage, TodoEntry},
        persistence::{Persistence, RuntimeInstance},
        ports::TurnEventSink,
        registry::{SessionInfo, SessionRegistry},
        support::{append_jsonl, shorten},
    },
    protocol::{
        ControlAction, CoreControlRequest, CoreControlResponse, CoreOutboundEvent, CoreTurnAccepted,
        CoreTurnRequest, OutboundSlot, RuntimeSummary,
    },
};

#[derive(Clone, Default)]
pub struct SessionLocks {
    locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl SessionLocks {
    async fn get(&self, session_key: &str) -> Arc<Mutex<()>> {
        let mut guard = self.locks.lock().await;
        guard
            .entry(session_key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

#[derive(Clone)]
pub struct CoreService {
    config: Arc<Config>,
    runtime: Arc<dyn AgentRuntime>,
    sink: Arc<dyn TurnEventSink>,
    pub persistence: Persistence,
    pub registry: SessionRegistry,
    session_locks: SessionLocks,
}

impl CoreService {
    pub fn new(
        config: Arc<Config>,
        runtime: Arc<dyn AgentRuntime>,
        sink: Arc<dyn TurnEventSink>,
        persistence: Persistence,
        registry: SessionRegistry,
    ) -> Self {
        Self {
            config,
            runtime,
            sink,
            persistence,
            registry,
            session_locks: SessionLocks::default(),
        }
    }

    pub async fn accept_turn(&self, request: CoreTurnRequest) -> Result<CoreTurnAccepted> {
        let session = self
            .registry
            .resolve(&request.session_key, request.parent_session_key.as_deref())
            .await
            .context("resolve session failed")?;
        let runtime = self.ensure_active_runtime(&session).await?;

        self.persistence
            .create_turn(&request.turn_id, &session.session_id, &request.text)
            .await
            .context("create turn failed")?;

        let service = self.clone();
        let turn_id = request.turn_id.clone();
        tokio::spawn(async move {
            if let Err(err) = service.run_turn(request, session, runtime).await {
                error!("turn failed: {err:?}");
            }
        });

        Ok(CoreTurnAccepted {
            ok: true,
            turn_id,
        })
    }

    pub async fn handle_control(&self, request: CoreControlRequest) -> Result<CoreControlResponse> {
        let session = self
            .registry
            .resolve(&request.session_key, request.parent_session_key.as_deref())
            .await
            .context("resolve session failed")?;

        let response = match request.action {
            ControlAction::ShowRuntime => {
                let active = self.ensure_active_runtime(&session).await?;
                CoreControlResponse {
                    ok: true,
                    message: format!(
                        "当前会话已绑定 `{}`，workspace=`{}`。",
                        active.label, active.workspace_path
                    ),
                    active_runtime: Some(runtime_summary(&active)),
                    runtimes: Vec::new(),
                }
            }
            ControlAction::ListRuntimes => {
                let active = self.ensure_active_runtime(&session).await?;
                let runtimes = self.persistence.list_runtimes(&session.session_key).await?;
                CoreControlResponse {
                    ok: true,
                    message: format!("当前共有 {} 个 runtime。", runtimes.len()),
                    active_runtime: Some(runtime_summary(&active)),
                    runtimes: runtimes.iter().map(runtime_summary).collect(),
                }
            }
            ControlAction::LoadRuntimes => {
                let active = self.ensure_active_runtime(&session).await?;
                let workspace =
                    self.resolve_runtime_load_workspace(request.workspace_path.as_deref(), &active)?;
                let imported = self.load_claude_runtimes(&session.session_key, &workspace).await?;
                let active = self.ensure_active_runtime(&session).await?;
                let runtimes = self.persistence.list_runtimes(&session.session_key).await?;
                CoreControlResponse {
                    ok: true,
                    message: if imported == 0 {
                        format!("在 `{}` 下没有找到可导入的 Claude session。", workspace)
                    } else {
                        format!("已从 `{}` 导入 {} 个 Claude session。", workspace, imported)
                    },
                    active_runtime: Some(runtime_summary(&active)),
                    runtimes: runtimes.iter().map(runtime_summary).collect(),
                }
            }
            ControlAction::CreateRuntime => {
                let workspace = self.resolve_workspace_path(request.workspace_path.as_deref())?;
                let agent_kind = request
                    .agent_kind
                    .clone()
                    .unwrap_or_else(|| self.config.acp_adapter.clone());
                let runtime = self
                    .persistence
                    .create_runtime(
                        &session.session_key,
                        request
                            .label
                            .as_deref()
                            .unwrap_or(&default_runtime_label(&agent_kind, &workspace)),
                        &agent_kind,
                        &workspace,
                        true,
                    )
                    .await?;
                self.activate_runtime(&session, &runtime).await?;
                CoreControlResponse {
                    ok: true,
                    message: format!("已新建并切换到 `{}`。", runtime.label),
                    active_runtime: Some(runtime_summary(&runtime)),
                    runtimes: self
                        .persistence
                        .list_runtimes(&session.session_key)
                        .await?
                        .iter()
                        .map(runtime_summary)
                        .collect(),
                }
            }
            ControlAction::SwitchRuntime => {
                let selector = request
                    .runtime_selector
                    .as_deref()
                    .ok_or_else(|| anyhow!("switch_runtime requires runtime_selector"))?;
                let runtimes = self.persistence.list_runtimes(&session.session_key).await?;
                let runtime = runtimes
                    .into_iter()
                    .find(|runtime| runtime_matches_selector(runtime, selector))
                    .ok_or_else(|| anyhow!("runtime not found: {}", selector))?;
                self.activate_runtime(&session, &runtime).await?;
                CoreControlResponse {
                    ok: true,
                    message: format!("已切换到 `{}`。", runtime.label),
                    active_runtime: Some(runtime_summary(&runtime)),
                    runtimes: self
                        .persistence
                        .list_runtimes(&session.session_key)
                        .await?
                        .iter()
                        .map(runtime_summary)
                        .collect(),
                }
            }
            ControlAction::SetWorkspace => {
                let workspace = self.resolve_workspace_path(request.workspace_path.as_deref())?;
                let active = self.ensure_active_runtime(&session).await?;
                let runtime = if active.runtime_session_ref.is_some() {
                    let runtime = self
                        .persistence
                        .create_runtime(
                            &session.session_key,
                            request
                                .label
                                .as_deref()
                                .unwrap_or(&default_runtime_label(&active.agent_kind, &workspace)),
                            &active.agent_kind,
                            &workspace,
                            true,
                        )
                        .await?;
                    self.activate_runtime(&session, &runtime).await?;
                    runtime
                } else {
                    self.persistence
                        .update_runtime_workspace(&active.runtime_id, &workspace)
                        .await?;
                    let mut updated = active.clone();
                    updated.workspace_path = workspace.clone();
                    self.activate_runtime(&session, &updated).await?;
                    updated
                };

                CoreControlResponse {
                    ok: true,
                    message: format!("当前 workspace 已切换到 `{}`。", runtime.workspace_path),
                    active_runtime: Some(runtime_summary(&runtime)),
                    runtimes: self
                        .persistence
                        .list_runtimes(&session.session_key)
                        .await?
                        .iter()
                        .map(runtime_summary)
                        .collect(),
                }
            }
        };

        Ok(response)
    }

    async fn run_turn(
        &self,
        request: CoreTurnRequest,
        session: SessionInfo,
        runtime: RuntimeInstance,
    ) -> Result<()> {
        let session_lock = self.session_locks.get(&session.session_key).await;
        let _guard = session_lock.lock().await;

        self.persistence.mark_turn_running(&request.turn_id).await?;

        let prompt = self.build_prompt(&session, &request.text).await;
        let mut render = TurnRenderState::new(&session);
        self.publish(&request.turn_id, OutboundSlot::Progress, render.progress_message())
            .await?;

        let mut turn = self
            .runtime
            .start_turn(RuntimeTurnRequest {
                prompt,
                runtime_session_ref: runtime.runtime_session_ref.clone(),
                agent_kind: Some(runtime.agent_kind.clone()),
                workspace_path: Some(runtime.workspace_path.clone().into()),
            })
            .await
            .context("start runtime turn failed")?;

        while let Some(event) = turn.events.recv().await {
            match event {
                RuntimeEvent::Agent(agent_event) => {
                    let todo_changed = render.apply_event(agent_event);
                    if todo_changed {
                        self.publish(&request.turn_id, OutboundSlot::Todo, render.todo_message())
                            .await?;
                    }
                }
                RuntimeEvent::TodoLog(value) => {
                    let line = json!({
                        "turn_id": request.turn_id,
                        "event": value,
                    });
                    let _ = append_jsonl(&self.config.todo_event_log_path, &line, "todo log").await;
                }
            }

            if render.should_flush(self.config.render_min_update_ms) {
                self.publish(&request.turn_id, OutboundSlot::Progress, render.progress_message())
                    .await?;
                render.mark_flushed();
            }
        }

        let completion = match turn.completion.await.context("join runtime turn failed")? {
            Ok(done) => done,
            Err(err) => {
                let error_text = err.to_string();
                render.fail(error_text.clone());
                let _ = self
                    .publish(&request.turn_id, OutboundSlot::Progress, render.progress_message())
                    .await;
                let _ = self
                    .publish(&request.turn_id, OutboundSlot::Final, render.final_message())
                    .await;
                let _ = self.persistence.fail_turn(&request.turn_id, &error_text).await;
                return Err(err);
            }
        };

        render.finalize();
        self.registry
            .replace_runtime_state(
                &session.session_key,
                render.runtime_session_ref.clone(),
                render.final_assistant_message(),
            )
            .await?;
        self.persistence
            .update_runtime_state(
                &runtime.runtime_id,
                render.runtime_session_ref.as_deref(),
                render.final_assistant_message().as_deref(),
            )
            .await?;
        self.persistence
            .complete_turn(&request.turn_id, render.final_assistant_message().as_deref())
            .await?;

        self.publish(&request.turn_id, OutboundSlot::Progress, render.progress_message())
            .await?;
        self.publish(&request.turn_id, OutboundSlot::Final, render.final_message())
            .await?;

        if let Some(stderr_text) = completion.stderr_summary.as_ref() {
            info!("agent runtime stderr summary: {}", shorten(stderr_text, 400));
        }

        Ok(())
    }

    async fn build_prompt(&self, session: &SessionInfo, user_text: &str) -> String {
        if session.runtime_session_ref.is_none() {
            if let Some(parent_id) = session.parent_session_id.as_ref() {
                if let Some(parent) = self.registry.get_by_session_id(parent_id).await {
                    if let Some(parent_last) = parent.last_assistant_message {
                        return format!(
                            "Parent session summary:\n{}\n\nUser request:\n{}",
                            shorten(&parent_last, 1200),
                            user_text,
                        );
                    }
                }
            }
        }
        user_text.to_string()
    }

    async fn publish(&self, turn_id: &str, slot: OutboundSlot, message: OutboundMessage) -> Result<()> {
        self.sink
            .publish(&CoreOutboundEvent {
                turn_id: turn_id.to_string(),
                slot,
                message,
            })
            .await
    }

    async fn ensure_active_runtime(&self, session: &SessionInfo) -> Result<RuntimeInstance> {
        if let Some(runtime) = self.persistence.get_active_runtime(&session.session_key).await? {
            return Ok(runtime);
        }

        let default_workspace = self.config.codex_workdir.to_string_lossy().to_string();
        let workspace = self.resolve_workspace_path(Some(&default_workspace))?;
        let agent_kind = self.config.acp_adapter.clone();
        let runtime = self
            .persistence
            .create_runtime(
                &session.session_key,
                &default_runtime_label(&agent_kind, &workspace),
                &agent_kind,
                &workspace,
                true,
            )
            .await?;
        self.activate_runtime(session, &runtime).await?;
        Ok(runtime)
    }

    async fn activate_runtime(&self, session: &SessionInfo, runtime: &RuntimeInstance) -> Result<()> {
        self.persistence
            .set_active_runtime(&session.session_key, &runtime.runtime_id)
            .await?;
        self.registry
            .replace_runtime_state(
                &session.session_key,
                runtime.runtime_session_ref.clone(),
                runtime.last_assistant_message.clone(),
            )
            .await?;
        Ok(())
    }

    fn resolve_workspace_path(&self, candidate: Option<&str>) -> Result<String> {
        let default_workspace = self.config.codex_workdir.to_string_lossy().to_string();
        let raw = candidate
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(default_workspace.as_str());
        let path = std::path::PathBuf::from(raw);
        let canonical = path
            .canonicalize()
            .with_context(|| format!("workspace does not exist: {}", raw))?;
        if !canonical.is_dir() {
            anyhow::bail!("workspace is not a directory: {}", canonical.display());
        }
        Ok(canonical.to_string_lossy().to_string())
    }

    fn resolve_runtime_load_workspace(
        &self,
        workspace_path: Option<&str>,
        active: &RuntimeInstance,
    ) -> Result<String> {
        match workspace_path {
            Some(value) => self.resolve_workspace_path(Some(value)),
            None => self.resolve_workspace_path(Some(&active.workspace_path)),
        }
    }

    async fn load_claude_runtimes(&self, session_key: &str, workspace_path: &str) -> Result<usize> {
        let workspace = PathBuf::from(workspace_path);
        let discovered = self.discover_claude_sessions(&workspace).await?;
        let mut imported = 0usize;

        for session in discovered {
            self.persistence
                .import_runtime(
                    session_key,
                    &imported_runtime_label(&session),
                    "claude_code",
                    &session.workspace_path,
                    &session.runtime_session_ref,
                    session.git_branch.as_deref(),
                    session.first_prompt.as_deref(),
                    false,
                )
                .await?;
            imported += 1;
        }

        Ok(imported)
    }

    async fn discover_claude_sessions(
        &self,
        workspace_path: &Path,
    ) -> Result<Vec<ImportedClaudeSession>> {
        let claude_home = self.config.claude_home_dir.clone();
        let workspace_path = workspace_path.to_path_buf();
        tokio::task::spawn_blocking(move || discover_claude_sessions(&claude_home, &workspace_path))
            .await
            .context("join claude session discovery task failed")?
    }
}

fn default_runtime_label(agent_kind: &str, workspace_path: &str) -> String {
    let leaf = std::path::Path::new(workspace_path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("workspace");
    format!("{}-{}", agent_kind, leaf)
}

fn runtime_summary(runtime: &RuntimeInstance) -> RuntimeSummary {
    RuntimeSummary {
        runtime_id: runtime.runtime_id.clone(),
        label: runtime.label.clone(),
        agent_kind: runtime.agent_kind.clone(),
        workspace_path: runtime.workspace_path.clone(),
        runtime_session_ref: runtime.runtime_session_ref.clone(),
        tag: runtime.tag.clone(),
        prompt_preview: runtime.prompt_preview.clone(),
        has_runtime_session_ref: runtime.runtime_session_ref.is_some(),
        is_active: runtime.is_active,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportedClaudeSession {
    runtime_session_ref: String,
    workspace_path: String,
    first_prompt: Option<String>,
    git_branch: Option<String>,
    modified_at: i64,
}

fn discover_claude_sessions(
    claude_home: &Path,
    workspace_path: &Path,
) -> Result<Vec<ImportedClaudeSession>> {
    let mut dirs = Vec::new();
    let mut seen_dirs = HashSet::new();

    for candidate in claude_project_dirs(claude_home, workspace_path) {
        if seen_dirs.insert(candidate.clone()) && candidate.exists() {
            dirs.push(candidate);
        }
    }

    let mut sessions = Vec::new();
    let mut seen_sessions = HashSet::new();
    for dir in dirs {
        let discovered = {
            let from_index = load_sessions_from_index(&dir)?;
            if from_index.is_empty() {
                load_sessions_from_jsonl_dir(&dir)?
            } else {
                from_index
            }
        };

        for session in discovered {
            if seen_sessions.insert(session.runtime_session_ref.clone()) {
                sessions.push(session);
            }
        }
    }

    sessions.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.runtime_session_ref.cmp(&b.runtime_session_ref))
    });
    Ok(sessions)
}

fn claude_project_dirs(claude_home: &Path, workspace_path: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();

    let mut candidates = vec![workspace_path.to_path_buf()];
    if let Ok(canonical) = workspace_path.canonicalize() {
        candidates.push(canonical);
    }

    let mut aliased = Vec::new();
    for candidate in candidates {
        aliased.push(candidate.clone());
        if let Some(stripped) = strip_private_prefix(&candidate) {
            aliased.push(stripped);
        }
        if let Some(prefixed) = add_private_prefix(&candidate) {
            aliased.push(prefixed);
        }
    }

    for candidate in aliased {
        let dir = claude_home
            .join("projects")
            .join(claude_project_dir_name(&candidate));
        if seen.insert(dir.clone()) {
            dirs.push(dir);
        }
    }

    dirs
}

fn claude_project_dir_name(workspace_path: &Path) -> String {
    workspace_path
        .to_string_lossy()
        .replace(['/', '\\'], "-")
}

fn strip_private_prefix(path: &Path) -> Option<PathBuf> {
    let raw = path.to_string_lossy();
    raw.strip_prefix("/private/")
        .map(|value| PathBuf::from(format!("/{}", value)))
}

fn add_private_prefix(path: &Path) -> Option<PathBuf> {
    let raw = path.to_string_lossy();
    if raw.starts_with("/private/") {
        None
    } else if raw.starts_with("/tmp/") {
        Some(PathBuf::from(format!("/private{}", raw)))
    } else {
        None
    }
}

fn load_sessions_from_index(dir: &Path) -> Result<Vec<ImportedClaudeSession>> {
    let index_path = dir.join("sessions-index.json");
    if !index_path.exists() {
        return Ok(Vec::new());
    }

    let value: Value = serde_json::from_reader(
        File::open(&index_path).with_context(|| format!("open claude index failed: {:?}", index_path))?,
    )
    .with_context(|| format!("parse claude index failed: {:?}", index_path))?;
    let mut sessions = Vec::new();
    for entry in value
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(session_id) = entry.get("sessionId").and_then(Value::as_str) else {
            continue;
        };
        let workspace_path = entry
            .get("projectPath")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let modified_at = entry
            .get("fileMtime")
            .and_then(Value::as_i64)
            .map(|value| value / 1000)
            .unwrap_or(0);
        sessions.push(ImportedClaudeSession {
            runtime_session_ref: session_id.to_string(),
            workspace_path,
            first_prompt: entry
                .get("firstPrompt")
                .and_then(Value::as_str)
                .map(str::to_string),
            git_branch: entry
                .get("gitBranch")
                .and_then(Value::as_str)
                .map(str::to_string),
            modified_at,
        });
    }
    Ok(sessions)
}

fn load_sessions_from_jsonl_dir(dir: &Path) -> Result<Vec<ImportedClaudeSession>> {
    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read claude dir failed: {:?}", dir))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        if let Some(session) = load_session_from_jsonl(&path)? {
            sessions.push(session);
        }
    }
    Ok(sessions)
}

fn load_session_from_jsonl(path: &Path) -> Result<Option<ImportedClaudeSession>> {
    let file = File::open(path).with_context(|| format!("open claude session failed: {:?}", path))?;
    let reader = BufReader::new(file);
    let modified_at = std::fs::metadata(path)
        .ok()
        .and_then(|value| value.modified().ok())
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0);

    let mut session_id = None;
    let mut workspace_path = None;
    let mut first_prompt = None;
    let mut git_branch = None;

    for line in reader.lines().take(48) {
        let line = line?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if session_id.is_none() {
            session_id = value.get("sessionId").and_then(Value::as_str).map(str::to_string);
        }
        if workspace_path.is_none() {
            workspace_path = value.get("cwd").and_then(Value::as_str).map(str::to_string);
        }
        if git_branch.is_none() {
            git_branch = value.get("gitBranch").and_then(Value::as_str).map(str::to_string);
        }
        if first_prompt.is_none() && value.get("type").and_then(Value::as_str) == Some("user") {
            first_prompt = extract_first_prompt(&value);
        }
        if session_id.is_some() && workspace_path.is_some() && first_prompt.is_some() {
            break;
        }
    }

    let Some(runtime_session_ref) = session_id.or_else(|| {
        path.file_stem()
            .and_then(|value| value.to_str())
            .map(str::to_string)
    }) else {
        return Ok(None);
    };

    Ok(Some(ImportedClaudeSession {
        runtime_session_ref,
        workspace_path: workspace_path.unwrap_or_default(),
        first_prompt,
        git_branch,
        modified_at,
    }))
}

fn extract_first_prompt(value: &Value) -> Option<String> {
    value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find_map(|item| {
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    item.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
        })
}

fn imported_runtime_label(session: &ImportedClaudeSession) -> String {
    format!("claude-{}", shorten(&session.runtime_session_ref, 8))
}

fn runtime_matches_selector(runtime: &RuntimeInstance, selector: &str) -> bool {
    runtime.runtime_id == selector
        || runtime.label == selector
        || runtime.runtime_id.starts_with(selector)
        || runtime
            .runtime_session_ref
            .as_deref()
            .map(|value| value.starts_with(selector))
            .unwrap_or(false)
}

struct TurnRenderState {
    session_id: String,
    session_key: String,
    status: &'static str,
    progress_excerpt: String,
    final_text: Option<String>,
    runtime_session_ref: Option<String>,
    usage: Option<Value>,
    error: Option<String>,
    todo_items: Vec<TodoEntry>,
    active_tool_count: usize,
    tool_event_icon: &'static str,
    tool_event_note: String,
    last_render_at: Instant,
}

impl TurnRenderState {
    fn new(session: &SessionInfo) -> Self {
        Self {
            session_id: session.session_id.clone(),
            session_key: session.session_key.clone(),
            status: "running",
            progress_excerpt: String::new(),
            final_text: None,
            runtime_session_ref: session.runtime_session_ref.clone(),
            usage: None,
            error: None,
            todo_items: Vec::new(),
            active_tool_count: 0,
            tool_event_icon: "🫥",
            tool_event_note: "等待 runtime 返回首个状态".to_string(),
            last_render_at: Instant::now(),
        }
    }

    fn apply_event(&mut self, event: NormalizedAgentEvent) -> bool {
        let prev_todos = self.todo_items.clone();
        match event {
            NormalizedAgentEvent::TurnStarted => {
                self.status = "running";
                self.tool_event_icon = "🔄";
                self.tool_event_note = "已经开始处理本轮请求".to_string();
            }
            NormalizedAgentEvent::TurnCompleted => {
                self.status = "completed";
                self.tool_event_icon = "✅";
                self.tool_event_note = "本轮执行已收尾".to_string();
            }
            NormalizedAgentEvent::RuntimeSessionReady(runtime_session_ref) => {
                self.runtime_session_ref = Some(runtime_session_ref);
                self.tool_event_icon = "🧵";
                self.tool_event_note = "runtime 会话已建立，可继续追踪".to_string();
            }
            NormalizedAgentEvent::AssistantChunk(text) => {
                self.progress_excerpt.push_str(&text);
            }
            NormalizedAgentEvent::AssistantMessage(text) => {
                self.final_text = Some(text);
                self.tool_event_icon = "💬";
                self.tool_event_note = "最终结果已经生成，详情见绿色结果卡片".to_string();
            }
            NormalizedAgentEvent::ToolState { state, .. } => match state {
                crate::agent::normalized::AgentToolState::Pending
                | crate::agent::normalized::AgentToolState::InProgress => {
                    self.active_tool_count = self.active_tool_count.saturating_add(1);
                    self.tool_event_icon = "🛠️";
                    self.tool_event_note = format!("正在执行工具调用，当前活跃 {} 个", self.active_tool_count);
                }
                crate::agent::normalized::AgentToolState::Completed
                | crate::agent::normalized::AgentToolState::Failed => {
                    self.active_tool_count = self.active_tool_count.saturating_sub(1);
                    self.tool_event_icon = if self.active_tool_count == 0 { "📬" } else { "🛠️" };
                    self.tool_event_note = if self.active_tool_count == 0 {
                        "最近一轮工具执行已结束，等待新的输出".to_string()
                    } else {
                        format!("仍有 {} 个工具调用在运行", self.active_tool_count)
                    };
                }
            },
            NormalizedAgentEvent::PlanUpdated(todos) => {
                self.todo_items = todos;
                self.tool_event_icon = "🧭";
                self.tool_event_note = format!("计划已刷新，共 {} 项", self.todo_items.len());
            }
            NormalizedAgentEvent::Usage(usage) => {
                self.usage = Some(usage);
            }
        }
        self.todo_items != prev_todos
    }

    fn finalize(&mut self) {
        if self.status != "failed" {
            self.status = "completed";
        }
        if self.final_text.is_none() && !self.progress_excerpt.trim().is_empty() {
            self.final_text = Some(self.progress_excerpt.clone());
        }
    }

    fn fail(&mut self, error_text: String) {
        self.status = "failed";
        self.error = Some(error_text);
        self.tool_event_icon = "⚠️";
        self.tool_event_note = "执行过程中发生异常".to_string();
    }

    fn should_flush(&self, min_update_ms: u64) -> bool {
        self.last_render_at.elapsed() >= Duration::from_millis(min_update_ms)
    }

    fn mark_flushed(&mut self) {
        self.last_render_at = Instant::now();
    }

    fn final_assistant_message(&self) -> Option<String> {
        self.final_text.clone()
    }

    fn progress_message(&self) -> OutboundMessage {
        let mut blocks = vec![
            CardBlock::Markdown {
                text: format!(
                    "**{} {}**\n`session {}`",
                    progress_status_icon(self.status),
                    progress_status_label(self.status),
                    self.session_id
                ),
            },
            CardBlock::Markdown {
                text: format!(
                    "{} {}\n{} 当前活跃工具数：`{}`",
                    self.tool_event_icon,
                    self.tool_event_note,
                    progress_status_icon(self.status),
                    self.active_tool_count
                ),
            },
        ];

        if let Some(runtime_session_ref) = self.runtime_session_ref.as_ref() {
            blocks.push(CardBlock::Markdown {
                text: format!("🧵 已建立 runtime session：`{}`", shorten(runtime_session_ref, 48)),
            });
        }

        if self.status == "running" && !self.progress_excerpt.trim().is_empty() {
            blocks.push(CardBlock::Divider);
            blocks.push(CardBlock::Markdown {
                text: format!("📌 **最近输出摘录**\n\n{}", format_output_excerpt(&self.progress_excerpt, 700)),
            });
        } else if self.status == "completed" && self.error.is_none() {
            blocks.push(CardBlock::Divider);
            blocks.push(CardBlock::Markdown {
                text: "✅ 最终结果已经单独整理到绿色卡片，这张灰卡不再重复正文。".to_string(),
            });
        }

        if let Some(usage) = self.usage.as_ref() {
            blocks.push(CardBlock::Divider);
            blocks.push(CardBlock::Markdown {
                text: format!("📊 **本轮资源消耗**\n{}", summarize_usage(usage)),
            });
        }

        if let Some(error_text) = self.error.as_ref() {
            blocks.push(CardBlock::Divider);
            blocks.push(CardBlock::Markdown {
                text: format!("⚠️ **异常信息**\n{}", shorten(error_text, 800)),
            });
        }

        blocks.push(CardBlock::Divider);
        blocks.push(CardBlock::Markdown {
            text: "💡 这张卡片会持续更新，用来表示 agent 仍在运行；最终答案会单独发一张绿色结果卡片。".to_string(),
        });

        card_message(
            "Codex 持续运行中",
            if self.status == "failed" { CardTheme::Red } else { CardTheme::Grey },
            true,
            blocks,
        )
    }

    fn todo_message(&self) -> OutboundMessage {
        if self.todo_items.is_empty() {
            return text_message("🧭 暂时还没有可展示的计划更新。");
        }

        let ordered = ordered_todos(&self.todo_items);
        let text = ordered
            .iter()
            .enumerate()
            .map(|(index, item)| {
                format!(
                    "{}. {} {}",
                    index + 1,
                    todo_status_symbol(&item.status),
                    shorten(&item.content, 160),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        card_message(
            "执行计划",
            CardTheme::Orange,
            true,
            vec![
                CardBlock::Markdown {
                    text: format!("🧭 **计划进度**  {}\n\n{}", summarize_todos(&ordered), text),
                },
                CardBlock::Markdown {
                    text: "💡 这张卡片只在计划发生变化时更新。".to_string(),
                },
            ],
        )
    }

    fn final_message(&self) -> OutboundMessage {
        let text = if let Some(error_text) = self.error.as_ref() {
            format!("⚠️ 本轮执行失败。\n\n{}", shorten(error_text, 1200))
        } else {
            self.final_text
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "暂未拿到可展示的最终结果。".to_string())
        };
        let summary = summarize_final_text(&text);

        card_message(
            "最终结果",
            if self.status == "failed" {
                CardTheme::Red
            } else {
                CardTheme::Green
            },
            false,
            vec![
                CardBlock::Markdown {
                    text: format!("✅ **本轮已结束**\n`session {}`", self.session_id),
                },
                CardBlock::Markdown {
                    text: format!("📌 **结论摘要**\n{}", summary),
                },
                CardBlock::Divider,
                CardBlock::Markdown {
                    text: format!("📝 **最终输出**\n\n{}", shorten(&text, 2200)),
                },
                CardBlock::Divider,
                CardBlock::Markdown {
                    text: format!("🔖 `session_key={}`", shorten(&self.session_key, 120)),
                },
            ],
        )
    }
}

fn todo_status_symbol(status: &str) -> &'static str {
    match status {
        "completed" | "done" => "✅",
        "in_progress" | "running" => "🔄",
        "failed" | "error" | "blocked" => "⛔",
        _ => "⏳",
    }
}

fn ordered_todos(items: &[TodoEntry]) -> Vec<TodoEntry> {
    let mut todos = items.to_vec();
    todos.sort_by_key(|item| todo_sort_key(&item.status));
    todos
}

fn todo_sort_key(status: &str) -> u8 {
    match status {
        "in_progress" | "running" => 0,
        "pending" | "todo" => 1,
        "completed" | "done" => 2,
        "failed" | "error" | "blocked" => 3,
        _ => 4,
    }
}

fn summarize_todos(items: &[TodoEntry]) -> String {
    let mut done = 0;
    let mut doing = 0;
    let mut waiting = 0;
    let mut failed = 0;
    for item in items {
        match item.status.as_str() {
            "completed" | "done" => done += 1,
            "in_progress" | "running" => doing += 1,
            "failed" | "error" | "blocked" => failed += 1,
            _ => waiting += 1,
        }
    }
    format!("✅ {}  🔄 {}  ⏳ {}  ⛔ {}", done, doing, waiting, failed)
}

fn progress_status_icon(status: &str) -> &'static str {
    match status {
        "completed" => "✅",
        "failed" => "⚠️",
        _ => "🔄",
    }
}

fn progress_status_label(status: &str) -> &'static str {
    match status {
        "completed" => "已完成",
        "failed" => "出现异常",
        _ => "正在运行",
    }
}

fn format_output_excerpt(text: &str, max_chars: usize) -> String {
    let compact = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    shorten(&compact, max_chars)
}

fn summarize_usage(usage: &Value) -> String {
    if let Some(obj) = usage.as_object() {
        let mut lines = Vec::new();
        for key in ["input_tokens", "output_tokens", "total_tokens"] {
            if let Some(value) = obj.get(key) {
                lines.push(format!("• `{}`: {}", key, value));
            }
        }
        if !lines.is_empty() {
            return lines.join("\n");
        }
    }
    format!("```json\n{}\n```", usage)
}

fn summarize_final_text(text: &str) -> String {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .map(|line| format!("• {}", shorten(line, 120)))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        "• 本轮没有提取到可概括的结论".to_string()
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use std::{path::{Path, PathBuf}, sync::Arc};

    use anyhow::Result;
    use async_trait::async_trait;
    use tokio::sync::{mpsc, Mutex};

    use super::*;
    use crate::{
        agent::runtime::{AgentRuntime, RuntimeCompletion, RuntimeEvent, RuntimeTurn},
        config::Config,
    };

    #[derive(Clone, Default)]
    struct MockSink {
        events: Arc<Mutex<Vec<CoreOutboundEvent>>>,
    }

    #[async_trait]
    impl TurnEventSink for MockSink {
        async fn publish(&self, event: &CoreOutboundEvent) -> Result<()> {
            self.events.lock().await.push(event.clone());
            Ok(())
        }
    }

    #[derive(Clone)]
    struct MockRuntime {
        captured: Arc<Mutex<Vec<RuntimeTurnRequest>>>,
        events: Vec<RuntimeEvent>,
    }

    #[async_trait]
    impl AgentRuntime for MockRuntime {
        async fn start_turn(&self, request: RuntimeTurnRequest) -> Result<RuntimeTurn> {
            self.captured.lock().await.push(request);
            let (tx, rx) = mpsc::unbounded_channel();
            let events = self.events.clone();
            tokio::spawn(async move {
                for event in events {
                    let _ = tx.send(event);
                }
            });
            let completion = tokio::spawn(async { Ok(RuntimeCompletion::default()) });
            Ok(RuntimeTurn { events: rx, completion })
        }

        fn name(&self) -> &'static str {
            "mock"
        }
    }

    fn test_config() -> Arc<Config> {
        test_config_with_paths(
            PathBuf::from("."),
            PathBuf::from(format!("/tmp/remoteagent-claude-{}", uuid::Uuid::new_v4())),
        )
    }

    fn test_config_with_paths(codex_workdir: PathBuf, claude_home_dir: PathBuf) -> Arc<Config> {
        Arc::new(Config {
            core_bind: "127.0.0.1:39001".parse().unwrap(),
            core_ingest_token: None,
            gateway_event_url: "http://127.0.0.1:39000/internal/gateway/event".to_string(),
            gateway_event_token: None,
            state_db_path: PathBuf::from(format!("/tmp/remoteagent-service-{}.db", uuid::Uuid::new_v4())),
            claude_home_dir,
            codex_bin: "codex".to_string(),
            codex_workdir,
            codex_model: None,
            codex_skip_git_repo_check: true,
            runtime_mode: "exec_json".to_string(),
            acp_adapter: "codex".to_string(),
            acp_agent_cmd: None,
            render_min_update_ms: 0,
            todo_event_log_path: PathBuf::from(format!("/tmp/remoteagent-todo-{}.jsonl", uuid::Uuid::new_v4())),
        })
    }

    async fn build_service(runtime: Arc<dyn AgentRuntime>, sink: Arc<dyn TurnEventSink>) -> CoreService {
        let config = test_config();
        build_service_with_config(runtime, sink, config).await
    }

    async fn build_service_with_config(
        runtime: Arc<dyn AgentRuntime>,
        sink: Arc<dyn TurnEventSink>,
        config: Arc<Config>,
    ) -> CoreService {
        let persistence = Persistence::new(config.state_db_path.clone());
        persistence.init().await.unwrap();
        let registry = SessionRegistry::new(persistence.clone()).await.unwrap();
        CoreService::new(config, runtime, sink, persistence, registry)
    }

    #[tokio::test]
    async fn run_turn_publishes_progress_todo_and_final_events() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: vec![
                RuntimeEvent::Agent(NormalizedAgentEvent::RuntimeSessionReady("thread_123".to_string())),
                RuntimeEvent::Agent(NormalizedAgentEvent::PlanUpdated(vec![TodoEntry {
                    content: "Inspect repository".to_string(),
                    status: "in_progress".to_string(),
                }])),
                RuntimeEvent::Agent(NormalizedAgentEvent::AssistantMessage("Final answer".to_string())),
                RuntimeEvent::Agent(NormalizedAgentEvent::TurnCompleted),
            ],
        });
        let service = build_service(runtime, sink.clone()).await;
        let session = service.registry.resolve("gateway:test", None).await.unwrap();
        let runtime = service.ensure_active_runtime(&session).await.unwrap();
        service
            .persistence
            .create_turn("turn_1", &session.session_id, "hello")
            .await
            .unwrap();

        service
            .run_turn(
                CoreTurnRequest {
                    turn_id: "turn_1".to_string(),
                    session_key: "gateway:test".to_string(),
                    parent_session_key: None,
                    text: "hello".to_string(),
                },
                session.clone(),
                runtime,
            )
            .await
            .unwrap();

        let events = sink.events.lock().await.clone();
        assert!(events.iter().any(|event| event.slot == OutboundSlot::Progress));
        assert!(events.iter().any(|event| event.slot == OutboundSlot::Todo));
        assert!(events.iter().any(|event| event.slot == OutboundSlot::Final));

        let stored = service.persistence.get_turn("turn_1").await.unwrap().unwrap();
        assert_eq!(stored.status, "completed");
        assert_eq!(stored.final_text.as_deref(), Some("Final answer"));

        let updated = service
            .registry
            .get_by_session_key("gateway:test")
            .await
            .unwrap();
        assert_eq!(updated.runtime_session_ref.as_deref(), Some("thread_123"));
        assert_eq!(updated.last_assistant_message.as_deref(), Some("Final answer"));
    }

    #[tokio::test]
    async fn build_prompt_uses_parent_summary_for_new_child_session() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: captured.clone(),
            events: vec![
                RuntimeEvent::Agent(NormalizedAgentEvent::AssistantMessage("child reply".to_string())),
                RuntimeEvent::Agent(NormalizedAgentEvent::TurnCompleted),
            ],
        });
        let service = build_service(runtime, sink).await;

        let parent = service.registry.resolve("session:parent", None).await.unwrap();
        service
            .registry
            .update_runtime_state(
                &parent.session_id,
                Some("runtime_parent".to_string()),
                Some("Parent result summary".to_string()),
            )
            .await
            .unwrap();

        let child = service
            .registry
            .resolve("session:child", Some("session:parent"))
            .await
            .unwrap();
        let runtime = service.ensure_active_runtime(&child).await.unwrap();
        service
            .persistence
            .create_turn("turn_2", &child.session_id, "follow up")
            .await
            .unwrap();

        service
            .run_turn(
                CoreTurnRequest {
                    turn_id: "turn_2".to_string(),
                    session_key: "session:child".to_string(),
                    parent_session_key: Some("session:parent".to_string()),
                    text: "follow up".to_string(),
                },
                child,
                runtime,
            )
            .await
            .unwrap();

        let requests = captured.lock().await;
        assert!(requests[0].prompt.contains("Parent result summary"));
        assert!(requests[0].prompt.contains("follow up"));
    }

    #[test]
    fn card_design_uses_requested_icons_and_colors() {
        let session = SessionInfo {
            session_id: "sess_demo".to_string(),
            session_key: "gateway:test".to_string(),
            parent_session_id: None,
            runtime_session_ref: Some("thread_1".to_string()),
            last_assistant_message: None,
        };
        let mut render = TurnRenderState::new(&session);
        render.todo_items = vec![
            TodoEntry {
                content: "Update todo card".to_string(),
                status: "in_progress".to_string(),
            },
            TodoEntry {
                content: "Ship final answer".to_string(),
                status: "completed".to_string(),
            },
        ];
        render.progress_excerpt = "first line\nsecond line".to_string();
        render.final_text = Some("line one\nline two\nline three".to_string());

        let progress = render.progress_message();
        let todo = render.todo_message();
        let final_message = render.final_message();

        match progress {
            OutboundMessage::Card { card } => {
                assert_eq!(card.theme, CardTheme::Grey);
                let body = format!("{:?}", card.blocks);
                assert!(body.contains("🔄"));
                assert!(body.contains("🧵"));
            }
            _ => panic!("progress should be card"),
        }

        match todo {
            OutboundMessage::Card { card } => {
                assert_eq!(card.theme, CardTheme::Orange);
                let body = format!("{:?}", card.blocks);
                assert!(body.contains("✅"));
                assert!(body.contains("🔄"));
                assert!(!body.contains("done"));
                assert!(!body.contains("pending"));
            }
            _ => panic!("todo should be card"),
        }

        match final_message {
            OutboundMessage::Card { card } => {
                assert_eq!(card.theme, CardTheme::Green);
                let body = format!("{:?}", card.blocks);
                assert!(body.contains("✅"));
                assert!(body.contains("📌"));
                assert!(body.contains("📝"));
            }
            _ => panic!("final should be card"),
        }
    }

    #[test]
    fn completed_progress_card_does_not_repeat_final_text() {
        let session = SessionInfo {
            session_id: "sess_demo".to_string(),
            session_key: "gateway:test".to_string(),
            parent_session_id: None,
            runtime_session_ref: None,
            last_assistant_message: None,
        };
        let mut render = TurnRenderState::new(&session);
        render.status = "completed";
        render.progress_excerpt = "intermediate snippet".to_string();
        render.final_text = Some("final answer body".to_string());

        match render.progress_message() {
            OutboundMessage::Card { card } => {
                let body = format!("{:?}", card.blocks);
                assert!(!body.contains("final answer body"));
                assert!(body.contains("绿色结果卡片"));
            }
            _ => panic!("progress should be card"),
        }
    }

    #[tokio::test]
    async fn control_can_create_and_switch_runtimes() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
        });
        let service = build_service(runtime, sink).await;

        let show = service
            .handle_control(CoreControlRequest {
                session_key: "control:test".to_string(),
                parent_session_key: None,
                action: ControlAction::ShowRuntime,
                runtime_selector: None,
                workspace_path: None,
                label: None,
                agent_kind: None,
            })
            .await
            .unwrap();
        assert!(show.active_runtime.is_some());

        let current_dir = std::env::current_dir().unwrap();
        let created = service
            .handle_control(CoreControlRequest {
                session_key: "control:test".to_string(),
                parent_session_key: None,
                action: ControlAction::CreateRuntime,
                runtime_selector: None,
                workspace_path: Some(current_dir.to_string_lossy().to_string()),
                label: Some("claude-alt".to_string()),
                agent_kind: Some("claude_code".to_string()),
            })
            .await
            .unwrap();
        assert_eq!(created.active_runtime.as_ref().unwrap().label, "claude-alt");

        let switched = service
            .handle_control(CoreControlRequest {
                session_key: "control:test".to_string(),
                parent_session_key: None,
                action: ControlAction::SwitchRuntime,
                runtime_selector: Some("claude-alt".to_string()),
                workspace_path: None,
                label: None,
                agent_kind: None,
            })
            .await
            .unwrap();
        assert_eq!(switched.active_runtime.as_ref().unwrap().label, "claude-alt");
        assert!(switched.runtimes.iter().any(|runtime| runtime.label == "claude-alt"));
    }

    #[tokio::test]
    async fn switch_runtime_accepts_short_runtime_prefix() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
        });
        let service = build_service(runtime, sink).await;
        let current_dir = std::env::current_dir().unwrap();
        let created = service
            .handle_control(CoreControlRequest {
                session_key: "control:short-switch".to_string(),
                parent_session_key: None,
                action: ControlAction::CreateRuntime,
                runtime_selector: None,
                workspace_path: Some(current_dir.to_string_lossy().to_string()),
                label: Some("claude-short".to_string()),
                agent_kind: Some("claude_code".to_string()),
            })
            .await
            .unwrap();

        let short_id = created
            .active_runtime
            .as_ref()
            .unwrap()
            .runtime_id
            .chars()
            .take(8)
            .collect::<String>();

        let switched = service
            .handle_control(CoreControlRequest {
                session_key: "control:short-switch".to_string(),
                parent_session_key: None,
                action: ControlAction::SwitchRuntime,
                runtime_selector: Some(short_id),
                workspace_path: None,
                label: None,
                agent_kind: None,
            })
            .await
            .unwrap();

        assert_eq!(switched.active_runtime.as_ref().unwrap().label, "claude-short");
    }

    #[tokio::test]
    async fn control_can_load_claude_runtimes_from_workspace_jsonl() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
        });
        let root = PathBuf::from(format!("/tmp/remoteagent-load-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        let claude_home = root.join(".claude");
        std::fs::create_dir_all(&workspace).unwrap();

        let project_dir = claude_home
            .join("projects")
            .join(claude_project_dir_name(&workspace));
        std::fs::create_dir_all(&project_dir).unwrap();
        write_claude_jsonl(
            &project_dir.join("sess-a.jsonl"),
            "sess-a",
            &workspace,
            "master",
            "first task",
        );
        write_claude_jsonl(
            &project_dir.join("sess-b.jsonl"),
            "sess-b",
            &workspace,
            "feature/demo",
            "second task",
        );

        let config = test_config_with_paths(workspace.clone(), claude_home);
        let service = build_service_with_config(runtime, sink, config).await;
        let discovered = service.discover_claude_sessions(&workspace).await.unwrap();
        assert_eq!(discovered.len(), 2);
        let response = service
            .handle_control(CoreControlRequest {
                session_key: "control:load".to_string(),
                parent_session_key: None,
                action: ControlAction::LoadRuntimes,
                runtime_selector: None,
                workspace_path: Some(workspace.to_string_lossy().to_string()),
                label: None,
                agent_kind: None,
            })
            .await
            .unwrap();

        assert!(response.ok);
        assert!(response.runtimes.iter().filter(|runtime| runtime.has_runtime_session_ref).count() >= 2);
        assert!(response
            .runtimes
            .iter()
            .any(|runtime| runtime.runtime_session_ref.as_deref() == Some("sess-a")));
        assert!(response
            .runtimes
            .iter()
            .any(|runtime| runtime.runtime_session_ref.as_deref() == Some("sess-b")));
    }

    #[test]
    fn claude_session_discovery_falls_back_to_jsonl_without_index() {
        let root = PathBuf::from(format!("/tmp/remoteagent-discovery-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        let claude_home = root.join(".claude");
        std::fs::create_dir_all(&workspace).unwrap();
        let project_dir = claude_home
            .join("projects")
            .join(claude_project_dir_name(&workspace));
        std::fs::create_dir_all(&project_dir).unwrap();

        write_claude_jsonl(
            &project_dir.join("sess-c.jsonl"),
            "sess-c",
            &workspace,
            "master",
            "scan repository state",
        );

        let sessions = discover_claude_sessions(&claude_home, &workspace).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].runtime_session_ref, "sess-c");
        assert_eq!(sessions[0].workspace_path, workspace.to_string_lossy().to_string());
        assert_eq!(sessions[0].git_branch.as_deref(), Some("master"));
    }

    fn write_claude_jsonl(
        path: &Path,
        session_id: &str,
        workspace: &Path,
        git_branch: &str,
        prompt: &str,
    ) {
        let content = format!(
            "{{\"type\":\"queue-operation\",\"operation\":\"dequeue\",\"timestamp\":\"2026-03-06T06:55:02.246Z\",\"sessionId\":\"{session_id}\"}}\n\
{{\"parentUuid\":null,\"isSidechain\":false,\"userType\":\"external\",\"cwd\":\"{}\",\"sessionId\":\"{session_id}\",\"version\":\"2.1.44\",\"gitBranch\":\"{git_branch}\",\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"{prompt}\"}}]}},\"uuid\":\"u1\",\"timestamp\":\"2026-03-06T06:55:02.316Z\"}}\n",
            workspace.to_string_lossy(),
        );
        std::fs::write(path, content).unwrap();
    }
}
