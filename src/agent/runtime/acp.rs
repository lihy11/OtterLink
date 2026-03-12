// ACP references:
// - Initialization: https://agentclientprotocol.com/protocol/initialization
// - Session setup (`session/new`, `session/load`): https://agentclientprotocol.com/protocol/session-setup
// - Prompt turns / completion / cancellation: https://agentclientprotocol.com/protocol/prompt-turn
// - Session modes: https://agentclientprotocol.com/protocol/session-modes
// - Session listing (`session/list`, unstable): https://agentclientprotocol.com/protocol/session-list
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as StdMutex,
    },
};

use agent_client_protocol::{
    Agent, CancelNotification, Client as AcpClientTrait, ClientSideConnection, ContentBlock,
    Implementation, InitializeRequest, InitializeResponse, ListSessionsRequest, LoadSessionRequest,
    NewSessionRequest, PermissionOptionKind, PromptRequest, ProtocolVersion,
    RequestPermissionOutcome, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionModeState, SessionUpdate, SetSessionModeRequest,
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    sync::{mpsc, oneshot, watch, Mutex},
    time::{Duration, Sleep},
};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing::{info, warn};

use crate::{
    agent::{
        normalized::normalize_acp_update,
        runtime::{
            adapters::{self, AcpAdapterSpec}, AgentRuntime, RuntimeCompletion, RuntimeEvent,
            RuntimeHistoryQuery, RuntimeHistoryTurn, RuntimeSessionListing, RuntimeSessionQuery,
            RuntimeTurn, RuntimeTurnRequest,
        },
    },
    config::Config,
};

fn acp_stop_reason_label(stop_reason: agent_client_protocol::StopReason) -> &'static str {
    match stop_reason {
        agent_client_protocol::StopReason::EndTurn => "end_turn",
        agent_client_protocol::StopReason::MaxTokens => "max_tokens",
        agent_client_protocol::StopReason::MaxTurnRequests => "max_turn_requests",
        agent_client_protocol::StopReason::Refusal => "refusal",
        agent_client_protocol::StopReason::Cancelled => "cancelled",
        _ => "unknown",
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct WorkerKey {
    adapter_id: String,
    workspace_path: PathBuf,
    proxy_mode: String,
    proxy_url: Option<String>,
}

#[derive(Clone)]
struct AcpWorkerHandle {
    tx: mpsc::UnboundedSender<WorkerCommand>,
}

enum WorkerCommand {
    StartTurn {
        request: RuntimeTurnRequest,
        events_tx: mpsc::UnboundedSender<RuntimeEvent>,
        cancel_rx: watch::Receiver<bool>,
        response_tx: oneshot::Sender<Result<RuntimeCompletion>>,
    },
    ListSessions {
        workspace_path: PathBuf,
        response_tx: oneshot::Sender<Result<Vec<RuntimeSessionListing>>>,
    },
    LoadHistory {
        runtime_session_ref: String,
        response_tx: oneshot::Sender<Result<Vec<RuntimeHistoryTurn>>>,
    },
}

struct AcpRuntimeProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr_task: tokio::task::JoinHandle<()>,
    adapter: AcpAdapterSpec,
}

struct AcpWorkerState {
    key: WorkerKey,
    child: Child,
    stderr_task: tokio::task::JoinHandle<()>,
    adapter: AcpAdapterSpec,
    conn: ClientSideConnection,
    initialize: InitializeResponse,
    updates_rx: mpsc::UnboundedReceiver<SessionUpdate>,
    cancelled_flag: Arc<StdMutex<Arc<AtomicBool>>>,
    loaded_sessions: HashSet<String>,
    session_modes: HashMap<String, SessionModeState>,
    history_cache: HashMap<String, Vec<RuntimeHistoryTurn>>,
}

pub struct AcpRuntime {
    config: Arc<Config>,
    workers: Arc<Mutex<HashMap<WorkerKey, AcpWorkerHandle>>>,
}

