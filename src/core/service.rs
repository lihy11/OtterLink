use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{
    agent::{
        normalized::NormalizedAgentEvent,
        runtime::{AgentRuntime, RuntimeEvent, RuntimeSteerRequest, RuntimeTurnRequest},
    },
    config::Config,
    core::{
        commands::{
            parse_inbound_message, render_control_response, render_invalid_runtime_command,
            render_runtime_help, ParsedInboundMessage,
        },
        inbound::{CoreInboundRequest, CoreInboundResponse},
        message_builder::{card_message, text_message},
        models::{CardBlock, CardTheme, OutboundMessage, TodoEntry},
        persistence::{Persistence, RuntimeInstance},
        persistence::RuntimeSelection,
        ports::TurnEventSink,
        registry::{SessionInfo, SessionRegistry},
        support::{append_jsonl, shorten},
    },
    protocol::{
        ControlAction, CoreControlRequest, CoreControlResponse, CoreOutboundEvent, CoreTurnAccepted,
        CoreTurnRequest, OutboundSlot, RuntimeSelectorSummary, RuntimeSummary,
    },
};

#[derive(Clone, Default)]
pub struct SessionLocks {
    locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

#[derive(Clone)]
struct ActiveTurnEntry {
    turn_id: String,
    agent_kind: Option<String>,
    runtime_session_ref: Option<String>,
    runtime_turn_ref: Option<String>,
    state: ActiveTurnState,
}

#[derive(Clone)]
struct ActiveTurnSnapshot {
    turn_id: String,
    agent_kind: Option<String>,
    runtime_session_ref: Option<String>,
    runtime_turn_ref: Option<String>,
}

#[derive(Clone)]
enum ActiveTurnState {
    Starting { stop_requested: bool },
    Running { cancel: crate::agent::runtime::RuntimeCancelHandle },
}

#[derive(Clone, Default)]
struct ActiveTurns {
    turns: Arc<Mutex<HashMap<String, ActiveTurnEntry>>>,
}

impl ActiveTurns {
    async fn set_starting(&self, session_key: &str, turn_id: &str) {
        let mut guard = self.turns.lock().await;
        guard.insert(
            session_key.to_string(),
            ActiveTurnEntry {
                turn_id: turn_id.to_string(),
                agent_kind: None,
                runtime_session_ref: None,
                runtime_turn_ref: None,
                state: ActiveTurnState::Starting {
                    stop_requested: false,
                },
            },
        );
    }

    async fn attach_cancel(
        &self,
        session_key: &str,
        turn_id: &str,
        agent_kind: Option<&str>,
        runtime_session_ref: Option<&str>,
        runtime_turn_ref: Option<&str>,
        cancel: crate::agent::runtime::RuntimeCancelHandle,
    ) {
        let mut guard = self.turns.lock().await;
        let Some(entry) = guard.get_mut(session_key) else {
            return;
        };
        if entry.turn_id != turn_id {
            return;
        }

        let stop_requested = matches!(
            entry.state,
            ActiveTurnState::Starting {
                stop_requested: true
            }
        );
        entry.agent_kind = agent_kind.map(str::to_string);
        entry.runtime_session_ref = runtime_session_ref.map(str::to_string);
        entry.runtime_turn_ref = runtime_turn_ref.map(str::to_string);
        entry.state = ActiveTurnState::Running {
            cancel: cancel.clone(),
        };
        if stop_requested {
            cancel.cancel();
        }
    }

    async fn get(&self, session_key: &str) -> Option<ActiveTurnSnapshot> {
        let guard = self.turns.lock().await;
        let entry = guard.get(session_key)?;
        Some(ActiveTurnSnapshot {
            turn_id: entry.turn_id.clone(),
            agent_kind: entry.agent_kind.clone(),
            runtime_session_ref: entry.runtime_session_ref.clone(),
            runtime_turn_ref: entry.runtime_turn_ref.clone(),
        })
    }

    async fn request_stop(&self, session_key: &str) -> Option<String> {
        let mut guard = self.turns.lock().await;
        let entry = guard.get_mut(session_key)?;
        match &mut entry.state {
            ActiveTurnState::Starting { stop_requested } => {
                *stop_requested = true;
            }
            ActiveTurnState::Running { cancel } => {
                cancel.cancel();
            }
        }
        Some(entry.turn_id.clone())
    }

