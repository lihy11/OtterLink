use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    sync::{mpsc, oneshot, watch, Mutex},
    task::JoinHandle,
    time::sleep,
};
use tracing::{info, warn};

use crate::{
    agent::{
        normalized::{AgentToolState, NormalizedAgentEvent},
        runtime::{
            AgentRuntime, RuntimeCancelHandle, RuntimeCompletion, RuntimeEvent,
            RuntimeHistoryQuery, RuntimeHistoryTurn, RuntimeSessionListing, RuntimeSessionQuery,
            RuntimeSteerRequest, RuntimeTurn, RuntimeTurnRequest, INTERRUPTED_ERROR_TEXT,
        },
    },
    config::Config,
    core::models::TodoEntry,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WorkerKey {
    session_key: String,
    workspace_path: PathBuf,
    proxy_mode: String,
    proxy_url: Option<String>,
}

enum WorkerCommand {
    StartTurn {
        request: RuntimeTurnRequest,
        events_tx: mpsc::UnboundedSender<RuntimeEvent>,
        cancel_rx: watch::Receiver<bool>,
        response_tx: oneshot::Sender<Result<StartedTurn>>,
        completion_tx: oneshot::Sender<Result<RuntimeCompletion>>,
    },
    SteerTurn {
        request: RuntimeSteerRequest,
        response_tx: oneshot::Sender<Result<()>>,
    },
}

#[derive(Clone)]
struct StartedTurn {
    runtime_session_ref: String,
    runtime_turn_ref: String,
}

#[derive(Clone)]
struct WorkerHandle {
    tx: mpsc::UnboundedSender<WorkerCommand>,
}

pub struct CodexAppServerRuntime {
    config: Arc<Config>,
    workers: Arc<Mutex<HashMap<WorkerKey, WorkerHandle>>>,
}

impl CodexAppServerRuntime {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            workers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn worker_for_turn(&self, request: &RuntimeTurnRequest) -> Result<WorkerHandle> {
        let workspace_path = request
            .workspace_path
            .clone()
            .unwrap_or_else(|| self.config.codex_workdir.clone());
        let key = build_worker_key(
            &request.session_key,
            workspace_path,
            request.proxy_mode.as_deref(),
            request.proxy_url.as_deref(),
            self.config.acp_proxy_url.as_deref(),
            self.config.default_proxy_mode_for_agent("codex"),
        );
        self.worker_for_key(key).await
    }

    async fn worker_for_steer(&self, request: &RuntimeSteerRequest) -> Result<WorkerHandle> {
        let workspace_path = request
            .workspace_path
            .clone()
            .unwrap_or_else(|| self.config.codex_workdir.clone());
        let key = build_worker_key(
            &request.session_key,
            workspace_path,
            request.proxy_mode.as_deref(),
            request.proxy_url.as_deref(),
            self.config.acp_proxy_url.as_deref(),
            self.config.default_proxy_mode_for_agent("codex"),
        );
        self.worker_for_key(key).await
    }

    async fn worker_for_key(&self, key: WorkerKey) -> Result<WorkerHandle> {
        let mut guard = self.workers.lock().await;
        if let Some(worker) = guard.get(&key) {
            return Ok(worker.clone());
        }
        let worker = spawn_worker(self.config.clone(), key.clone())?;
        guard.insert(key, worker.clone());
        Ok(worker)
    }
}

#[async_trait]
impl AgentRuntime for CodexAppServerRuntime {
    async fn start_turn(&self, request: RuntimeTurnRequest) -> Result<RuntimeTurn> {
        let worker = self.worker_for_turn(&request).await?;
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (cancel, cancel_rx) = RuntimeCancelHandle::new();
        let (response_tx, response_rx) = oneshot::channel();
        let (completion_tx, completion_rx) = oneshot::channel();
        worker
            .tx
            .send(WorkerCommand::StartTurn {
                request,
                events_tx,
                cancel_rx,
                response_tx,
                completion_tx,
            })
            .map_err(|_| anyhow!("codex app-server worker command channel closed"))?;

        let started = response_rx
            .await
            .map_err(|_| anyhow!("codex app-server worker response channel closed"))??;

        let completion = tokio::spawn(async move {
            completion_rx
                .await
                .map_err(|_| anyhow!("codex app-server completion channel closed"))?
        });

        Ok(RuntimeTurn {
            events: events_rx,
            completion,
            cancel,
            runtime_session_ref: Some(started.runtime_session_ref),
            runtime_turn_ref: Some(started.runtime_turn_ref),
        })
    }

    async fn steer_turn(&self, request: RuntimeSteerRequest) -> Result<()> {
        let worker = self.worker_for_steer(&request).await?;
        let (response_tx, response_rx) = oneshot::channel();
        worker
            .tx
            .send(WorkerCommand::SteerTurn { request, response_tx })
            .map_err(|_| anyhow!("codex app-server worker command channel closed"))?;
        response_rx
            .await
            .map_err(|_| anyhow!("codex app-server steer response channel closed"))?
    }

    async fn list_sessions(&self, query: RuntimeSessionQuery) -> Result<Vec<RuntimeSessionListing>> {
        let key = build_worker_key(
            "__otterlink_control__",
            query.workspace_path.clone(),
            query.proxy_mode.as_deref(),
            query.proxy_url.as_deref(),
            self.config.acp_proxy_url.as_deref(),
            self.config.default_proxy_mode_for_agent("codex"),
        );
        let mut process = spawn_process(&self.config, &key).await?;
        initialize_app_server(&process.conn).await?;
        let result = list_threads(&process.conn, &query.workspace_path).await;
        shutdown_process(&mut process.child, &mut process.stderr_task).await;
        result
    }

    async fn load_history(&self, query: RuntimeHistoryQuery) -> Result<Vec<RuntimeHistoryTurn>> {
        let key = build_worker_key(
            "__otterlink_control__",
            query.workspace_path.clone(),
            query.proxy_mode.as_deref(),
            query.proxy_url.as_deref(),
            self.config.acp_proxy_url.as_deref(),
            self.config.default_proxy_mode_for_agent("codex"),
        );
        let mut process = spawn_process(&self.config, &key).await?;
        initialize_app_server(&process.conn).await?;
        let result = read_thread_history(&process.conn, &query.runtime_session_ref).await;
        shutdown_process(&mut process.child, &mut process.stderr_task).await;
        result
    }

    fn name(&self) -> &'static str {
        "codex_app_server"
    }
}