impl AcpRuntime {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            workers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn worker_for_turn(&self, request: &RuntimeTurnRequest) -> Result<AcpWorkerHandle> {
        let adapter_id = request
            .agent_kind
            .as_deref()
            .unwrap_or(&self.config.acp_adapter);
        let workspace_path = request
            .workspace_path
            .clone()
            .unwrap_or_else(|| self.config.codex_workdir.clone());
        let key = build_worker_key(
            adapter_id,
            workspace_path,
            request.proxy_mode.as_deref(),
            request.proxy_url.as_deref(),
            self.config.acp_proxy_url.as_deref(),
            self.config.default_proxy_mode_for_agent(adapter_id),
        );
        self.worker_for_key(key).await
    }

    async fn worker_for_query(&self, query: &RuntimeSessionQuery) -> Result<AcpWorkerHandle> {
        let adapter_id = query
            .agent_kind
            .as_deref()
            .unwrap_or(&self.config.acp_adapter);
        let key = build_worker_key(
            adapter_id,
            query.workspace_path.clone(),
            query.proxy_mode.as_deref(),
            query.proxy_url.as_deref(),
            self.config.acp_proxy_url.as_deref(),
            self.config.default_proxy_mode_for_agent(adapter_id),
        );
        self.worker_for_key(key).await
    }

    async fn worker_for_key(&self, key: WorkerKey) -> Result<AcpWorkerHandle> {
        let mut guard = self.workers.lock().await;
        if let Some(handle) = guard.get(&key) {
            return Ok(handle.clone());
        }

        let handle = spawn_worker(self.config.clone(), key.clone())?;
        guard.insert(key, handle.clone());
        Ok(handle)
    }
}

#[async_trait]
impl AgentRuntime for AcpRuntime {
    async fn start_turn(&self, request: RuntimeTurnRequest) -> Result<RuntimeTurn> {
        let worker = self.worker_for_turn(&request).await?;
        let (cancel, cancel_rx) = crate::agent::runtime::RuntimeCancelHandle::new();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (response_tx, response_rx) = oneshot::channel();
        worker
            .tx
            .send(WorkerCommand::StartTurn {
                request,
                events_tx,
                cancel_rx,
                response_tx,
            })
            .map_err(|_| anyhow!("acp worker command channel closed"))?;

        let completion = tokio::spawn(async move {
            response_rx
                .await
                .map_err(|_| anyhow!("acp worker response channel closed"))?
        });

        Ok(RuntimeTurn {
            events: events_rx,
            completion,
            cancel,
        })
    }

    async fn list_sessions(&self, query: RuntimeSessionQuery) -> Result<Vec<RuntimeSessionListing>> {
        let worker = self.worker_for_query(&query).await?;
        let (response_tx, response_rx) = oneshot::channel();
        worker
            .tx
            .send(WorkerCommand::ListSessions {
                workspace_path: query.workspace_path,
                response_tx,
            })
            .map_err(|_| anyhow!("acp worker command channel closed"))?;
        response_rx
            .await
            .map_err(|_| anyhow!("acp worker response channel closed"))?
    }

    async fn load_history(&self, query: RuntimeHistoryQuery) -> Result<Vec<RuntimeHistoryTurn>> {
        let worker = self.worker_for_query(&RuntimeSessionQuery {
            agent_kind: query.agent_kind,
            workspace_path: query.workspace_path,
            proxy_mode: query.proxy_mode,
            proxy_url: query.proxy_url,
        }).await?;
        let (response_tx, response_rx) = oneshot::channel();
        worker
            .tx
            .send(WorkerCommand::LoadHistory {
                runtime_session_ref: query.runtime_session_ref,
                response_tx,
            })
            .map_err(|_| anyhow!("acp worker command channel closed"))?;
        response_rx
            .await
            .map_err(|_| anyhow!("acp worker response channel closed"))?
    }

    fn name(&self) -> &'static str {
        "acp"
    }
}

fn spawn_worker(config: Arc<Config>, key: WorkerKey) -> Result<AcpWorkerHandle> {
    let (tx, rx) = mpsc::unbounded_channel();
    let thread_name = format!(
        "acp-worker-{}-{}",
        key.adapter_id,
        key.workspace_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("workspace")
    );
    std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build acp worker runtime failed");
            let local = tokio::task::LocalSet::new();
            rt.block_on(local.run_until(async move {
                if let Err(err) = run_worker(config, key, rx).await {
                    warn!("acp worker exited: {err:?}");
                }
            }));
        })
        .context("spawn acp worker thread failed")?;
    Ok(AcpWorkerHandle { tx })
}