    async fn clear(&self, session_key: &str, turn_id: &str) {
        let mut guard = self.turns.lock().await;
        let should_remove = guard
            .get(session_key)
            .map(|entry| entry.turn_id == turn_id)
            .unwrap_or(false);
        if should_remove {
            guard.remove(session_key);
        }
    }
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
    active_turns: ActiveTurns,
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
            active_turns: ActiveTurns::default(),
        }
    }

    pub async fn accept_turn(&self, request: CoreTurnRequest) -> Result<CoreTurnAccepted> {
        let session = self
            .registry
            .resolve(&request.session_key, request.parent_session_key.as_deref())
            .await
            .context("resolve session failed")?;
        let selection = self.ensure_runtime_selection(&session).await?;
        let selected_runtime_id = selection.selected_runtime_id.as_deref().ok_or_else(|| {
            anyhow!(missing_runtime_selection_message(&selection))
        })?;
        let runtime = self
            .persistence
            .get_runtime(selected_runtime_id)
            .await?
            .ok_or_else(|| anyhow!("selected runtime not found: {}", selected_runtime_id))?;

        self.persistence
            .create_turn(&request.turn_id, &session.session_id, &request.text)
            .await
            .context("create turn failed")?;

        let service = self.clone();
        let turn_id = request.turn_id.clone();
        let selection = selection.clone();
        tokio::spawn(async move {
            if let Err(err) = service.run_turn(request, session, selection, runtime).await {
                error!("turn failed: {err:?}");
            }
        });

        Ok(CoreTurnAccepted {
            ok: true,
            turn_id,
        })
    }

    pub async fn handle_inbound(&self, request: CoreInboundRequest) -> Result<CoreInboundResponse> {
        match parse_inbound_message(
            &request.text,
            request.session_key.clone(),
            request.parent_session_key.clone(),
        ) {
            ParsedInboundMessage::Help => Ok(CoreInboundResponse {
                turn_id: None,
                replies: vec![render_runtime_help()],
                react_to_message: false,
            }),
            ParsedInboundMessage::Invalid { message } => Ok(CoreInboundResponse {
                turn_id: None,
                replies: vec![render_invalid_runtime_command(&message)],
                react_to_message: false,
            }),
            ParsedInboundMessage::Control(control) => {
                match self.handle_control(control).await {
                    Ok(response) => Ok(CoreInboundResponse {
                        turn_id: None,
                        replies: render_control_response(&response),
                        react_to_message: false,
                    }),
                    Err(err) => Ok(CoreInboundResponse {
                        turn_id: None,
                        replies: vec![text_message(format!("Runtime 控制失败：{}", err))],
                        react_to_message: false,
                    }),
                }
            }
            ParsedInboundMessage::Turn => {
                if let Some(response) = self.try_steer_active_codex_turn(&request).await? {
                    return Ok(response);
                }
                let turn_id = format!("turn_{}", Uuid::new_v4().simple());
                let accepted = self
                    .accept_turn(CoreTurnRequest {
                        turn_id: turn_id.clone(),
                        session_key: request.session_key,
                        parent_session_key: request.parent_session_key,
                        text: request.text,
                    })
                    .await?;
                Ok(CoreInboundResponse {
                    turn_id: Some(accepted.turn_id),
                    replies: Vec::new(),
                    react_to_message: true,
                })
            }
        }
    }

    async fn try_steer_active_codex_turn(
        &self,
        request: &CoreInboundRequest,
    ) -> Result<Option<CoreInboundResponse>> {
        let Some(active_turn) = self.active_turns.get(&request.session_key).await else {
            return Ok(None);
        };
        if active_turn.agent_kind.as_deref() != Some("codex") {
            return Ok(None);
        }
        let Some(runtime_session_ref) = active_turn.runtime_session_ref.clone() else {
            return Ok(None);
        };
        let Some(runtime_turn_ref) = active_turn.runtime_turn_ref.clone() else {
            return Ok(None);
        };

        let session = self
            .registry
            .resolve(&request.session_key, request.parent_session_key.as_deref())
            .await
            .context("resolve session failed")?;
        let selector = self.ensure_runtime_selection(&session).await?;
        if selector.agent_kind != "codex" {
            return Ok(None);
        }

        self.runtime
            .steer_turn(RuntimeSteerRequest {
                session_key: request.session_key.clone(),
                prompt: request.text.clone(),
                runtime_session_ref,
                runtime_turn_ref,
                agent_kind: Some("codex".to_string()),
                workspace_path: Some(PathBuf::from(&selector.workspace_path)),
                proxy_mode: Some(selector.proxy_mode.clone()),
                proxy_url: selector
                    .proxy_url
                    .clone()
                    .or_else(|| self.config.acp_proxy_url.clone()),
            })
            .await?;

        Ok(Some(CoreInboundResponse {
            turn_id: Some(active_turn.turn_id),
            replies: vec![text_message("已将补充消息发送给当前 Codex 任务。")],
            react_to_message: false,
        }))
    }

    pub async fn handle_control(&self, request: CoreControlRequest) -> Result<CoreControlResponse> {
        let session = self
            .registry
            .resolve(&request.session_key, request.parent_session_key.as_deref())
            .await
            .context("resolve session failed")?;
        let selector = self.ensure_runtime_selection(&session).await?;

        let response = match request.action {
            ControlAction::ShowRuntime => {
                let active = self.selected_runtime(&selector).await?;
                CoreControlResponse {
                    ok: true,
                    message: if let Some(active) = active.as_ref() {
                        format!("当前已选定 `{}`。", active.label)
                    } else {
                        "当前已选定 agent 与 workspace，请继续选择会话或新建。".to_string()
                    },
                    selector: Some(selector_summary(&selector)),
                    active_runtime: active.as_ref().map(|runtime| runtime_summary(runtime, selector.selected_runtime_id.as_deref())),
                    runtimes: Vec::new(),
                    history_overview: None,
                }
            }
            ControlAction::ListRuntimes => {
                let runtimes = self.list_selector_runtimes(&selector).await?;
                CoreControlResponse {
                    ok: true,
                    message: if runtimes.is_empty() {
                        format!("`{}` 在当前 workspace 下还没有可选会话，请执行 `/ot new`。", selector.agent_kind)
                    } else {
                        format!("当前共有 {} 个可选会话，请执行 `/ot pick <short_id>`。", runtimes.len())
                    },
                    selector: Some(selector_summary(&selector)),
                    active_runtime: self
                        .selected_runtime(&selector)
                        .await?
                        .as_ref()
                        .map(|runtime| runtime_summary(runtime, selector.selected_runtime_id.as_deref())),
                    runtimes: runtimes
                        .iter()
                        .map(|runtime| runtime_summary(runtime, selector.selected_runtime_id.as_deref()))
                        .collect(),
                    history_overview: None,
                }
            }
            ControlAction::LoadRuntimes => {
                let workspace = self.resolve_runtime_load_workspace(
                    request.workspace_path.as_deref(),
                    &selector.workspace_path,
                )?;
                let updated_selector = self
                    .replace_runtime_selection(&session, &selector.agent_kind, &workspace, None, None, None)
                    .await?;
                let imported = self.load_agent_runtimes(&updated_selector).await?;
                let runtimes = self.list_selector_runtimes(&updated_selector).await?;
                CoreControlResponse {
                    ok: true,
                    message: if runtimes.is_empty() {
                        format!(
                            "`{}` 在 `{}` 下没有可选会话，请执行 `/ot new`。",
                            updated_selector.agent_kind, workspace
                        )
                    } else {
                        format!(
                            "`{}` 已在 `{}` 下加载 {} 个会话，请执行 `/ot pick <short_id>`。",
                            updated_selector.agent_kind,
                            workspace,
                            runtimes.len().max(imported)
                        )
                    },
                    selector: Some(selector_summary(&updated_selector)),
                    active_runtime: None,
                    runtimes: runtimes
                        .iter()
                        .map(|runtime| runtime_summary(runtime, updated_selector.selected_runtime_id.as_deref()))
                        .collect(),
                    history_overview: None,
                }
            }
            ControlAction::UseAgent => {
                let agent_kind = normalize_agent_kind(
                    request
                        .agent_kind
                        .as_deref()
                        .or(request.runtime_selector.as_deref())
                        .ok_or_else(|| anyhow!("use_agent requires agent_kind"))?,
                )?;
                let updated_selector = self
                    .replace_runtime_selection(
                        &session,
                        &agent_kind,
                        &selector.workspace_path,
                        None,
                        None,
                        None,
                    )
                    .await?;
                let _ = self.load_agent_runtimes(&updated_selector).await?;
                let runtimes = self.list_selector_runtimes(&updated_selector).await?;
                CoreControlResponse {
                    ok: true,
                    message: if runtimes.is_empty() {
                        format!("已切换到 `{}`，当前 workspace 下暂无可选会话，请执行 `/ot new`。", agent_kind)
                    } else {
                        format!("已切换到 `{}`，请从下方选择会话，或执行 `/ot new`。", agent_kind)
                    },
                    selector: Some(selector_summary(&updated_selector)),
                    active_runtime: None,
                    runtimes: runtimes
                        .iter()
                        .map(|runtime| runtime_summary(runtime, updated_selector.selected_runtime_id.as_deref()))
                        .collect(),
                    history_overview: None,
                }
            }
            ControlAction::CreateRuntime => {
                let workspace = self.resolve_workspace_path(
                    request.workspace_path.as_deref().or(Some(selector.workspace_path.as_str())),
                )?;
                let agent_kind = normalize_agent_kind(
                    request
                        .agent_kind
                        .as_deref()
                        .unwrap_or(selector.agent_kind.as_str()),
                )?;
                let runtime = self
                    .persistence
                    .create_runtime(
                        &runtime_scope_key(&agent_kind, &workspace),
                        request
                            .label
                            .as_deref()
                            .unwrap_or(&default_runtime_label(&agent_kind, &workspace)),
                        &agent_kind,
                        &workspace,
                        false,
                    )
                    .await?;
                let selected = self
                    .replace_runtime_selection(
                        &session,
                        &agent_kind,
                        &workspace,
                        Some(&runtime.runtime_id),
                        None,
                        None,
                    )
                    .await?;
                self.activate_runtime(&session, &runtime).await?;
                let runtimes = self.list_selector_runtimes(&selected).await?;
                CoreControlResponse {
                    ok: true,
                    message: format!("已新建并切换到 `{}`。", runtime.label),
                    selector: Some(selector_summary(&selected)),
                    active_runtime: Some(runtime_summary(&runtime, selected.selected_runtime_id.as_deref())),
                    runtimes: runtimes
                        .iter()
                        .map(|item| runtime_summary(item, selected.selected_runtime_id.as_deref()))
                        .collect(),
                    history_overview: None,
                }
            }
            ControlAction::SwitchRuntime => {
                let runtime_selector = request
                    .runtime_selector
                    .as_deref()
                    .ok_or_else(|| anyhow!("pick runtime requires runtime_selector"))?;
                let runtimes = self.list_selector_runtimes(&selector).await?;
                let runtime = runtimes
                    .into_iter()
                    .find(|runtime| runtime_matches_selector(runtime, runtime_selector))
                    .ok_or_else(|| anyhow!("runtime not found: {}", runtime_selector))?;
                let selected = self
                    .replace_runtime_selection(
                        &session,
                        &runtime.agent_kind,
                        &runtime.workspace_path,
                        Some(&runtime.runtime_id),
                        None,
                        None,
                    )
                    .await?;
                self.activate_runtime(&session, &runtime).await?;
                let history_overview = self
                    .load_runtime_history_overview(&selected, &runtime)
                    .await?;
                CoreControlResponse {
                    ok: true,
                    message: format!("已选定 `{}`。", runtime.label),
                    selector: Some(selector_summary(&selected)),
                    active_runtime: Some(runtime_summary(&runtime, selected.selected_runtime_id.as_deref())),
                    runtimes: self
                        .list_selector_runtimes(&selected)
                        .await?
                        .iter()
                        .map(|item| runtime_summary(item, selected.selected_runtime_id.as_deref()))
                        .collect(),
                    history_overview,
                }
            }
            ControlAction::SetWorkspace => {
                let workspace = self.resolve_workspace_path(request.workspace_path.as_deref())?;
                let updated_selector = self
                    .replace_runtime_selection(&session, &selector.agent_kind, &workspace, None, None, None)
                    .await?;
                let _ = self.load_agent_runtimes(&updated_selector).await?;
                let runtimes = self.list_selector_runtimes(&updated_selector).await?;

                CoreControlResponse {
                    ok: true,
                    message: if runtimes.is_empty() {
                        format!("当前 workspace 已切换到 `{}`，暂无可选会话，请执行 `/ot new`。", workspace)
                    } else {
                        format!("当前 workspace 已切换到 `{}`，请重新选择会话。", workspace)
                    },
                    selector: Some(selector_summary(&updated_selector)),
                    active_runtime: None,
                    runtimes: runtimes
                        .iter()
                        .map(|runtime| runtime_summary(runtime, updated_selector.selected_runtime_id.as_deref()))
                        .collect(),
                    history_overview: None,
                }
            }
            ControlAction::SetProxy => {
                let proxy_mode = normalize_proxy_mode(
                    request
                        .proxy_mode
                        .as_deref()
                        .ok_or_else(|| anyhow!("set_proxy requires proxy_mode"))?,
                )?;
                let proxy_url = normalize_proxy_url(
                    &proxy_mode,
                    request.proxy_url.as_deref(),
                    self.config.acp_proxy_url.as_deref(),
                )?;
                let updated_selector = self
                    .replace_runtime_selection(
                        &session,
                        &selector.agent_kind,
                        &selector.workspace_path,
                        selector.selected_runtime_id.as_deref(),
                        Some(&proxy_mode),
                        proxy_url.as_deref(),
                    )
                    .await?;
                let active = self.selected_runtime(&updated_selector).await?;
                let runtimes = self.list_selector_runtimes(&updated_selector).await?;
                CoreControlResponse {
                    ok: true,
                    message: if let Some(url) = proxy_url.as_deref() {
                        format!("当前代理模式已切换为 `{}`，代理地址为 `{}`。", proxy_mode, url)
                    } else {
                        format!("当前代理模式已切换为 `{}`。", proxy_mode)
                    },
                    selector: Some(selector_summary(&updated_selector)),
                    active_runtime: active
                        .as_ref()
                        .map(|runtime| runtime_summary(runtime, updated_selector.selected_runtime_id.as_deref())),
                    runtimes: runtimes
                        .iter()
                        .map(|runtime| runtime_summary(runtime, updated_selector.selected_runtime_id.as_deref()))
                        .collect(),
                    history_overview: None,
                }
            }
            ControlAction::StopRuntime => {
                let turn_id = self
                    .active_turns
                    .request_stop(&session.session_key)
                    .await
                    .ok_or_else(|| anyhow!("当前没有正在运行的任务。"))?;
                info!(
                    "turn stop requested: turn_id={} session_key={}",
                    turn_id, session.session_key
                );
                CoreControlResponse {
                    ok: true,
                    message: format!("已请求停止当前任务 `{}`。", turn_id),
                    selector: Some(selector_summary(&selector)),
                    active_runtime: self
                        .selected_runtime(&selector)
                        .await?
                        .as_ref()
                        .map(|runtime| runtime_summary(runtime, selector.selected_runtime_id.as_deref())),
                    runtimes: self
                        .list_selector_runtimes(&selector)
                        .await?
                        .iter()
                        .map(|runtime| runtime_summary(runtime, selector.selected_runtime_id.as_deref()))
                        .collect(),
                    history_overview: None,
                }
            }
        };

        Ok(response)
    }

    async fn run_turn(
        &self,
        request: CoreTurnRequest,
        session: SessionInfo,
        selection: RuntimeSelection,
        runtime: RuntimeInstance,
    ) -> Result<()> {
        let session_lock = self.session_locks.get(&session.session_key).await;
        let _guard = session_lock.lock().await;
        self.active_turns
            .set_starting(&session.session_key, &request.turn_id)
            .await;
        info!(
            "turn start: turn_id={} session_key={} agent={} workspace={}",
            request.turn_id,
            session.session_key,
            runtime.agent_kind,
            runtime.workspace_path
        );
        let result = async {
            self.persistence.mark_turn_running(&request.turn_id).await?;

            let prompt = self.build_prompt(&session, &request.text).await;
            let mut render = TurnRenderState::new(&session);

            let mut turn = self
                .runtime
                .start_turn(RuntimeTurnRequest {
                    session_key: session.session_key.clone(),
                    prompt,
                    runtime_session_ref: runtime.runtime_session_ref.clone(),
                    agent_kind: Some(runtime.agent_kind.clone()),
                    workspace_path: Some(runtime.workspace_path.clone().into()),
                    proxy_mode: Some(selection.proxy_mode.clone()),
                    proxy_url: selection.proxy_url.clone(),
                })
                .await
                .context("start runtime turn failed")?;

            if let Some(runtime_session_ref) = turn.runtime_session_ref.as_ref() {
                render.apply_event(NormalizedAgentEvent::RuntimeSessionReady(
                    runtime_session_ref.clone(),
                ));
                self.registry
                    .replace_runtime_state(
                        &session.session_key,
                        Some(runtime_session_ref.clone()),
                        None,
                    )
                    .await?;
                self.persistence
                    .update_runtime_state(&runtime.runtime_id, Some(runtime_session_ref), None)
                    .await?;
            }

            self.active_turns
                .attach_cancel(
                    &session.session_key,
                    &request.turn_id,
                    Some(&runtime.agent_kind),
                    turn.runtime_session_ref.as_deref(),
                    turn.runtime_turn_ref.as_deref(),
                    turn.cancel.clone(),
                )
                .await;

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

                if render.has_progress_update() {
                    self.publish(&request.turn_id, OutboundSlot::Progress, render.progress_message())
                        .await?;
                    render.mark_flushed();
                }
            }

            let completion = match turn.completion.await.context("join runtime turn failed")? {
                Ok(done) => done,
                Err(err) => {
                    let error_text = err.to_string();
                    error!(
                        "turn failed: turn_id={} session_key={} error={}",
                        request.turn_id, session.session_key, error_text
                    );
                    render.fail(error_text.clone());
                    if render.has_progress_update() {
                        let _ = self
                            .publish(&request.turn_id, OutboundSlot::Progress, render.progress_message())
                            .await;
                        render.mark_flushed();
                    }
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

            if render.has_progress_update() {
                self.publish(&request.turn_id, OutboundSlot::Progress, render.progress_message())
                    .await?;
                render.mark_flushed();
            }
            self.publish(&request.turn_id, OutboundSlot::Final, render.final_message())
                .await?;

            if let Some(stop_reason) = completion.stop_reason.as_deref() {
                info!(
                    "turn stop_reason: turn_id={} session_key={} stop_reason={}",
                    request.turn_id, session.session_key, stop_reason
                );
            }
            if let Some(stderr_text) = completion.stderr_summary.as_ref() {
                info!("agent runtime stderr summary: {}", shorten(stderr_text, 400));
            }

            info!(
                "turn completed: turn_id={} session_key={}",
                request.turn_id, session.session_key
            );

            Ok(())
        }
        .await;

        self.active_turns
            .clear(&session.session_key, &request.turn_id)
            .await;
        result
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

    async fn ensure_runtime_selection(&self, session: &SessionInfo) -> Result<RuntimeSelection> {
        if let Some(selection) = self.persistence.get_runtime_selection(&session.session_key).await? {
            return Ok(selection);
        }

        if let Some(runtime) = self.persistence.get_active_runtime(&session.session_key).await? {
            self.persistence
                .upsert_runtime_selection(
                    &session.session_key,
                    &runtime.agent_kind,
                    &runtime.workspace_path,
                    Some(&runtime.runtime_id),
                    "default",
                    self.config.acp_proxy_url.as_deref(),
                )
                .await?;
            return Ok(RuntimeSelection {
                session_key: session.session_key.clone(),
                agent_kind: runtime.agent_kind,
                workspace_path: runtime.workspace_path,
                selected_runtime_id: Some(runtime.runtime_id),
                proxy_mode: "default".to_string(),
                proxy_url: self.config.acp_proxy_url.clone(),
            });
        }

        let default_workspace = self.config.codex_workdir.to_string_lossy().to_string();
        let workspace = self.resolve_workspace_path(Some(&default_workspace))?;
        let agent_kind = self.config.acp_adapter.clone();
        self.persistence
            .upsert_runtime_selection(
                &session.session_key,
                &agent_kind,
                &workspace,
                None,
                "default",
                self.config.acp_proxy_url.as_deref(),
            )
            .await?;
        Ok(RuntimeSelection {
            session_key: session.session_key.clone(),
            agent_kind,
            workspace_path: workspace,
            selected_runtime_id: None,
            proxy_mode: "default".to_string(),
            proxy_url: self.config.acp_proxy_url.clone(),
        })
    }

    async fn replace_runtime_selection(
        &self,
        session: &SessionInfo,
        agent_kind: &str,
        workspace_path: &str,
        selected_runtime_id: Option<&str>,
        proxy_mode: Option<&str>,
        proxy_url: Option<&str>,
    ) -> Result<RuntimeSelection> {
        let existing = self.persistence.get_runtime_selection(&session.session_key).await?;
        let next_proxy_mode = proxy_mode
            .map(str::to_string)
            .or_else(|| existing.as_ref().map(|selection| selection.proxy_mode.clone()))
            .unwrap_or_else(|| "default".to_string());
        let next_proxy_url = proxy_url
            .map(str::to_string)
            .or_else(|| existing.as_ref().and_then(|selection| selection.proxy_url.clone()))
            .or_else(|| self.config.acp_proxy_url.clone());
        self.persistence
            .upsert_runtime_selection(
                &session.session_key,
                agent_kind,
                workspace_path,
                selected_runtime_id,
                &next_proxy_mode,
                next_proxy_url.as_deref(),
            )
            .await?;
        if selected_runtime_id.is_some() {
            if let Some(runtime_id) = selected_runtime_id {
                self.persistence
                    .set_active_runtime(&session.session_key, runtime_id)
                    .await?;
            }
        } else {
            self.persistence.clear_active_runtime(&session.session_key).await?;
        }
        Ok(RuntimeSelection {
            session_key: session.session_key.clone(),
            agent_kind: agent_kind.to_string(),
            workspace_path: workspace_path.to_string(),
            selected_runtime_id: selected_runtime_id.map(str::to_string),
            proxy_mode: next_proxy_mode,
            proxy_url: next_proxy_url,
        })
    }

    async fn selected_runtime(&self, selector: &RuntimeSelection) -> Result<Option<RuntimeInstance>> {
        match selector.selected_runtime_id.as_deref() {
            Some(runtime_id) => self.persistence.get_runtime(runtime_id).await,
            None => Ok(None),
        }
    }

    async fn list_selector_runtimes(&self, selector: &RuntimeSelection) -> Result<Vec<RuntimeInstance>> {
        let scope_key = runtime_scope_key(&selector.agent_kind, &selector.workspace_path);
        let mut runtimes = self.persistence.list_runtimes(&scope_key).await?;
        for runtime in &mut runtimes {
            runtime.is_active = selector.selected_runtime_id.as_deref() == Some(runtime.runtime_id.as_str());
        }
        Ok(runtimes)
    }

    fn resolve_workspace_path(&self, candidate: Option<&str>) -> Result<String> {
        let default_workspace = self.config.codex_workdir.to_string_lossy().to_string();
        let raw = candidate
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(default_workspace.as_str());
        let path = expand_workspace_input(raw)?;
        let canonical = path
            .canonicalize()
            .with_context(|| format!("workspace path not found: {}", raw))?;
        if !canonical.is_dir() {
            anyhow::bail!("workspace path is not a directory: {}", canonical.display());
        }
        Ok(canonical.to_string_lossy().to_string())
    }

    fn resolve_runtime_load_workspace(
        &self,
        workspace_path: Option<&str>,
        current_workspace: &str,
    ) -> Result<String> {
        match workspace_path {
            Some(value) => self.resolve_workspace_path(Some(value)),
            None => self.resolve_workspace_path(Some(current_workspace)),
        }
    }

    async fn load_agent_runtimes(&self, selector: &RuntimeSelection) -> Result<usize> {
        let scope_key = runtime_scope_key(&selector.agent_kind, &selector.workspace_path);
        match selector.agent_kind.as_str() {
            "claude_code" => self.load_claude_runtimes(&scope_key, selector).await,
            "codex" => self.load_codex_runtimes(&scope_key, selector).await,
            other => Err(anyhow!("unsupported agent kind: {}", other)),
        }
    }

    async fn load_claude_runtimes(&self, scope_key: &str, selector: &RuntimeSelection) -> Result<usize> {
        let workspace = PathBuf::from(&selector.workspace_path);
        let discovered = match self
            .discover_sessions_via_runtime(selector)
            .await
        {
            Ok(sessions) => sessions,
            Err(err) if crate::agent::runtime::is_list_sessions_unsupported_error(&err) => {
                self.discover_claude_sessions(&workspace).await?
            }
            Err(err) => return Err(err),
        };
        self.import_discovered_runtimes(scope_key, "claude_code", &discovered)
            .await
    }

    async fn load_codex_runtimes(&self, scope_key: &str, selector: &RuntimeSelection) -> Result<usize> {
        let workspace = PathBuf::from(&selector.workspace_path);
        let discovered = match self.discover_sessions_via_runtime(selector).await {
            Ok(sessions) => sessions,
            Err(err) if crate::agent::runtime::is_list_sessions_unsupported_error(&err) => {
                self.discover_codex_sessions(&workspace).await?
            }
            Err(err) => return Err(err),
        };
        self.import_discovered_runtimes(scope_key, "codex", &discovered)
            .await
    }

    async fn discover_sessions_via_runtime(
        &self,
        selector: &RuntimeSelection,
    ) -> Result<Vec<DiscoveredRuntimeSession>> {
        let sessions = self
            .runtime
            .list_sessions(crate::agent::runtime::RuntimeSessionQuery {
                agent_kind: Some(selector.agent_kind.clone()),
                workspace_path: PathBuf::from(&selector.workspace_path),
                proxy_mode: Some(selector.proxy_mode.clone()),
                proxy_url: selector
                    .proxy_url
                    .clone()
                    .or_else(|| self.config.acp_proxy_url.clone()),
            })
            .await?;
        Ok(sessions
            .into_iter()
            .map(|session| DiscoveredRuntimeSession {
                runtime_session_ref: session.runtime_session_ref,
                workspace_path: session.workspace_path,
                prompt_preview: session.title,
                tag: None,
                modified_at: 0,
            })
            .collect())
    }

    async fn import_discovered_runtimes(
        &self,
        scope_key: &str,
        agent_kind: &str,
        discovered: &[DiscoveredRuntimeSession],
    ) -> Result<usize> {
        let mut imported = 0usize;
        for session in discovered {
            self.persistence
                .import_runtime(
                    scope_key,
                    &imported_runtime_label(agent_kind, session),
                    agent_kind,
                    &session.workspace_path,
                    &session.runtime_session_ref,
                    session.tag.as_deref(),
                    session.prompt_preview.as_deref(),
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
    ) -> Result<Vec<DiscoveredRuntimeSession>> {
        let claude_home = self.config.claude_home_dir.clone();
        let workspace_path = workspace_path.to_path_buf();
        tokio::task::spawn_blocking(move || discover_claude_sessions(&claude_home, &workspace_path))
            .await
            .context("join claude session discovery task failed")?
    }

    async fn discover_codex_sessions(
        &self,
        workspace_path: &Path,
    ) -> Result<Vec<DiscoveredRuntimeSession>> {
        let codex_home = self.config.codex_home_dir.clone();
        let workspace_path = workspace_path.to_path_buf();
        tokio::task::spawn_blocking(move || discover_codex_sessions(&codex_home, &workspace_path))
            .await
            .context("join codex session discovery task failed")?
    }

    async fn load_runtime_history_overview(
        &self,
        selector: &RuntimeSelection,
        runtime: &RuntimeInstance,
    ) -> Result<Option<crate::protocol::RuntimeHistoryOverview>> {
        let Some(runtime_session_ref) = runtime.runtime_session_ref.clone() else {
            return Ok(None);
        };
        let history = match self
            .runtime
            .load_history(crate::agent::runtime::RuntimeHistoryQuery {
                agent_kind: Some(selector.agent_kind.clone()),
                workspace_path: PathBuf::from(&selector.workspace_path),
                runtime_session_ref: runtime_session_ref.clone(),
                proxy_mode: Some(selector.proxy_mode.clone()),
                proxy_url: selector
                    .proxy_url
                    .clone()
                    .or_else(|| self.config.acp_proxy_url.clone()),
            })
            .await
        {
            Ok(history) => history,
            Err(err) => {
                warn!(
                    "runtime history overview unavailable: runtime_id={} agent={} session_ref={} error={err:#}",
                    runtime.runtime_id, selector.agent_kind, runtime_session_ref
                );
                return Ok(None);
            }
        };
        let turns = history
            .into_iter()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|turn| crate::protocol::RuntimeHistoryTurn {
                user_text: shorten_history_line(&turn.user_text),
                assistant_text: shorten_history_line(&turn.assistant_text),
            })
            .collect::<Vec<_>>();
        if turns.is_empty() {
            return Ok(None);
        }
        Ok(Some(crate::protocol::RuntimeHistoryOverview {
            runtime_session_ref,
            turns,
        }))
    }
}

fn default_runtime_label(agent_kind: &str, workspace_path: &str) -> String {
    let leaf = std::path::Path::new(workspace_path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("workspace");
    format!("{}-{}", agent_kind, leaf)
}

fn runtime_scope_key(agent_kind: &str, workspace_path: &str) -> String {
    format!("scope:{}:{}", agent_kind, workspace_path)
}

fn expand_workspace_input(raw: &str) -> Result<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("workspace path is empty");
    }

    if trimmed == "~" {
        return home_dir().map_err(|_| anyhow!("cannot resolve `~`: HOME is not set"));
    }

    if let Some(stripped) = trimmed.strip_prefix("~/") {
        return home_dir()
            .map(|home| home.join(stripped))
            .map_err(|_| anyhow!("cannot resolve `~/{}`: HOME is not set", stripped));
    }

    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        return Ok(path);
    }

    Ok(std::env::current_dir()
        .context("failed to resolve current working directory for relative workspace path")?
        .join(path))
}

