use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use axum::{routing::{get, post}, Router};
use tokio::signal::unix::{signal, SignalKind};
use tokio::fs;
use tracing::info;

use crate::{
    agent::runtime::build_runtime,
    api::http::{healthz, submit_control, submit_inbound, submit_turn},
    config::Config,
    core::{
        persistence::Persistence,
        ports::TurnEventSink,
        registry::SessionRegistry,
        service::CoreService,
    },
    protocol::CoreOutboundEvent,
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub core: CoreService,
}

#[derive(Clone)]
struct HttpTurnEventSink {
    client: reqwest::Client,
    url: String,
    token: Option<String>,
}

impl HttpTurnEventSink {
    fn new(url: String, token: Option<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("build gateway callback client"),
            url,
            token,
        }
    }
}

#[async_trait]
impl TurnEventSink for HttpTurnEventSink {
    async fn publish(&self, event: &CoreOutboundEvent) -> Result<()> {
        let mut request = self.client.post(&self.url).json(event);
        if let Some(token) = self.token.as_deref() {
            request = request.header("x-gateway-event-token", token);
        }
        let response = request.send().await.context("post gateway event failed")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("gateway event rejected status={} body={}", status, body);
        }
        Ok(())
    }
}

pub async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = Arc::new(Config::from_env()?);
    fs::create_dir_all(&config.codex_workdir)
        .await
        .with_context(|| format!("create CODEX_WORKDIR failed: {:?}", config.codex_workdir))?;

    let persistence = Persistence::new(config.state_db_path.clone());
    persistence.init().await?;
    let registry = SessionRegistry::new(persistence.clone()).await?;
    let runtime = build_runtime(config.clone());
    let sink: Arc<dyn TurnEventSink> = Arc::new(HttpTurnEventSink::new(
        config.gateway_event_url.clone(),
        config.gateway_event_token.clone(),
    ));
    let core = CoreService::new(config.clone(), runtime, sink, persistence, registry);

    let state = AppState {
        config: config.clone(),
        core,
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/internal/core/inbound", post(submit_inbound))
        .route("/internal/core/turn", post(submit_turn))
        .route("/internal/core/control", post(submit_control))
        .with_state(state);

    info!(
        "runtime config: mode={}, adapter={}, workdir={}",
        config.runtime_mode,
        config.acp_adapter,
        config.codex_workdir.display()
    );
    info!("core listening on {}", config.core_bind);
    let listener = tokio::net::TcpListener::bind(config.core_bind).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(wait_for_shutdown_signal())
        .await?;
    Ok(())
}

async fn wait_for_shutdown_signal() {
    let mut sigterm = signal(SignalKind::terminate()).expect("register SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("register SIGINT handler");
    let mut sighup = signal(SignalKind::hangup()).expect("register SIGHUP handler");

    tokio::select! {
        _ = sigterm.recv() => {
            info!("received SIGTERM, shutting down core");
        }
        _ = sigint.recv() => {
            info!("received SIGINT, shutting down core");
        }
        _ = sighup.recv() => {
            info!("received SIGHUP, shutting down core for reload");
        }
    }
}