async fn run_worker(
    config: Arc<Config>,
    key: WorkerKey,
    mut rx: mpsc::UnboundedReceiver<WorkerCommand>,
) -> Result<()> {
    let mut state = connect_worker(&config, &key).await?;
    while let Some(command) = rx.recv().await {
        match command {
            WorkerCommand::StartTurn {
                request,
                events_tx,
                cancel_rx,
                response_tx,
            } => {
                let result = state.handle_turn(request, events_tx, cancel_rx).await;
                let _ = response_tx.send(result);
            }
            WorkerCommand::ListSessions {
                workspace_path,
                response_tx,
            } => {
                let result = state.list_sessions(workspace_path).await;
                let _ = response_tx.send(result);
            }
            WorkerCommand::LoadHistory {
                runtime_session_ref,
                response_tx,
            } => {
                let result = state.load_history(runtime_session_ref).await;
                let _ = response_tx.send(result);
            }
        }
    }
    state.shutdown().await;
    Ok(())
}

async fn connect_worker(config: &Arc<Config>, key: &WorkerKey) -> Result<AcpWorkerState> {
    let AcpRuntimeProcess {
        child,
        stdin,
        stdout,
        stderr_task,
        adapter,
    } = spawn_process(config, key).await?;

    let (updates_tx, updates_rx) = mpsc::unbounded_channel::<SessionUpdate>();
    let cancelled_flag = Arc::new(StdMutex::new(Arc::new(AtomicBool::new(false))));
    let client = AcpBridgeClient {
        updates_tx,
        cancelled_flag: cancelled_flag.clone(),
    };
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

    let initialize = conn
        .initialize(
            InitializeRequest::new(ProtocolVersion::LATEST)
                .client_info(Implementation::new("feishu-acp-bridge", "0.1.0")),
        )
        .await
        .map_err(|e| anyhow!("acp initialize failed: {e:?}"))?;

    info!(
        "acp worker connected: adapter={} cwd={} load_session={}",
        key.adapter_id,
        key.workspace_path.display(),
        initialize.agent_capabilities.load_session
    );

    Ok(AcpWorkerState {
        key: key.clone(),
        child,
        stderr_task,
        adapter,
        conn,
        initialize,
        updates_rx,
        cancelled_flag,
        loaded_sessions: HashSet::new(),
        session_modes: HashMap::new(),
        history_cache: HashMap::new(),
    })
}

