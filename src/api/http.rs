use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};

use crate::{
    app::AppState,
    core::inbound::CoreInboundRequest,
    protocol::{CoreControlRequest, CoreTurnRequest},
};

pub async fn healthz() -> &'static str {
    "ok"
}

pub async fn submit_turn(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CoreTurnRequest>,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" })));
    }

    match state.core.accept_turn(request).await {
        Ok(response) => (StatusCode::ACCEPTED, Json(serde_json::to_value(response).unwrap())),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": err.to_string() })),
        ),
    }
}

pub async fn submit_control(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CoreControlRequest>,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" })));
    }

    match state.core.handle_control(request).await {
        Ok(response) => (StatusCode::OK, Json(serde_json::to_value(response).unwrap())),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": err.to_string() })),
        ),
    }
}

pub async fn submit_inbound(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CoreInboundRequest>,
) -> impl IntoResponse {
    if !authorized(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" })));
    }

    match state.core.handle_inbound(request).await {
        Ok(response) => (StatusCode::OK, Json(serde_json::to_value(response).unwrap())),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": err.to_string() })),
        ),
    }
}

fn authorized(state: &AppState, headers: &HeaderMap) -> bool {
    match state.config.core_ingest_token.as_deref() {
        Some(expected) => headers
            .get("x-core-ingest-token")
            .and_then(|value| value.to_str().ok())
            .map(|value| value == expected)
            .unwrap_or(false),
        None => true,
    }
}