fn home_dir() -> Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn normalize_agent_kind(raw: &str) -> Result<String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude_code" | "claude-code" => Ok("claude_code".to_string()),
        "codex" => Ok("codex".to_string()),
        other => Err(anyhow!("unsupported agent kind: {}", other)),
    }
}

fn normalize_proxy_mode(raw: &str) -> Result<String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "default" | "auto" => Ok("default".to_string()),
        "on" | "enable" | "enabled" => Ok("on".to_string()),
        "off" | "disable" | "disabled" => Ok("off".to_string()),
        other => Err(anyhow!("unsupported proxy mode: {}", other)),
    }
}

fn normalize_proxy_url(
    proxy_mode: &str,
    request_proxy_url: Option<&str>,
    default_proxy_url: Option<&str>,
) -> Result<Option<String>> {
    let cleaned = request_proxy_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(extract_proxy_url)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    match proxy_mode {
        "off" => Ok(None),
        "on" => Ok(Some(
            cleaned
                .or_else(|| default_proxy_url.map(str::to_string))
                .ok_or_else(|| anyhow!("proxy mode `on` requires proxy url or ACP_PROXY_URL/ALL_PROXY/HTTPS_PROXY/HTTP_PROXY"))?,
        )),
        "default" => Ok(cleaned.or_else(|| default_proxy_url.map(str::to_string))),
        other => Err(anyhow!("unsupported proxy mode: {}", other)),
    }
}