impl AcpWorkerState {
    async fn handle_turn(
        &mut self,
        request: RuntimeTurnRequest,
        events_tx: mpsc::UnboundedSender<RuntimeEvent>,
        mut cancel_rx: watch::Receiver<bool>,
    ) -> Result<RuntimeCompletion> {
        let cancel_flag = Arc::new(AtomicBool::new(false));
        {
            let mut guard = self.cancelled_flag.lock().unwrap();
            *guard = cancel_flag.clone();
        }

        let session_id = self.ensure_session(&request, &events_tx).await?;
        let mut prompt_fut = Box::pin(self.conn.prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::from(request.prompt)],
        )));
        let mut cancel_requested = false;
        let mut cancel_deadline: Option<std::pin::Pin<Box<Sleep>>> = None;

        loop {
            tokio::select! {
                changed = cancel_rx.changed(), if !cancel_requested => {
                    if changed.is_ok() && *cancel_rx.borrow() {
                        cancel_flag.store(true, Ordering::SeqCst);
                        let _ = self.conn.cancel(CancelNotification::new(session_id.clone())).await;
                        cancel_requested = true;
                        cancel_deadline = Some(Box::pin(tokio::time::sleep(Duration::from_secs(8))));
                    }
                }
                _ = async {
                    if let Some(deadline) = cancel_deadline.as_mut() {
                        deadline.as_mut().await;
                    }
                }, if cancel_deadline.is_some() => {
                    return Err(anyhow!(super::types::INTERRUPTED_ERROR_TEXT));
                }
                maybe_upd = self.updates_rx.recv() => {
                    if let Some(upd) = maybe_upd {
                        for event in normalize_acp_update(upd) {
                            let _ = events_tx.send(RuntimeEvent::Agent(event));
                        }
                    } else {
                        return Err(anyhow!("acp updates channel closed before prompt completed"));
                    }
                }
                resp = &mut prompt_fut => {
                    let response = match resp {
                        Ok(response) => response,
                        Err(err) => {
                            let text = format!("{err:?}");
                            if cancel_requested && text.contains("Request cancelled") {
                                return Err(anyhow!(super::types::INTERRUPTED_ERROR_TEXT));
                            }
                            return Err(anyhow!("acp prompt failed: {err:?}"));
                        }
                    };
                    while let Ok(upd) = self.updates_rx.try_recv() {
                        for event in normalize_acp_update(upd) {
                            let _ = events_tx.send(RuntimeEvent::Agent(event));
                        }
                    }
                    let stop_reason = response.stop_reason;
                    let stop_reason_label = acp_stop_reason_label(stop_reason).to_string();
                    info!(
                        "acp prompt completed: adapter={} session_id={} stop_reason={}",
                        self.adapter.id,
                        session_id,
                        stop_reason_label
                    );
                    let _ = events_tx.send(RuntimeEvent::Agent(
                        crate::agent::normalized::NormalizedAgentEvent::TurnCompleted,
                    ));
                    if stop_reason == agent_client_protocol::StopReason::Cancelled {
                        return Err(anyhow!(super::types::INTERRUPTED_ERROR_TEXT));
                    }
                    return Ok(RuntimeCompletion {
                        stderr_summary: None,
                        stop_reason: Some(stop_reason_label),
                    });
                }
            }
        }
    }

    async fn ensure_session(
        &mut self,
        request: &RuntimeTurnRequest,
        events_tx: &mpsc::UnboundedSender<RuntimeEvent>,
    ) -> Result<String> {
        let session_id = if let Some(runtime_session_ref) = request.runtime_session_ref.clone() {
            if !self.loaded_sessions.contains(&runtime_session_ref) {
                if self.initialize.agent_capabilities.load_session {
                    info!(
                        "acp loading session: adapter={} session_id={} cwd={}",
                        self.adapter.id,
                        runtime_session_ref,
                        self.key.workspace_path.display()
                    );
                    let response = self
                        .conn
                        .load_session(LoadSessionRequest::new(runtime_session_ref.clone(), &self.key.workspace_path))
                        .await
                        .map_err(|e| anyhow!("acp load_session failed: {e:?}"))?;
                    if let Some(modes) = response.modes {
                        self.session_modes.insert(runtime_session_ref.clone(), modes);
                    }
                    let replayed_updates = self.capture_replayed_history(&runtime_session_ref);
                    if replayed_updates > 0 {
                        info!(
                            "acp load_session replay drained: adapter={} session_id={} replay_updates={}",
                            self.adapter.id,
                            runtime_session_ref,
                            replayed_updates
                        );
                    }
                    self.loaded_sessions.insert(runtime_session_ref.clone());
                } else {
                    return Err(anyhow!(
                        "acp agent `{}` does not advertise session/load; cannot resume session `{}`",
                        self.adapter.id,
                        runtime_session_ref
                    ));
                }
            }
            runtime_session_ref
        } else {
            let response = self
                .conn
                .new_session(NewSessionRequest::new(&self.key.workspace_path))
                .await
                .map_err(|e| anyhow!("acp new_session failed: {e:?}"))?;
            let session_id = response.session_id.to_string();
            info!(
                "acp created new session: adapter={} session_id={} cwd={}",
                self.adapter.id,
                session_id,
                self.key.workspace_path.display()
            );
            if let Some(modes) = response.modes {
                self.session_modes.insert(session_id.clone(), modes);
            }
            self.loaded_sessions.insert(session_id.clone());
            session_id
        };

        let _ = events_tx.send(RuntimeEvent::Agent(
            crate::agent::normalized::NormalizedAgentEvent::RuntimeSessionReady(session_id.clone()),
        ));
        self.ensure_session_mode(&session_id).await;
        Ok(session_id)
    }

    fn capture_replayed_history(&mut self, session_id: &str) -> usize {
        let mut drained = 0usize;
        let mut updates = Vec::new();
        while let Ok(update) = self.updates_rx.try_recv() {
            drained += 1;
            updates.push(update);
        }
        if !updates.is_empty() {
            self.history_cache
                .insert(session_id.to_string(), build_history_from_updates(&updates));
        }
        drained
    }

    async fn ensure_session_mode(&mut self, session_id: &str) {
        let Some(session_mode) = self.adapter.session_mode else {
            return;
        };
        let Some(mode_state) = self.session_modes.get(session_id) else {
            return;
        };
        let mode_available = mode_state
            .available_modes
            .iter()
            .any(|mode| mode.id.0.as_ref() == session_mode);
        let mode_current = mode_state.current_mode_id.0.as_ref() == session_mode;
        if mode_available && !mode_current {
            if self
                .conn
                .set_session_mode(SetSessionModeRequest::new(session_id.to_string(), session_mode))
                .await
                .is_ok()
            {
                if let Some(mode_state) = self.session_modes.get_mut(session_id) {
                    mode_state.current_mode_id = session_mode.into();
                }
            }
        }
    }

    async fn list_sessions(&mut self, workspace_path: PathBuf) -> Result<Vec<RuntimeSessionListing>> {
        if self
            .initialize
            .agent_capabilities
            .session_capabilities
            .list
            .is_none()
        {
            return Err(anyhow!(super::types::LIST_SESSIONS_UNSUPPORTED_ERROR_TEXT));
        }

        let mut cursor = None;
        let mut sessions = Vec::new();
        loop {
            let request = ListSessionsRequest::new()
                .cwd(workspace_path.clone())
                .cursor(cursor.clone());
            let response = self
                .conn
                .list_sessions(request)
                .await
                .map_err(|e| anyhow!("acp list_sessions failed: {e:?}"))?;
            sessions.extend(response.sessions.into_iter().map(|session| RuntimeSessionListing {
                runtime_session_ref: session.session_id.to_string(),
                workspace_path: session.cwd.to_string_lossy().to_string(),
                title: session.title,
                updated_at: session.updated_at,
            }));
            if let Some(next_cursor) = response.next_cursor {
                cursor = Some(next_cursor);
            } else {
                break;
            }
        }
        Ok(sessions)
    }

    async fn load_history(&mut self, runtime_session_ref: String) -> Result<Vec<RuntimeHistoryTurn>> {
        if !self.loaded_sessions.contains(&runtime_session_ref) {
            if self.initialize.agent_capabilities.load_session {
                let response = self
                    .conn
                    .load_session(LoadSessionRequest::new(runtime_session_ref.clone(), &self.key.workspace_path))
                    .await
                    .map_err(|e| anyhow!("acp load_session failed: {e:?}"))?;
                if let Some(modes) = response.modes {
                    self.session_modes.insert(runtime_session_ref.clone(), modes);
                }
                self.loaded_sessions.insert(runtime_session_ref.clone());
                self.capture_replayed_history(&runtime_session_ref);
            } else {
                return Err(anyhow!(
                    "acp agent `{}` does not advertise session/load; cannot resume session `{}`",
                    self.adapter.id,
                    runtime_session_ref
                ));
            }
        }
        Ok(self
            .history_cache
            .get(&runtime_session_ref)
            .cloned()
            .unwrap_or_default())
    }

    async fn shutdown(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        let _ = (&mut self.stderr_task).await;
    }
}