fn spawn_worker(config: Arc<Config>, key: WorkerKey) -> Result<WorkerHandle> {
    let (tx, rx) = mpsc::unbounded_channel();
    let thread_name = format!(
        "codex-app-{}",
        key.session_key
            .chars()
            .take(12)
            .collect::<String>()
    );
    std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build codex app-server worker runtime failed");
            rt.block_on(async move {
                if let Err(err) = run_worker(config, key, rx).await {
                    warn!("codex app-server worker exited: {err:?}");
                }
            });
        })
        .context("spawn codex app-server worker thread failed")?;
    Ok(WorkerHandle { tx })
}

struct RpcConnection {
    next_id: AtomicU64,
    write_tx: mpsc::UnboundedSender<Value>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
}

impl RpcConnection {
    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        self.write_tx
            .send(json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            }))
            .map_err(|_| anyhow!("codex app-server write channel closed"))?;
        rx.await
            .map_err(|_| anyhow!("codex app-server pending response dropped"))?
    }
}

struct WorkerProcess {
    child: Child,
    notifications_rx: mpsc::UnboundedReceiver<Value>,
    stderr_task: JoinHandle<()>,
    conn: RpcConnection,
}

struct ActiveTurn {
    thread_id: String,
    runtime_turn_id: String,
    events_tx: mpsc::UnboundedSender<RuntimeEvent>,
    cancel_rx: watch::Receiver<bool>,
    completion_tx: Option<oneshot::Sender<Result<RuntimeCompletion>>>,
    cancel_requested: bool,
}

