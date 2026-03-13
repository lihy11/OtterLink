#!/usr/bin/env sh

remoteagent_source_env() {
  if [ "$#" -lt 1 ]; then
    echo "remoteagent_source_env requires ROOT_DIR" >&2
    return 1
  fi

  ROOT_DIR="$1"
  ENV_FILE="${REMOTEAGENT_ENV_FILE:-$ROOT_DIR/.run/feishu.env}"

  if [ -f "$ENV_FILE" ]; then
    set -a
    # shellcheck disable=SC1090
    . "$ENV_FILE"
    set +a
  fi
}

remoteagent_default_runtime_env() {
  if [ "$#" -lt 1 ]; then
    echo "remoteagent_default_runtime_env requires ROOT_DIR" >&2
    return 1
  fi

  ROOT_DIR="$1"

  export BIND="${BIND:-127.0.0.1:1127}"
  export CORE_BIND="${CORE_BIND:-127.0.0.1:7211}"
  export CORE_BASE_URL="${CORE_BASE_URL:-http://$CORE_BIND}"
  export CORE_INGEST_TOKEN="${CORE_INGEST_TOKEN:-bridge_ingest_local_20260307}"
  export GATEWAY_EVENT_URL="${GATEWAY_EVENT_URL:-http://$BIND/internal/gateway/event}"
  export GATEWAY_EVENT_TOKEN="${GATEWAY_EVENT_TOKEN:-gateway_event_local_20260307}"
  export BRIDGE_INGEST_TOKEN="${BRIDGE_INGEST_TOKEN:-bridge_ingest_local_20260307}"
  export BRIDGE_NOTIFY_TOKEN="${BRIDGE_NOTIFY_TOKEN:-notify_local_20260307}"
  export FEISHU_AUTH_MODE="${FEISHU_AUTH_MODE:-off}"
  export PAIR_AUTH_TOKEN="${PAIR_AUTH_TOKEN:-}"
  export ALLOW_FROM_OPEN_IDS="${ALLOW_FROM_OPEN_IDS:-}"
  export PAIR_STORE_PATH="${PAIR_STORE_PATH:-$ROOT_DIR/.run/pairings.json}"
  export STATE_DB_PATH="${STATE_DB_PATH:-$ROOT_DIR/.run/state.db}"
  export CODEX_BIN="${CODEX_BIN:-codex}"
  export CODEX_WORKDIR="${CODEX_WORKDIR:-$ROOT_DIR/workspace}"
  export CODEX_SKIP_GIT_REPO_CHECK="${CODEX_SKIP_GIT_REPO_CHECK:-true}"
  export RUNTIME_MODE="${RUNTIME_MODE:-acp_fallback}"
  export ACP_ADAPTER="${ACP_ADAPTER:-claude_code}"
  export ACP_AGENT_CMD="${ACP_AGENT_CMD:-}"
  export ACP_PROXY_URL="${ACP_PROXY_URL:-}"
  export CLAUDE_CODE_DEFAULT_PROXY_MODE="${CLAUDE_CODE_DEFAULT_PROXY_MODE:-off}"
  export CODEX_DEFAULT_PROXY_MODE="${CODEX_DEFAULT_PROXY_MODE:-on}"
  export RENDER_MIN_UPDATE_MS="${RENDER_MIN_UPDATE_MS:-700}"
  export TODO_EVENT_LOG_PATH="${TODO_EVENT_LOG_PATH:-$ROOT_DIR/.run/todo-events.jsonl}"

  if [ "${CODEX_WORKDIR#/}" = "$CODEX_WORKDIR" ]; then
    CODEX_WORKDIR="$ROOT_DIR/${CODEX_WORKDIR#./}"
  fi
  if [ "$CODEX_WORKDIR" = "$ROOT_DIR" ]; then
    CODEX_WORKDIR="$ROOT_DIR/workspace"
  fi
  export CODEX_WORKDIR
}