async fn spawn_process(config: &Config, key: &WorkerKey) -> Result<AcpRuntimeProcess> {
    let adapter = adapters::for_id(&key.adapter_id)?;
    let acp_agent_cmd = if key.adapter_id == config.acp_adapter {
        config
            .acp_agent_cmd
            .clone()
            .unwrap_or_else(|| adapter.default_command.to_string())
    } else {
        adapter.default_command.to_string()
    };

    let mut child = Command::new("sh");
    child
        .arg("-lc")
        .arg(acp_agent_cmd)
        .current_dir(&key.workspace_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    apply_proxy_env(&mut child, key);
    let mut child = child.spawn().context("failed to spawn ACP agent process")?;
    let stdin = child.stdin.take().ok_or_else(|| anyhow!("missing ACP agent stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("missing ACP agent stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow!("missing ACP agent stderr"))?;

    Ok(AcpRuntimeProcess {
        child,
        stdin,
        stdout,
        stderr_task: spawn_stderr_task(stderr, key.adapter_id.clone()),
        adapter,
    })
}

fn build_worker_key(
    adapter_id: &str,
    workspace_path: PathBuf,
    proxy_mode: Option<&str>,
    proxy_url: Option<&str>,
    default_proxy_url: Option<&str>,
    default_proxy_mode: &str,
) -> WorkerKey {
    let proxy_mode = effective_proxy_mode(proxy_mode, default_proxy_mode).to_string();
    let proxy_url = if proxy_mode == "on" {
        proxy_url
            .or(default_proxy_url)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
    } else {
        None
    };
    WorkerKey {
        adapter_id: adapter_id.to_string(),
        workspace_path,
        proxy_mode,
        proxy_url,
    }
}

fn apply_proxy_env(cmd: &mut Command, key: &WorkerKey) {
    match key.proxy_mode.as_str() {
        "off" => {
            for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"] {
                cmd.env_remove(key);
            }
        }
        "on" => {
            if let Some(proxy_url) = key.proxy_url.as_deref() {
                for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"] {
                    cmd.env(key, proxy_url);
                }
            }
        }
        _ => {}
    }
}