async fn run_worker(
    config: Arc<Config>,
    key: WorkerKey,
    mut rx: mpsc::UnboundedReceiver<WorkerCommand>,
) -> Result<()> {
    let mut process = spawn_process(&config, &key).await?;
    initialize_app_server(&process.conn).await?;
    let mut active_turn: Option<ActiveTurn> = None;

    loop {
        tokio::select! {
            maybe_command = rx.recv() => {
                let Some(command) = maybe_command else {
                    shutdown_process(&mut process.child, &mut process.stderr_task).await;
                    return Ok(());
                };
                match command {
                    WorkerCommand::StartTurn { request, events_tx, cancel_rx, response_tx, completion_tx } => {
                        if active_turn.is_some() {
                            let _ = response_tx.send(Err(anyhow!("codex app-server turn already active")));
                            continue;
                        }
                        let started = start_turn(&process.conn, &config, &request, &events_tx).await;
                        match started {
                            Ok(started) => {
                                active_turn = Some(ActiveTurn {
                                    thread_id: started.runtime_session_ref.clone(),
                                    runtime_turn_id: started.runtime_turn_ref.clone(),
                                    events_tx,
                                    cancel_rx,
                                    completion_tx: Some(completion_tx),
                                    cancel_requested: false,
                                });
                                let _ = response_tx.send(Ok(started));
                            }
                            Err(err) => {
                                let _ = response_tx.send(Err(err));
                            }
                        }
                    }
                    WorkerCommand::SteerTurn { request, response_tx } => {
                        let result = if let Some(active) = active_turn.as_ref() {
                            if active.thread_id != request.runtime_session_ref || active.runtime_turn_id != request.runtime_turn_ref {
                                Err(anyhow!("codex active turn changed before steer"))
                            } else {
                                steer_turn(&process.conn, &request).await
                            }
                        } else {
                            Err(anyhow!("当前没有可追加消息的 Codex 任务。"))
                        };
                        let _ = response_tx.send(result);
                    }
                }
            }
            maybe_notification = process.notifications_rx.recv() => {
                let Some(notification) = maybe_notification else {
                    finish_active_turn(&mut active_turn, Err(anyhow!("codex app-server notification stream closed")));
                    shutdown_process(&mut process.child, &mut process.stderr_task).await;
                    return Ok(());
                };
                if let Some(active) = active_turn.as_mut() {
                    if let Some(done) = handle_notification(active, notification) {
                        finish_active_turn(&mut active_turn, done);
                    }
                }
            }
            _ = async {
                if let Some(active) = active_turn.as_mut() {
                    let _ = active.cancel_rx.changed().await;
                }
            }, if active_turn.as_ref().map(|active| !active.cancel_requested).unwrap_or(false) => {
                if let Some(active) = active_turn.as_mut() {
                    if *active.cancel_rx.borrow() {
                        active.cancel_requested = true;
                        let _ = process.conn.request("turn/interrupt", json!({
                            "threadId": active.thread_id,
                            "turnId": active.runtime_turn_id,
                        })).await;
                    }
                }
            }
        }
    }
}

fn finish_active_turn(active_turn: &mut Option<ActiveTurn>, result: Result<RuntimeCompletion>) {
    if let Some(mut active) = active_turn.take() {
        if let Some(completion_tx) = active.completion_tx.take() {
            let _ = completion_tx.send(result);
        }
    }
}

async fn start_turn(
    conn: &RpcConnection,
    config: &Config,
    request: &RuntimeTurnRequest,
    events_tx: &mpsc::UnboundedSender<RuntimeEvent>,
) -> Result<StartedTurn> {
    let workspace_path = request
        .workspace_path
        .clone()
        .unwrap_or_else(|| config.codex_workdir.clone());
    let thread_id = if let Some(runtime_session_ref) = request.runtime_session_ref.as_ref() {
        let response = conn
            .request(
                "thread/resume",
                json!({
                    "threadId": runtime_session_ref,
                    "cwd": workspace_path,
                    "model": config.codex_model,
                    "approvalPolicy": "never",
                    "sandbox": "danger-full-access",
                    "personality": "pragmatic",
                }),
            )
            .await?;
        response
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("codex app-server thread/resume response missing thread.id"))?
    } else {
        let response = conn
            .request(
                "thread/start",
                json!({
                    "cwd": workspace_path,
                    "model": config.codex_model,
                    "approvalPolicy": "never",
                    "sandbox": "danger-full-access",
                    "personality": "pragmatic",
                }),
            )
            .await?;
        response
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("codex app-server thread/start response missing thread.id"))?
    };

    let _ = events_tx.send(RuntimeEvent::Agent(NormalizedAgentEvent::RuntimeSessionReady(
        thread_id.clone(),
    )));
    let _ = events_tx.send(RuntimeEvent::Agent(NormalizedAgentEvent::TurnStarted));

    let response = conn
        .request(
            "turn/start",
            json!({
                "threadId": thread_id,
                "cwd": workspace_path,
                "model": config.codex_model,
                "approvalPolicy": "never",
                "sandboxPolicy": {
                    "type": "dangerFullAccess"
                },
                "personality": "pragmatic",
                "input": [
                    {
                        "type": "text",
                        "text": request.prompt,
                    }
                ]
            }),
        )
        .await?;
    let runtime_turn_ref = response
        .get("turn")
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("codex app-server turn/start response missing turn.id"))?;

    Ok(StartedTurn {
        runtime_session_ref: thread_id,
        runtime_turn_ref,
    })
}