fn extract_proxy_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some((label, target)) = parse_markdown_link(trimmed) {
        if label == target || trim_trailing_slash(&label) == trim_trailing_slash(&target) {
            return label;
        }
    }
    trimmed.to_string()
}

fn parse_markdown_link(value: &str) -> Option<(String, String)> {
    let open_bracket = value.find('[')?;
    let close_bracket = value[open_bracket + 1..].find(']')? + open_bracket + 1;
    let open_paren = value[close_bracket + 1..].find('(')? + close_bracket + 1;
    let close_paren = value[open_paren + 1..].rfind(')')? + open_paren + 1;
    if open_bracket != 0 || open_paren != close_bracket + 1 || close_paren != value.len() - 1 {
        return None;
    }
    Some((
        value[open_bracket + 1..close_bracket].to_string(),
        value[open_paren + 1..close_paren].to_string(),
    ))
}

fn trim_trailing_slash(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

fn shorten_history_line(value: &str) -> String {
    let first = value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    shorten(first, 120)
}

fn selector_summary(selection: &RuntimeSelection) -> RuntimeSelectorSummary {
    RuntimeSelectorSummary {
        agent_kind: selection.agent_kind.clone(),
        workspace_path: selection.workspace_path.clone(),
        has_selected_runtime: selection.selected_runtime_id.is_some(),
        proxy_mode: selection.proxy_mode.clone(),
        proxy_url: selection.proxy_url.clone(),
    }
}

fn missing_runtime_selection_message(selection: &RuntimeSelection) -> String {
    format!(
        "当前还没有选定会话。当前 agent 为 `{}`，workspace 为 `{}`。`/ot use <claude|codex>` 和 `/ot cwd <path>` 可以按任意顺序调整，准备好后再执行 `/ot pick <short_id>` 或 `/ot new`。",
        selection.agent_kind, selection.workspace_path
    )
}

fn runtime_summary(runtime: &RuntimeInstance, selected_runtime_id: Option<&str>) -> RuntimeSummary {
    RuntimeSummary {
        runtime_id: runtime.runtime_id.clone(),
        label: runtime.label.clone(),
        agent_kind: runtime.agent_kind.clone(),
        workspace_path: runtime.workspace_path.clone(),
        runtime_session_ref: runtime.runtime_session_ref.clone(),
        tag: runtime.tag.clone(),
        prompt_preview: runtime.prompt_preview.clone(),
        has_runtime_session_ref: runtime.runtime_session_ref.is_some(),
        is_active: selected_runtime_id == Some(runtime.runtime_id.as_str()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiscoveredRuntimeSession {
    runtime_session_ref: String,
    workspace_path: String,
    prompt_preview: Option<String>,
    tag: Option<String>,
    modified_at: i64,
}

fn discover_claude_sessions(
    claude_home: &Path,
    workspace_path: &Path,
) -> Result<Vec<DiscoveredRuntimeSession>> {
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

fn load_sessions_from_index(dir: &Path) -> Result<Vec<DiscoveredRuntimeSession>> {
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
        sessions.push(DiscoveredRuntimeSession {
            runtime_session_ref: session_id.to_string(),
            workspace_path,
            prompt_preview: entry
                .get("firstPrompt")
                .and_then(Value::as_str)
                .map(str::to_string),
            tag: entry
                .get("gitBranch")
                .and_then(Value::as_str)
                .map(str::to_string),
            modified_at,
        });
    }
    Ok(sessions)
}

fn load_sessions_from_jsonl_dir(dir: &Path) -> Result<Vec<DiscoveredRuntimeSession>> {
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

fn load_session_from_jsonl(path: &Path) -> Result<Option<DiscoveredRuntimeSession>> {
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

    Ok(Some(DiscoveredRuntimeSession {
        runtime_session_ref,
        workspace_path: workspace_path.unwrap_or_default(),
        prompt_preview: first_prompt,
        tag: git_branch,
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

fn discover_codex_sessions(
    codex_home: &Path,
    workspace_path: &Path,
) -> Result<Vec<DiscoveredRuntimeSession>> {
    let mut sessions = load_codex_sessions_from_sqlite(codex_home, workspace_path)?;
    sessions.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.runtime_session_ref.cmp(&b.runtime_session_ref))
    });
    Ok(sessions)
}

fn load_codex_sessions_from_sqlite(
    codex_home: &Path,
    workspace_path: &Path,
) -> Result<Vec<DiscoveredRuntimeSession>> {
    let db_path = codex_home.join("state_5.sqlite");
    if !db_path.exists() {
        return Ok(Vec::new());
    }

    let workspace_variants = workspace_variants(workspace_path);
    let conn = rusqlite::Connection::open(&db_path)
        .with_context(|| format!("open codex sqlite failed: {:?}", db_path))?;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, cwd, title, updated_at, git_branch, first_user_message
        FROM threads
        WHERE archived = 0
        ORDER BY updated_at DESC
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(DiscoveredRuntimeSession {
            runtime_session_ref: row.get(0)?,
            workspace_path: row.get(1)?,
            prompt_preview: {
                let title: Option<String> = row.get(2)?;
                let first_user_message: Option<String> = row.get(5)?;
                first_user_message
                    .filter(|value| !value.trim().is_empty())
                    .or_else(|| title.filter(|value| !value.trim().is_empty()))
            },
            modified_at: row.get(3)?,
            tag: row.get(4)?,
        })
    })?;

    let mut sessions = Vec::new();
    let mut seen = HashSet::new();
    for row in rows {
        let session = row?;
        if !workspace_variants_match(&workspace_variants, &session.workspace_path) {
            continue;
        }
        if seen.insert(session.runtime_session_ref.clone()) {
            sessions.push(session);
        }
    }
    Ok(sessions)
}

fn workspace_variants(path: &Path) -> HashSet<String> {
    let mut variants = HashSet::new();
    let mut candidates = vec![path.to_path_buf()];
    if let Ok(canonical) = path.canonicalize() {
        candidates.push(canonical);
    }

    for candidate in candidates {
        variants.insert(candidate.to_string_lossy().to_string());
        if let Some(stripped) = strip_private_prefix(&candidate) {
            variants.insert(stripped.to_string_lossy().to_string());
        }
        if let Some(prefixed) = add_private_prefix(&candidate) {
            variants.insert(prefixed.to_string_lossy().to_string());
        }
    }
    variants
}

fn workspace_variants_match(expected: &HashSet<String>, candidate: &str) -> bool {
    let candidate_path = PathBuf::from(candidate);
    let candidate_variants = workspace_variants(&candidate_path);
    candidate_variants.iter().any(|value| expected.contains(value))
}

fn imported_runtime_label(agent_kind: &str, session: &DiscoveredRuntimeSession) -> String {
    let short_agent = match agent_kind {
        "claude_code" => "claude",
        other => other,
    };
    format!("{}-{}", short_agent, shorten(&session.runtime_session_ref, 8))
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
    status: &'static str,
    buffered_assistant_text: String,
    pending_progress_text: String,
    current_assistant_text: String,
    final_text: Option<String>,
    runtime_session_ref: Option<String>,
    error: Option<String>,
    todo_items: Vec<TodoEntry>,
    active_tool_count: usize,
}

impl TurnRenderState {
    fn new(session: &SessionInfo) -> Self {
        Self {
            status: "running",
            buffered_assistant_text: String::new(),
            pending_progress_text: String::new(),
            current_assistant_text: String::new(),
            final_text: None,
            runtime_session_ref: session.runtime_session_ref.clone(),
            error: None,
            todo_items: Vec::new(),
            active_tool_count: 0,
        }
    }

    fn apply_event(&mut self, event: NormalizedAgentEvent) -> bool {
        let prev_todos = self.todo_items.clone();
        match event {
            NormalizedAgentEvent::TurnStarted => {
                self.status = "running";
            }
            NormalizedAgentEvent::TurnCompleted => {
                self.status = "completed";
            }
            NormalizedAgentEvent::RuntimeSessionReady(runtime_session_ref) => {
                self.runtime_session_ref = Some(runtime_session_ref);
            }
            NormalizedAgentEvent::AssistantChunk(text) => {
                self.buffered_assistant_text.push_str(&text);
                self.current_assistant_text.push_str(&text);
            }
            NormalizedAgentEvent::AssistantMessage(text) => {
                self.buffered_assistant_text.clear();
                self.current_assistant_text = text.clone();
                self.final_text = Some(text);
            }
            NormalizedAgentEvent::ToolState { state, .. } => match state {
                crate::agent::normalized::AgentToolState::Pending
                | crate::agent::normalized::AgentToolState::InProgress => {
                    self.queue_progress_from_buffer();
                    self.active_tool_count = self.active_tool_count.saturating_add(1);
                    if !self.current_assistant_text.trim().is_empty() {
                        self.current_assistant_text.clear();
                    }
                }
                crate::agent::normalized::AgentToolState::Completed
                | crate::agent::normalized::AgentToolState::Failed => {
                    self.active_tool_count = self.active_tool_count.saturating_sub(1);
                }
            },
            NormalizedAgentEvent::PlanUpdated(todos) => {
                self.todo_items = todos;
            }
            NormalizedAgentEvent::Usage(_) => {}
        }
        self.todo_items != prev_todos
    }

    fn finalize(&mut self) {
        if self.status != "failed" {
            self.status = "completed";
        }
        if self.final_text.is_none() && !self.current_assistant_text.trim().is_empty() {
            self.final_text = Some(self.current_assistant_text.clone());
        }
        self.buffered_assistant_text.clear();
    }

    fn fail(&mut self, error_text: String) {
        self.status = "failed";
        self.error = Some(error_text);
    }

    fn queue_progress_from_buffer(&mut self) {
        if self.pending_progress_text.trim().is_empty() && !self.buffered_assistant_text.trim().is_empty() {
            self.pending_progress_text = self.buffered_assistant_text.clone();
        }
        self.buffered_assistant_text.clear();
    }

    fn has_progress_update(&self) -> bool {
        !self.pending_progress_text.trim().is_empty()
    }

    fn mark_flushed(&mut self) {
        self.pending_progress_text.clear();
    }

    fn final_assistant_message(&self) -> Option<String> {
        self.final_text.clone()
    }

    fn progress_message(&self) -> OutboundMessage {
        text_message(self.pending_progress_text.clone())
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

        card_message(
            if self.status == "failed" { "⚠️ 运行异常" } else { "✅ 本轮已结束" },
            if self.status == "failed" {
                CardTheme::Red
            } else {
                CardTheme::Green
            },
            false,
            vec![CardBlock::Markdown {
                text: shorten(&text, 2200),
            }],
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

#[cfg(test)]
mod tests {
    use std::{path::{Path, PathBuf}, sync::Arc};

    use anyhow::Result;
    use async_trait::async_trait;
    use tokio::sync::{mpsc, Mutex};
    use tokio::time::Duration;

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
        sessions: Vec<crate::agent::runtime::RuntimeSessionListing>,
        history: Vec<crate::agent::runtime::RuntimeHistoryTurn>,
    }

    #[derive(Clone, Default)]
    struct InterruptibleMockRuntime;

    #[derive(Clone, Default)]
    struct SlowInterruptibleMockRuntime;

    #[derive(Clone, Default)]
    struct SteeringMockRuntime {
        steer_requests: Arc<Mutex<Vec<RuntimeSteerRequest>>>,
    }

    #[derive(Clone, Default)]
    struct HistoryUnavailableMockRuntime;

    #[async_trait]
    impl AgentRuntime for MockRuntime {
        async fn start_turn(&self, request: RuntimeTurnRequest) -> Result<RuntimeTurn> {
            self.captured.lock().await.push(request);
            let (tx, rx) = mpsc::unbounded_channel();
            let (cancel, _cancel_rx) = crate::agent::runtime::RuntimeCancelHandle::new();
            let events = self.events.clone();
            tokio::spawn(async move {
                for event in events {
                    let _ = tx.send(event);
                }
            });
            let completion = tokio::spawn(async { Ok(RuntimeCompletion::default()) });
            Ok(RuntimeTurn {
                events: rx,
                completion,
                cancel,
                runtime_session_ref: None,
                runtime_turn_ref: None,
            })
        }

        async fn list_sessions(
            &self,
            _query: crate::agent::runtime::RuntimeSessionQuery,
        ) -> Result<Vec<crate::agent::runtime::RuntimeSessionListing>> {
            if self.sessions.is_empty() {
                return Err(anyhow!(crate::agent::runtime::LIST_SESSIONS_UNSUPPORTED_ERROR_TEXT));
            }
            Ok(self.sessions.clone())
        }

        async fn load_history(
            &self,
            _query: crate::agent::runtime::RuntimeHistoryQuery,
        ) -> Result<Vec<crate::agent::runtime::RuntimeHistoryTurn>> {
            Ok(self.history.clone())
        }

        fn name(&self) -> &'static str {
            "mock"
        }
    }

    #[async_trait]
    impl AgentRuntime for InterruptibleMockRuntime {
        async fn start_turn(&self, _request: RuntimeTurnRequest) -> Result<RuntimeTurn> {
            let (tx, rx) = mpsc::unbounded_channel();
            let (cancel, mut cancel_rx) = crate::agent::runtime::RuntimeCancelHandle::new();
            let completion = tokio::spawn(async move {
                let _ = cancel_rx.changed().await;
                drop(tx);
                Err(anyhow!(crate::agent::runtime::INTERRUPTED_ERROR_TEXT))
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
            "interruptible-mock"
        }
    }

    #[async_trait]
    impl AgentRuntime for SlowInterruptibleMockRuntime {
        async fn start_turn(&self, _request: RuntimeTurnRequest) -> Result<RuntimeTurn> {
            tokio::time::sleep(Duration::from_millis(120)).await;
            let (tx, rx) = mpsc::unbounded_channel();
            let (cancel, mut cancel_rx) = crate::agent::runtime::RuntimeCancelHandle::new();
            let completion = tokio::spawn(async move {
                let _ = cancel_rx.changed().await;
                drop(tx);
                Err(anyhow!(crate::agent::runtime::INTERRUPTED_ERROR_TEXT))
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
            "slow-interruptible-mock"
        }
    }

    #[async_trait]
    impl AgentRuntime for SteeringMockRuntime {
        async fn start_turn(&self, _request: RuntimeTurnRequest) -> Result<RuntimeTurn> {
            let (_tx, rx) = mpsc::unbounded_channel();
            let (cancel, _cancel_rx) = crate::agent::runtime::RuntimeCancelHandle::new();
            let completion = tokio::spawn(async { Ok(RuntimeCompletion::default()) });
            Ok(RuntimeTurn {
                events: rx,
                completion,
                cancel,
                runtime_session_ref: None,
                runtime_turn_ref: None,
            })
        }

        async fn steer_turn(&self, request: RuntimeSteerRequest) -> Result<()> {
            self.steer_requests.lock().await.push(request);
            Ok(())
        }

        fn name(&self) -> &'static str {
            "steering-mock"
        }
    }

    #[async_trait]
    impl AgentRuntime for HistoryUnavailableMockRuntime {
        async fn start_turn(&self, _request: RuntimeTurnRequest) -> Result<RuntimeTurn> {
            let (_tx, rx) = mpsc::unbounded_channel();
            let (cancel, _cancel_rx) = crate::agent::runtime::RuntimeCancelHandle::new();
            let completion = tokio::spawn(async { Ok(RuntimeCompletion::default()) });
            Ok(RuntimeTurn {
                events: rx,
                completion,
                cancel,
                runtime_session_ref: None,
                runtime_turn_ref: None,
            })
        }

        async fn load_history(
            &self,
            _query: crate::agent::runtime::RuntimeHistoryQuery,
        ) -> Result<Vec<crate::agent::runtime::RuntimeHistoryTurn>> {
            Err(anyhow!("history unavailable"))
        }

        fn name(&self) -> &'static str {
            "history-unavailable-mock"
        }
    }

    fn test_config() -> Arc<Config> {
        test_config_with_paths(
            PathBuf::from("."),
            PathBuf::from(format!("/tmp/otterlink-claude-{}", uuid::Uuid::new_v4())),
            PathBuf::from(format!("/tmp/otterlink-codex-{}", uuid::Uuid::new_v4())),
        )
    }

    fn test_config_with_paths(
        codex_workdir: PathBuf,
        claude_home_dir: PathBuf,
        codex_home_dir: PathBuf,
    ) -> Arc<Config> {
        Arc::new(Config {
            core_bind: "127.0.0.1:39001".parse().unwrap(),
            core_ingest_token: None,
            gateway_event_url: "http://127.0.0.1:39000/internal/gateway/event".to_string(),
            gateway_event_token: None,
            state_db_path: PathBuf::from(format!("/tmp/otterlink-service-{}.db", uuid::Uuid::new_v4())),
            claude_home_dir,
            codex_home_dir,
            acp_proxy_url: Some("http://127.0.0.1:7890".to_string()),
            claude_code_default_proxy_mode: "off".to_string(),
            codex_default_proxy_mode: "on".to_string(),
            codex_bin: "codex".to_string(),
            codex_workdir,
            codex_model: None,
            codex_skip_git_repo_check: true,
            runtime_mode: "exec_json".to_string(),
            acp_adapter: "codex".to_string(),
            acp_agent_cmd: None,
            render_min_update_ms: 0,
            todo_event_log_path: PathBuf::from(format!("/tmp/otterlink-todo-{}.jsonl", uuid::Uuid::new_v4())),
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
                RuntimeEvent::Agent(NormalizedAgentEvent::AssistantChunk(
                    "让我先搜索一下当前项目。".to_string(),
                )),
                RuntimeEvent::Agent(NormalizedAgentEvent::ToolState {
                    tool_call_id: "call_1".to_string(),
                    state: crate::agent::normalized::AgentToolState::InProgress,
                }),
                RuntimeEvent::Agent(NormalizedAgentEvent::PlanUpdated(vec![TodoEntry {
                    content: "Inspect repository".to_string(),
                    status: "in_progress".to_string(),
                }])),
                RuntimeEvent::Agent(NormalizedAgentEvent::AssistantMessage("Final answer".to_string())),
                RuntimeEvent::Agent(NormalizedAgentEvent::TurnCompleted),
            ],
            sessions: Vec::new(),
            history: Vec::new(),
        });
        let service = build_service(runtime, sink.clone()).await;
        let session = service.registry.resolve("gateway:test", None).await.unwrap();
        let workspace = std::env::current_dir().unwrap().to_string_lossy().to_string();
        let runtime = service
            .persistence
            .create_runtime(
                &runtime_scope_key("codex", &workspace),
                "codex-test",
                "codex",
                &workspace,
                false,
            )
            .await
            .unwrap();
        let _ = service
            .replace_runtime_selection(&session, "codex", &workspace, Some(&runtime.runtime_id), None, None)
            .await
            .unwrap();
        service.activate_runtime(&session, &runtime).await.unwrap();
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
                service.ensure_runtime_selection(&session).await.unwrap(),
                runtime,
            )
            .await
            .unwrap();

        let events = sink.events.lock().await.clone();
        assert!(events.iter().any(|event| event.slot == OutboundSlot::Progress));
        assert!(events.iter().any(|event| event.slot == OutboundSlot::Todo));
        assert!(events.iter().any(|event| event.slot == OutboundSlot::Final));
        assert!(events.iter().any(|event| {
            event.slot == OutboundSlot::Progress
                && matches!(&event.message, OutboundMessage::Text { text } if text.contains("让我先搜索一下当前项目。"))
        }));
        assert!(!events.iter().any(|event| {
            event.slot == OutboundSlot::Progress
                && matches!(&event.message, OutboundMessage::Text { text } if text.contains("Final answer"))
        }));

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
            sessions: Vec::new(),
            history: Vec::new(),
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
        let workspace = std::env::current_dir().unwrap().to_string_lossy().to_string();
        let runtime = service
            .persistence
            .create_runtime(
                &runtime_scope_key("codex", &workspace),
                "codex-child",
                "codex",
                &workspace,
                false,
            )
            .await
            .unwrap();
        let _ = service
            .replace_runtime_selection(&child, "codex", &workspace, Some(&runtime.runtime_id), None, None)
            .await
            .unwrap();
        service.activate_runtime(&child, &runtime).await.unwrap();
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
                service.ensure_runtime_selection(&service.registry.get_by_session_key("session:child").await.unwrap()).await.unwrap(),
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
        render.pending_progress_text = "first line\nsecond line".to_string();
        render.current_assistant_text = "line one\nline two\nline three".to_string();
        render.final_text = Some("line one\nline two\nline three".to_string());

        let progress = render.progress_message();
        let todo = render.todo_message();
        let final_message = render.final_message();

        match progress {
            OutboundMessage::Text { text } => {
                assert!(text.contains("first line"));
                assert!(text.contains("second line"));
            }
            _ => panic!("progress should be text"),
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
                assert_eq!(card.title, "✅ 本轮已结束");
                let body = format!("{:?}", card.blocks);
                assert!(body.contains("line one"));
                assert!(!body.contains("📌"));
                assert!(!body.contains("📝"));
                assert!(!body.contains("session"));
            }
            _ => panic!("final should be card"),
        }
    }

    #[test]
    fn finalize_uses_latest_assistant_segment_instead_of_all_progress() {
        let session = SessionInfo {
            session_id: "sess_demo".to_string(),
            session_key: "gateway:test".to_string(),
            parent_session_id: None,
            runtime_session_ref: None,
            last_assistant_message: None,
        };
        let mut render = TurnRenderState::new(&session);
        render.pending_progress_text = "让我先搜索".to_string();
        render.current_assistant_text = "让我先搜索".to_string();
        render.apply_event(NormalizedAgentEvent::ToolState {
            tool_call_id: "call_1".to_string(),
            state: crate::agent::normalized::AgentToolState::InProgress,
        });
        render.apply_event(NormalizedAgentEvent::AssistantChunk("最终答案第一段。".to_string()));
        render.apply_event(NormalizedAgentEvent::AssistantChunk("最终答案第二段。".to_string()));
        render.finalize();

        assert_eq!(
            render.final_assistant_message().as_deref(),
            Some("最终答案第一段。最终答案第二段。")
        );
    }

    #[test]
    fn progress_is_only_queued_when_assistant_segment_is_interrupted_by_tool() {
        let session = SessionInfo {
            session_id: "sess_demo".to_string(),
            session_key: "gateway:test".to_string(),
            parent_session_id: None,
            runtime_session_ref: None,
            last_assistant_message: None,
        };
        let mut render = TurnRenderState::new(&session);
        render.apply_event(NormalizedAgentEvent::AssistantChunk("让我".to_string()));
        render.apply_event(NormalizedAgentEvent::AssistantChunk("先搜索".to_string()));
        assert!(!render.has_progress_update());

        render.apply_event(NormalizedAgentEvent::ToolState {
            tool_call_id: "call_1".to_string(),
            state: crate::agent::normalized::AgentToolState::InProgress,
        });
        assert!(render.has_progress_update());
        match render.progress_message() {
            OutboundMessage::Text { text } => assert_eq!(text, "让我先搜索"),
            _ => panic!("progress should be text"),
        }
    }

    #[tokio::test]
    async fn control_can_create_and_switch_runtimes() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
            sessions: Vec::new(),
            history: Vec::new(),
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
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();
        assert!(show.selector.is_some());
        assert!(show.active_runtime.is_none());

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
                proxy_mode: None,
                proxy_url: None,
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
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();
        assert_eq!(switched.active_runtime.as_ref().unwrap().label, "claude-alt");
        assert!(switched.runtimes.iter().any(|runtime| runtime.label == "claude-alt"));
    }

    #[tokio::test]
    async fn inbound_help_returns_immediate_reply() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
            sessions: Vec::new(),
            history: Vec::new(),
        });
        let service = build_service(runtime, sink).await;

        let response = service
            .handle_inbound(crate::core::inbound::CoreInboundRequest {
                session_key: "control:help".to_string(),
                parent_session_key: None,
                text: "/ot help".to_string(),
            })
            .await
            .unwrap();

        assert!(response.turn_id.is_none());
        assert_eq!(response.replies.len(), 1);
        assert!(!response.react_to_message);
    }

    #[tokio::test]
    async fn inbound_plain_message_accepts_turn_and_generates_turn_id() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
            sessions: Vec::new(),
            history: Vec::new(),
        });
        let service = build_service(runtime, sink).await;

        let current_dir = std::env::current_dir().unwrap();
        let _ = service
            .handle_control(CoreControlRequest {
                session_key: "control:plain".to_string(),
                parent_session_key: None,
                action: ControlAction::CreateRuntime,
                runtime_selector: None,
                workspace_path: Some(current_dir.to_string_lossy().to_string()),
                label: Some("codex-plain".to_string()),
                agent_kind: Some("codex".to_string()),
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();

        let response = service
            .handle_inbound(crate::core::inbound::CoreInboundRequest {
                session_key: "control:plain".to_string(),
                parent_session_key: None,
                text: "hello".to_string(),
            })
            .await
            .unwrap();

        assert!(response.turn_id.as_deref().unwrap_or_default().starts_with("turn_"));
        assert!(response.replies.is_empty());
        assert!(response.react_to_message);
    }

    #[tokio::test]
    async fn inbound_plain_message_steers_active_codex_turn() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(SteeringMockRuntime::default());
        let service = build_service(runtime.clone(), sink).await;
        let workspace = "/tmp/codex-steer-workspace";
        let session_key = "control:codex-steer";

        let session = service.registry.resolve(session_key, None).await.unwrap();
        service
            .persistence
            .import_runtime(
                &runtime_scope_key("codex", workspace),
                "codex-live",
                "codex",
                workspace,
                "thread-codex-1",
                None,
                Some("active codex thread"),
                false,
            )
            .await
            .unwrap();
        let runtime_instance = service
            .persistence
            .list_runtimes(&runtime_scope_key("codex", workspace))
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        service
            .replace_runtime_selection(
                &session,
                "codex",
                workspace,
                Some(&runtime_instance.runtime_id),
                None,
                None,
            )
            .await
            .unwrap();
        service
            .active_turns
            .set_starting(session_key, "turn_active_codex")
            .await;
        let (cancel, _cancel_rx) = crate::agent::runtime::RuntimeCancelHandle::new();
        service
            .active_turns
            .attach_cancel(
                session_key,
                "turn_active_codex",
                Some("codex"),
                Some("thread-codex-1"),
                Some("turn-codex-1"),
                cancel,
            )
            .await;

        let response = service
            .handle_inbound(crate::core::inbound::CoreInboundRequest {
                session_key: session_key.to_string(),
                parent_session_key: None,
                text: "在实现之前，先检查一下当前目录结构。".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(response.turn_id.as_deref(), Some("turn_active_codex"));
        assert_eq!(response.replies.len(), 1);
        assert!(!response.react_to_message);
        let steer_requests = runtime.steer_requests.lock().await.clone();
        assert_eq!(steer_requests.len(), 1);
        assert_eq!(steer_requests[0].runtime_session_ref, "thread-codex-1");
        assert_eq!(steer_requests[0].runtime_turn_ref, "turn-codex-1");
        assert_eq!(steer_requests[0].prompt, "在实现之前，先检查一下当前目录结构。");
    }

    #[tokio::test]
    async fn accept_turn_rejection_message_treats_agent_and_workspace_as_parallel_prerequisites() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
            sessions: Vec::new(),
            history: Vec::new(),
        });
        let service = build_service(runtime, sink).await;

        let err = service
            .accept_turn(CoreTurnRequest {
                turn_id: "turn_missing_runtime".to_string(),
                session_key: "control:missing-runtime".to_string(),
                parent_session_key: None,
                text: "hello".to_string(),
            })
            .await
            .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("当前还没有选定会话"));
        assert!(message.contains("可以按任意顺序调整"));
        assert!(message.contains("/ot cwd <path>"));
        assert!(message.contains("/ot pick <short_id>` 或 `/ot new`"));
    }

    #[tokio::test]
    async fn switch_runtime_accepts_short_runtime_prefix() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
            sessions: Vec::new(),
            history: Vec::new(),
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
                proxy_mode: None,
                proxy_url: None,
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
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();

        assert_eq!(switched.active_runtime.as_ref().unwrap().label, "claude-short");
    }

    #[tokio::test]
    async fn switch_runtime_returns_history_overview() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
            sessions: Vec::new(),
            history: vec![
                crate::agent::runtime::RuntimeHistoryTurn {
                    user_text: "第一个问题".to_string(),
                    assistant_text: "第一个回答".to_string(),
                },
                crate::agent::runtime::RuntimeHistoryTurn {
                    user_text: "第二个问题".to_string(),
                    assistant_text: "第二个回答".to_string(),
                },
                crate::agent::runtime::RuntimeHistoryTurn {
                    user_text: "第三个问题".to_string(),
                    assistant_text: "第三个回答".to_string(),
                },
                crate::agent::runtime::RuntimeHistoryTurn {
                    user_text: "第四个问题".to_string(),
                    assistant_text: "第四个回答".to_string(),
                },
                crate::agent::runtime::RuntimeHistoryTurn {
                    user_text: "第五个问题".to_string(),
                    assistant_text: "第五个回答".to_string(),
                },
                crate::agent::runtime::RuntimeHistoryTurn {
                    user_text: "第六个问题".to_string(),
                    assistant_text: "第六个回答".to_string(),
                },
            ],
        });
        let service = build_service(runtime, sink).await;
        let current_dir = std::env::current_dir().unwrap();
        let created = service
            .handle_control(CoreControlRequest {
                session_key: "control:history".to_string(),
                parent_session_key: None,
                action: ControlAction::CreateRuntime,
                runtime_selector: None,
                workspace_path: Some(current_dir.to_string_lossy().to_string()),
                label: Some("codex-history".to_string()),
                agent_kind: Some("codex".to_string()),
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();
        let runtime_id = created.active_runtime.as_ref().unwrap().runtime_id.clone();
        service
            .persistence
            .update_runtime_state(&runtime_id, Some("sess-history"), None)
            .await
            .unwrap();

        let switched = service
            .handle_control(CoreControlRequest {
                session_key: "control:history".to_string(),
                parent_session_key: None,
                action: ControlAction::SwitchRuntime,
                runtime_selector: Some(runtime_id),
                workspace_path: None,
                label: None,
                agent_kind: None,
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();

        let history = switched.history_overview.expect("history overview missing");
        assert_eq!(history.runtime_session_ref, "sess-history");
        assert_eq!(history.turns.len(), 5);
        assert_eq!(history.turns[0].user_text, "第二个问题");
        assert_eq!(history.turns[4].assistant_text, "第六个回答");
    }

    #[tokio::test]
    async fn switch_runtime_ignores_history_overview_failure() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(HistoryUnavailableMockRuntime);
        let service = build_service(runtime, sink).await;
        let current_dir = std::env::current_dir().unwrap();
        let created = service
            .handle_control(CoreControlRequest {
                session_key: "control:history-missing".to_string(),
                parent_session_key: None,
                action: ControlAction::CreateRuntime,
                runtime_selector: None,
                workspace_path: Some(current_dir.to_string_lossy().to_string()),
                label: Some("codex-history-missing".to_string()),
                agent_kind: Some("codex".to_string()),
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();
        let runtime_id = created.active_runtime.as_ref().unwrap().runtime_id.clone();
        service
            .persistence
            .update_runtime_state(&runtime_id, Some("sess-history-missing"), None)
            .await
            .unwrap();

        let switched = service
            .handle_control(CoreControlRequest {
                session_key: "control:history-missing".to_string(),
                parent_session_key: None,
                action: ControlAction::SwitchRuntime,
                runtime_selector: Some(runtime_id),
                workspace_path: None,
                label: None,
                agent_kind: None,
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();

        assert!(switched.ok);
        assert_eq!(
            switched.active_runtime.as_ref().map(|item| item.label.as_str()),
            Some("codex-history-missing")
        );
        assert!(switched.history_overview.is_none());
    }

    #[tokio::test]
    async fn control_can_load_claude_runtimes_from_workspace_jsonl() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
            sessions: Vec::new(),
            history: Vec::new(),
        });
        let root = PathBuf::from(format!("/tmp/otterlink-load-{}", uuid::Uuid::new_v4()));
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

        let config = test_config_with_paths(workspace.clone(), claude_home, root.join(".codex"));
        let service = build_service_with_config(runtime, sink, config).await;
        let discovered = service.discover_claude_sessions(&workspace).await.unwrap();
        assert_eq!(discovered.len(), 2);
        service
            .handle_control(CoreControlRequest {
                session_key: "control:load".to_string(),
                parent_session_key: None,
                action: ControlAction::UseAgent,
                runtime_selector: None,
                workspace_path: None,
                label: None,
                agent_kind: Some("claude_code".to_string()),
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();
        let response = service
            .handle_control(CoreControlRequest {
                session_key: "control:load".to_string(),
                parent_session_key: None,
                action: ControlAction::LoadRuntimes,
                runtime_selector: None,
                workspace_path: Some(workspace.to_string_lossy().to_string()),
                label: None,
                agent_kind: None,
                proxy_mode: None,
                proxy_url: None,
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

    #[tokio::test]
    async fn control_prefers_runtime_list_sessions_over_local_fallback() {
        let sink = Arc::new(MockSink::default());
        let root = PathBuf::from(format!("/tmp/otterlink-runtime-list-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        let claude_home = root.join(".claude");
        std::fs::create_dir_all(&workspace).unwrap();
        let project_dir = claude_home
            .join("projects")
            .join(claude_project_dir_name(&workspace));
        std::fs::create_dir_all(&project_dir).unwrap();
        write_claude_jsonl(
            &project_dir.join("sess-local.jsonl"),
            "sess-local",
            &workspace,
            "master",
            "local fallback session",
        );

        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
            sessions: vec![crate::agent::runtime::RuntimeSessionListing {
                runtime_session_ref: "sess-acp".to_string(),
                workspace_path: workspace.to_string_lossy().to_string(),
                title: Some("acp listed session".to_string()),
                updated_at: None,
            }],
            history: Vec::new(),
        });
        let config = test_config_with_paths(workspace.clone(), claude_home, root.join(".codex"));
        let service = build_service_with_config(runtime, sink, config).await;

        service
            .handle_control(CoreControlRequest {
                session_key: "control:runtime-list".to_string(),
                parent_session_key: None,
                action: ControlAction::UseAgent,
                runtime_selector: None,
                workspace_path: None,
                label: None,
                agent_kind: Some("claude_code".to_string()),
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();
        let response = service
            .handle_control(CoreControlRequest {
                session_key: "control:runtime-list".to_string(),
                parent_session_key: None,
                action: ControlAction::LoadRuntimes,
                runtime_selector: None,
                workspace_path: Some(workspace.to_string_lossy().to_string()),
                label: None,
                agent_kind: None,
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();

        assert!(response
            .runtimes
            .iter()
            .any(|runtime| runtime.runtime_session_ref.as_deref() == Some("sess-acp")));
        assert!(!response
            .runtimes
            .iter()
            .any(|runtime| runtime.runtime_session_ref.as_deref() == Some("sess-local")));
    }

    #[test]
    fn claude_session_discovery_falls_back_to_jsonl_without_index() {
        let root = PathBuf::from(format!("/tmp/otterlink-discovery-{}", uuid::Uuid::new_v4()));
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
        assert_eq!(sessions[0].tag.as_deref(), Some("master"));
    }

    #[tokio::test]
    async fn control_can_load_codex_runtimes_from_state_sqlite() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
            sessions: Vec::new(),
            history: Vec::new(),
        });
        let root = PathBuf::from(format!("/tmp/otterlink-codex-load-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        let claude_home = root.join(".claude");
        let codex_home = root.join(".codex");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&codex_home).unwrap();
        write_codex_state_db(&codex_home.join("state_5.sqlite"), &workspace);

        let config = test_config_with_paths(workspace.clone(), claude_home, codex_home);
        let service = build_service_with_config(runtime, sink, config).await;
        let discovered = service.discover_codex_sessions(&workspace).await.unwrap();
        assert_eq!(discovered.len(), 2);
        assert_eq!(discovered[0].runtime_session_ref, "codex-b");
        assert_eq!(discovered[1].runtime_session_ref, "codex-a");

        service
            .handle_control(CoreControlRequest {
                session_key: "control:codex-load".to_string(),
                parent_session_key: None,
                action: ControlAction::UseAgent,
                runtime_selector: None,
                workspace_path: None,
                label: None,
                agent_kind: Some("codex".to_string()),
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();
        let response = service
            .handle_control(CoreControlRequest {
                session_key: "control:codex-load".to_string(),
                parent_session_key: None,
                action: ControlAction::LoadRuntimes,
                runtime_selector: None,
                workspace_path: Some(workspace.to_string_lossy().to_string()),
                label: None,
                agent_kind: None,
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();

        assert!(response.ok);
        assert_eq!(response.runtimes.len(), 2);
        assert!(response
            .runtimes
            .iter()
            .any(|runtime| runtime.runtime_session_ref.as_deref() == Some("codex-a")));
        assert!(response
            .runtimes
            .iter()
            .any(|runtime| runtime.prompt_preview.as_deref() == Some("first codex task")));
    }

    #[tokio::test]
    async fn control_can_update_proxy_mode_for_selector() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
            sessions: Vec::new(),
            history: Vec::new(),
        });
        let service = build_service(runtime, sink).await;

        let response = service
            .handle_control(CoreControlRequest {
                session_key: "control:proxy".to_string(),
                parent_session_key: None,
                action: ControlAction::SetProxy,
                runtime_selector: None,
                workspace_path: None,
                label: None,
                agent_kind: None,
                proxy_mode: Some("on".to_string()),
                proxy_url: Some("http://127.0.0.1:8888".to_string()),
            })
            .await
            .unwrap();

        assert_eq!(response.selector.as_ref().unwrap().proxy_mode, "on");
        assert_eq!(
            response.selector.as_ref().unwrap().proxy_url.as_deref(),
            Some("http://127.0.0.1:8888")
        );
    }

    #[tokio::test]
    async fn control_strips_markdown_wrapper_from_proxy_url() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(MockRuntime {
            captured: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
            sessions: Vec::new(),
            history: Vec::new(),
        });
        let service = build_service(runtime, sink).await;

        let response = service
            .handle_control(CoreControlRequest {
                session_key: "control:proxy-markdown".to_string(),
                parent_session_key: None,
                action: ControlAction::SetProxy,
                runtime_selector: None,
                workspace_path: None,
                label: None,
                agent_kind: None,
                proxy_mode: Some("on".to_string()),
                proxy_url: Some("[http://127.0.0.1:7890](http://127.0.0.1:7890/)".to_string()),
            })
            .await
            .unwrap();

        assert_eq!(response.selector.as_ref().unwrap().proxy_mode, "on");
        assert_eq!(
            response.selector.as_ref().unwrap().proxy_url.as_deref(),
            Some("http://127.0.0.1:7890")
        );
    }

    #[tokio::test]
    async fn control_can_stop_active_turn() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(InterruptibleMockRuntime);
        let service = build_service(runtime, sink).await;
        let session = service.registry.resolve("control:stop", None).await.unwrap();
        let workspace = std::env::current_dir().unwrap().to_string_lossy().to_string();
        let runtime = service
            .persistence
            .create_runtime(
                &runtime_scope_key("codex", &workspace),
                "codex-stop",
                "codex",
                &workspace,
                false,
            )
            .await
            .unwrap();
        let _ = service
            .replace_runtime_selection(&session, "codex", &workspace, Some(&runtime.runtime_id), None, None)
            .await
            .unwrap();

        service
            .accept_turn(CoreTurnRequest {
                turn_id: "turn_stop".to_string(),
                session_key: "control:stop".to_string(),
                parent_session_key: None,
                text: "long task".to_string(),
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let response = service
            .handle_control(CoreControlRequest {
                session_key: "control:stop".to_string(),
                parent_session_key: None,
                action: ControlAction::StopRuntime,
                runtime_selector: None,
                workspace_path: None,
                label: None,
                agent_kind: None,
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();

        assert!(response.ok);
        assert!(response.message.contains("已请求停止当前任务"));

        tokio::time::sleep(Duration::from_millis(50)).await;
        let turn = service.persistence.get_turn("turn_stop").await.unwrap().unwrap();
        assert_eq!(turn.status, "failed");
        assert!(turn.error_text.as_deref().unwrap_or_default().contains("interrupted"));
    }

    #[tokio::test]
    async fn control_can_stop_turn_while_runtime_is_starting() {
        let sink = Arc::new(MockSink::default());
        let runtime = Arc::new(SlowInterruptibleMockRuntime);
        let service = build_service(runtime, sink).await;
        let session = service.registry.resolve("control:stop-starting", None).await.unwrap();
        let workspace = std::env::current_dir().unwrap().to_string_lossy().to_string();
        let runtime = service
            .persistence
            .create_runtime(
                &runtime_scope_key("codex", &workspace),
                "codex-stop-starting",
                "codex",
                &workspace,
                false,
            )
            .await
            .unwrap();
        let _ = service
            .replace_runtime_selection(&session, "codex", &workspace, Some(&runtime.runtime_id), None, None)
            .await
            .unwrap();

        service
            .accept_turn(CoreTurnRequest {
                turn_id: "turn_stop_starting".to_string(),
                session_key: "control:stop-starting".to_string(),
                parent_session_key: None,
                text: "long task".to_string(),
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        let response = service
            .handle_control(CoreControlRequest {
                session_key: "control:stop-starting".to_string(),
                parent_session_key: None,
                action: ControlAction::StopRuntime,
                runtime_selector: None,
                workspace_path: None,
                label: None,
                agent_kind: None,
                proxy_mode: None,
                proxy_url: None,
            })
            .await
            .unwrap();

        assert!(response.ok);
        assert!(response.message.contains("已请求停止当前任务"));

        tokio::time::sleep(Duration::from_millis(200)).await;
        let turn = service
            .persistence
            .get_turn("turn_stop_starting")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(turn.status, "failed");
        assert!(turn.error_text.as_deref().unwrap_or_default().contains("interrupted"));
    }

    #[test]
    fn expand_workspace_input_supports_tilde_paths() {
        let fake_home = PathBuf::from(format!("/tmp/otterlink-home-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(fake_home.join("demo")).unwrap();
        std::env::set_var("HOME", &fake_home);

        let resolved = expand_workspace_input("~/demo").unwrap();
        assert_eq!(resolved, fake_home.join("demo"));
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

    fn write_codex_state_db(path: &Path, workspace: &Path) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                source TEXT NOT NULL,
                model_provider TEXT NOT NULL,
                cwd TEXT NOT NULL,
                title TEXT NOT NULL,
                sandbox_policy TEXT NOT NULL,
                approval_mode TEXT NOT NULL,
                tokens_used INTEGER NOT NULL DEFAULT 0,
                has_user_event INTEGER NOT NULL DEFAULT 0,
                archived INTEGER NOT NULL DEFAULT 0,
                archived_at INTEGER,
                git_sha TEXT,
                git_branch TEXT,
                git_origin_url TEXT,
                cli_version TEXT NOT NULL DEFAULT '',
                first_user_message TEXT NOT NULL DEFAULT '',
                agent_nickname TEXT,
                agent_role TEXT,
                memory_mode TEXT NOT NULL DEFAULT 'enabled'
            );
            "#,
        )
        .unwrap();
        let workspace = workspace.to_string_lossy().to_string();
        conn.execute(
            r#"
            INSERT INTO threads (
                id, rollout_path, created_at, updated_at, source, model_provider, cwd, title,
                sandbox_policy, approval_mode, tokens_used, has_user_event, archived, git_branch,
                cli_version, first_user_message, memory_mode
            ) VALUES (?1, '', 1, 10, 'cli', 'openai', ?2, 'mcdataset', 'danger-full-access',
                      'never', 0, 1, 0, 'master', '0.107.0', 'first codex task', 'enabled')
            "#,
            rusqlite::params!["codex-a", workspace],
        )
        .unwrap();
        conn.execute(
            r#"
            INSERT INTO threads (
                id, rollout_path, created_at, updated_at, source, model_provider, cwd, title,
                sandbox_policy, approval_mode, tokens_used, has_user_event, archived, git_branch,
                cli_version, first_user_message, memory_mode
            ) VALUES (?1, '', 1, 20, 'cli', 'openai', ?2, 'mcdataset 2', 'danger-full-access',
                      'never', 0, 1, 0, 'feature/demo', '0.107.0', 'second codex task', 'enabled')
            "#,
            rusqlite::params!["codex-b", workspace],
        )
        .unwrap();
        conn.execute(
            r#"
            INSERT INTO threads (
                id, rollout_path, created_at, updated_at, source, model_provider, cwd, title,
                sandbox_policy, approval_mode, tokens_used, has_user_event, archived, git_branch,
                cli_version, first_user_message, memory_mode
            ) VALUES (?1, '', 1, 30, 'cli', 'openai', '/tmp/elsewhere', 'other', 'danger-full-access',
                      'never', 0, 1, 0, 'other', '0.107.0', 'ignore me', 'enabled')
            "#,
            rusqlite::params!["codex-other"],
        )
        .unwrap();
    }
}