fn effective_proxy_mode<'a>(proxy_mode: Option<&'a str>, default_proxy_mode: &'a str) -> &'a str {
    match proxy_mode.unwrap_or("default") {
        "on" => "on",
        "off" => "off",
        _ => default_proxy_mode,
    }
}

fn spawn_stderr_task(stderr: ChildStderr, adapter_id: String) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut stderr_lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = stderr_lines.next_line().await {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                warn!("acp stderr [{}]: {}", adapter_id, trimmed);
            }
        }
    })
}

pub struct AcpBridgeClient {
    pub updates_tx: mpsc::UnboundedSender<SessionUpdate>,
    pub cancelled_flag: Arc<StdMutex<Arc<AtomicBool>>>,
}

#[async_trait::async_trait(?Send)]
impl AcpClientTrait for AcpBridgeClient {
    async fn request_permission(
        &self,
        args: agent_client_protocol::RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        let cancelled = self.cancelled_flag.lock().unwrap().clone();
        if cancelled.load(Ordering::SeqCst) {
            return Ok(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ));
        }
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

fn build_history_from_updates(updates: &[SessionUpdate]) -> Vec<RuntimeHistoryTurn> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Role {
        User,
        Assistant,
    }

    let mut turns = Vec::<RuntimeHistoryTurn>::new();
    let mut current_role: Option<Role> = None;
    let mut current_text = String::new();

    let flush = |turns: &mut Vec<RuntimeHistoryTurn>, role: Option<Role>, text: &mut String| {
        let content = text.trim();
        if content.is_empty() {
            text.clear();
            return;
        }
        match role {
            Some(Role::User) => turns.push(RuntimeHistoryTurn {
                user_text: content.to_string(),
                assistant_text: String::new(),
            }),
            Some(Role::Assistant) => {
                if let Some(last) = turns.last_mut() {
                    if last.assistant_text.is_empty() {
                        last.assistant_text = content.to_string();
                    } else {
                        turns.push(RuntimeHistoryTurn {
                            user_text: String::new(),
                            assistant_text: content.to_string(),
                        });
                    }
                } else {
                    turns.push(RuntimeHistoryTurn {
                        user_text: String::new(),
                        assistant_text: content.to_string(),
                    });
                }
            }
            None => {}
        }
        text.clear();
    };

    for update in updates {
        let next = match update {
            SessionUpdate::UserMessageChunk(chunk) => extract_text_chunk(&chunk.content).map(|text| (Role::User, text)),
            SessionUpdate::AgentMessageChunk(chunk) => extract_text_chunk(&chunk.content).map(|text| (Role::Assistant, text)),
            _ => None,
        };
        let Some((role, text)) = next else {
            continue;
        };
        if current_role == Some(role) {
            current_text.push_str(&text);
        } else {
            flush(&mut turns, current_role, &mut current_text);
            current_role = Some(role);
            current_text.push_str(&text);
        }
    }
    flush(&mut turns, current_role, &mut current_text);

    turns
        .into_iter()
        .filter(|turn| !turn.user_text.trim().is_empty() || !turn.assistant_text.trim().is_empty())
        .collect()
}

fn extract_text_chunk(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::Text(text) => Some(text.text.clone()),
        _ => None,
    }
}