async fn steer_turn(conn: &RpcConnection, request: &RuntimeSteerRequest) -> Result<()> {
    conn.request(
        "turn/steer",
        json!({
            "threadId": request.runtime_session_ref,
            "expectedTurnId": request.runtime_turn_ref,
            "input": [
                {
                    "type": "text",
                    "text": request.prompt,
                }
            ]
        }),
    )
    .await?;
    Ok(())
}

async fn list_threads(
    conn: &RpcConnection,
    workspace_path: &std::path::Path,
) -> Result<Vec<RuntimeSessionListing>> {
    let mut sessions = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let result = conn
            .request(
                "thread/list",
                json!({
                    "archived": false,
                    "cwd": workspace_path,
                    "cursor": cursor,
                    "limit": 100,
                    "sortKey": "updated_at",
                    "sourceKinds": ["cli", "vscode", "exec", "appServer"]
                }),
            )
            .await?;

        if let Some(data) = result.get("data").and_then(Value::as_array) {
            for item in data {
                let Some(runtime_session_ref) = item.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let workspace_path = item
                    .get("cwd")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let title = item
                    .get("preview")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
                    .or_else(|| {
                        item.get("name")
                            .and_then(Value::as_str)
                            .filter(|value| !value.trim().is_empty())
                            .map(str::to_string)
                    });
                let updated_at = item
                    .get("updatedAt")
                    .and_then(Value::as_i64)
                    .map(|value| value.to_string());
                sessions.push(RuntimeSessionListing {
                    runtime_session_ref: runtime_session_ref.to_string(),
                    workspace_path,
                    title,
                    updated_at,
                });
            }
        }

        cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }

    Ok(sessions)
}

async fn read_thread_history(
    conn: &RpcConnection,
    runtime_session_ref: &str,
) -> Result<Vec<RuntimeHistoryTurn>> {
    let result = conn
        .request(
            "thread/read",
            json!({
                "threadId": runtime_session_ref,
                "includeTurns": true,
            }),
        )
        .await?;
    Ok(parse_thread_history_from_read_response(&result))
}

fn parse_thread_history_from_read_response(result: &Value) -> Vec<RuntimeHistoryTurn> {
    result
        .get("thread")
        .and_then(|thread| thread.get("turns"))
        .and_then(Value::as_array)
        .map(|turns| {
            turns.iter()
                .filter_map(parse_history_turn)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn parse_history_turn(turn: &Value) -> Option<RuntimeHistoryTurn> {
    let items = turn.get("items")?.as_array()?;
    let user_text = items
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("userMessage"))
        .flat_map(extract_user_message_texts)
        .collect::<Vec<_>>()
        .join("\n");
    let assistant_text = items
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("agentMessage"))
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>()
        .join("\n");

    if user_text.is_empty() && assistant_text.is_empty() {
        return None;
    }

    Some(RuntimeHistoryTurn {
        user_text,
        assistant_text,
    })
}

fn extract_user_message_texts(item: &Value) -> Vec<String> {
    item.get("content")
        .and_then(Value::as_array)
        .map(|content| {
            content
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn handle_notification(active: &mut ActiveTurn, notification: Value) -> Option<Result<RuntimeCompletion>> {
    let method = notification.get("method").and_then(Value::as_str).unwrap_or_default();
    let params = notification.get("params").cloned().unwrap_or_else(|| json!({}));
    if !matches_thread(&params, &active.thread_id) {
        return None;
    }
    if !matches_turn_or_global(&params, &active.runtime_turn_id) {
        return None;
    }

    match method {
        "agentMessage/delta" | "item/agentMessage/delta" => {
            if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                let _ = active
                    .events_tx
                    .send(RuntimeEvent::Agent(NormalizedAgentEvent::AssistantChunk(
                        delta.to_string(),
                    )));
            }
            None
        }
        "turn/plan/updated" => {
            let todos = params
                .get("plan")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            Some(TodoEntry {
                                content: item.get("step")?.as_str()?.to_string(),
                                status: match item.get("status").and_then(Value::as_str).unwrap_or("pending") {
                                    "inProgress" => "in_progress".to_string(),
                                    other => other.to_lowercase(),
                                },
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if !todos.is_empty() {
                let _ = active
                    .events_tx
                    .send(RuntimeEvent::Agent(NormalizedAgentEvent::PlanUpdated(todos)));
            }
            None
        }
        "item/started" => {
            if let Some(item_type) = params
                .get("item")
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
            {
                if item_type.contains("command") {
                    let tool_call_id = params
                        .get("item")
                        .and_then(|item| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("commandExecution")
                        .to_string();
                    let _ = active.events_tx.send(RuntimeEvent::Agent(
                        NormalizedAgentEvent::ToolState {
                            tool_call_id,
                            state: AgentToolState::InProgress,
                        },
                    ));
                }
            }
            None
        }
        "item/completed" => {
            if let Some(item_type) = params
                .get("item")
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
            {
                if item_type.contains("command") {
                    let tool_call_id = params
                        .get("item")
                        .and_then(|item| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("commandExecution")
                        .to_string();
                    let _ = active.events_tx.send(RuntimeEvent::Agent(
                        NormalizedAgentEvent::ToolState {
                            tool_call_id,
                            state: AgentToolState::Completed,
                        },
                    ));
                }
            }
            None
        }
        "error" => {
            let message = params
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("codex app-server turn failed")
                .to_string();
            let will_retry = params
                .get("willRetry")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if will_retry {
                return None;
            }
            Some(Err(anyhow!(message)))
        }
        "turn/completed" => {
            let _ = active
                .events_tx
                .send(RuntimeEvent::Agent(NormalizedAgentEvent::TurnCompleted));
            let status = params
                .get("turn")
                .and_then(|turn| turn.get("status"))
                .map(turn_status_name)
                .unwrap_or_else(|| "completed".to_string());
            if status == "interrupted" {
                Some(Err(anyhow!(INTERRUPTED_ERROR_TEXT)))
            } else if status == "failed" {
                let message = params
                    .get("turn")
                    .and_then(|turn| turn.get("error"))
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("codex app-server turn failed");
                Some(Err(anyhow!(message.to_string())))
            } else {
                Some(Ok(RuntimeCompletion {
                    stderr_summary: None,
                    stop_reason: Some(status),
                }))
            }
        }
        _ => None,
    }
}

fn turn_status_name(value: &Value) -> String {
    match value {
        Value::String(status) => status.to_lowercase(),
        Value::Object(map) => map.keys().next().cloned().unwrap_or_else(|| "completed".to_string()),
        _ => "completed".to_string(),
    }
}

fn matches_thread(params: &Value, expected_thread_id: &str) -> bool {
    params
        .get("threadId")
        .and_then(Value::as_str)
        .map(|value| value == expected_thread_id)
        .unwrap_or(false)
}

fn matches_turn_or_global(params: &Value, expected_turn_id: &str) -> bool {
    params
        .get("turnId")
        .and_then(Value::as_str)
        .map(|value| value == expected_turn_id)
        .unwrap_or(true)
        || params
            .get("turn")
            .and_then(|turn| turn.get("id"))
            .and_then(Value::as_str)
            .map(|value| value == expected_turn_id)
            .unwrap_or(false)
}

async fn initialize_app_server(conn: &RpcConnection) -> Result<()> {
    conn.request(
        "initialize",
        json!({
            "clientInfo": {
                "name": "otterlink",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": {
                "experimentalApi": false,
            }
        }),
    )
    .await?;
    Ok(())
}

async fn spawn_process(config: &Config, key: &WorkerKey) -> Result<WorkerProcess> {
    let mut child = Command::new(&config.codex_bin);
    child
        .arg("app-server")
        .arg("--listen")
        .arg("stdio://")
        .arg("--session-source")
        .arg("exec")
        .current_dir(&key.workspace_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    apply_proxy_env(&mut child, key);

    let mut child = child.spawn().context("failed to spawn codex app-server process")?;
    let stdin = child.stdin.take().ok_or_else(|| anyhow!("missing codex app-server stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("missing codex app-server stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow!("missing codex app-server stderr"))?;

    let (write_tx, write_rx) = mpsc::unbounded_channel::<Value>();
    let pending = Arc::new(Mutex::new(HashMap::<u64, oneshot::Sender<Result<Value>>>::new()));
    let (notifications_tx, notifications_rx) = mpsc::unbounded_channel();
    let pending_for_reader = pending.clone();
    let pending_for_writer_cleanup = pending.clone();

    tokio::spawn(async move {
        let _ = writer_task(stdin, write_rx).await;
        let mut guard = pending_for_writer_cleanup.lock().await;
        for (_, tx) in guard.drain() {
            let _ = tx.send(Err(anyhow!("codex app-server writer task exited")));
        }
    });

    tokio::spawn(async move {
        let _ = reader_task(stdout, notifications_tx, pending_for_reader).await;
    });

    Ok(WorkerProcess {
        child,
        notifications_rx,
        stderr_task: spawn_stderr_task(stderr),
        conn: RpcConnection {
            next_id: AtomicU64::new(1),
            write_tx,
            pending,
        },
    })
}

async fn writer_task(
    mut stdin: ChildStdin,
    mut write_rx: mpsc::UnboundedReceiver<Value>,
) -> Result<()> {
    while let Some(message) = write_rx.recv().await {
        let line = serde_json::to_vec(&message)?;
        stdin.write_all(&line).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
    }
    Ok(())
}

async fn reader_task(
    stdout: ChildStdout,
    notifications_tx: mpsc::UnboundedSender<Value>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
) -> Result<()> {
    let mut lines = BufReader::new(stdout).lines();
    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            if let Some(tx) = pending.lock().await.remove(&id) {
                if let Some(error) = value.get("error") {
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("codex app-server request failed");
                    let _ = tx.send(Err(anyhow!(message.to_string())));
                } else {
                    let _ = tx.send(Ok(value.get("result").cloned().unwrap_or(Value::Null)));
                }
            }
            continue;
        }
        let _ = notifications_tx.send(value);
    }
    Ok(())
}

fn build_worker_key(
    session_key: &str,
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
        session_key: session_key.to_string(),
        workspace_path,
        proxy_mode,
        proxy_url,
    }
}

fn apply_proxy_env(cmd: &mut Command, key: &WorkerKey) {
    match key.proxy_mode.as_str() {
        "off" => {
            for env_key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"] {
                cmd.env_remove(env_key);
            }
        }
        "on" => {
            if let Some(proxy_url) = key.proxy_url.as_deref() {
                for env_key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"] {
                    cmd.env(env_key, proxy_url);
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

fn spawn_stderr_task(stderr: ChildStderr) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                info!("codex app-server stderr: {}", trimmed);
            }
        }
    })
}

async fn shutdown_process(child: &mut Child, stderr_task: &mut JoinHandle<()>) {
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
    stderr_task.abort();
    let _ = sleep(Duration::from_millis(10)).await;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::parse_thread_history_from_read_response;

    #[test]
    fn parse_thread_history_maps_user_and_agent_messages() {
        let parsed = parse_thread_history_from_read_response(&json!({
            "thread": {
                "turns": [
                    {
                        "id": "turn_1",
                        "status": "completed",
                        "items": [
                            {
                                "id": "u1",
                                "type": "userMessage",
                                "content": [
                                    {"type": "text", "text": "先检查目录结构"},
                                    {"type": "text", "text": "然后总结"}
                                ]
                            },
                            {
                                "id": "a1",
                                "type": "agentMessage",
                                "text": "我先列一下当前目录。"
                            },
                            {
                                "id": "a2",
                                "type": "agentMessage",
                                "text": "接着我会总结关键模块。"
                            }
                        ]
                    },
                    {
                        "id": "turn_2",
                        "status": "completed",
                        "items": [
                            {
                                "id": "u2",
                                "type": "userMessage",
                                "content": [{"type": "text", "text": "继续"}]
                            }
                        ]
                    }
                ]
            }
        }));

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].user_text, "先检查目录结构\n然后总结");
        assert_eq!(parsed[0].assistant_text, "我先列一下当前目录。\n接着我会总结关键模块。");
        assert_eq!(parsed[1].user_text, "继续");
        assert_eq!(parsed[1].assistant_text, "");
    }
}
