#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_DIR="$ROOT_DIR/.run"
GATEWAY_DIR="$ROOT_DIR/gateway"
ENV_FILE="${REMOTEAGENT_ENV_FILE:-$ROOT_DIR/.run/feishu.env}"
mkdir -p "$RUN_DIR"

if [ -f "$ENV_FILE" ]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

: "${APP_ID:?missing APP_ID}"
: "${APP_SECRET:?missing APP_SECRET}"

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
export CODEX_WORKDIR="${CODEX_WORKDIR:-./workspace}"
export CODEX_SKIP_GIT_REPO_CHECK="${CODEX_SKIP_GIT_REPO_CHECK:-true}"
export RUNTIME_MODE="${RUNTIME_MODE:-acp_fallback}"
export ACP_ADAPTER="${ACP_ADAPTER:-claude_code}"
export ACP_AGENT_CMD="${ACP_AGENT_CMD:-}"
export ACP_PROXY_URL="${ACP_PROXY_URL:-}"
export CLAUDE_CODE_DEFAULT_PROXY_MODE="${CLAUDE_CODE_DEFAULT_PROXY_MODE:-off}"
export CODEX_DEFAULT_PROXY_MODE="${CODEX_DEFAULT_PROXY_MODE:-on}"
export RENDER_MIN_UPDATE_MS="${RENDER_MIN_UPDATE_MS:-700}"
export TODO_EVENT_LOG_PATH="${TODO_EVENT_LOG_PATH:-$ROOT_DIR/.run/todo-events.jsonl}"

if [[ "$CODEX_WORKDIR" != /* ]]; then
  CODEX_WORKDIR="$ROOT_DIR/${CODEX_WORKDIR#./}"
fi
if [[ "$CODEX_WORKDIR" == "$ROOT_DIR" ]]; then
  CODEX_WORKDIR="$ROOT_DIR/workspace"
fi
export CODEX_WORKDIR
mkdir -p "$CODEX_WORKDIR"

for name in rust gateway; do
  pid_file="$RUN_DIR/$name.pid"
  if [ -f "$pid_file" ]; then
    pid="$(cat "$pid_file")"
    if kill -0 "$pid" >/dev/null 2>&1; then
      echo "$name is already running with pid=$pid"
      exit 1
    fi
  fi
done

for port in "${BIND##*:}" "${CORE_BIND##*:}"; do
  if lsof -iTCP:"$port" -sTCP:LISTEN -n -P | rg -q LISTEN; then
    echo "port $port is already in use; stop existing process first"
    exit 1
  fi
done

if [ ! -d "$GATEWAY_DIR/node_modules" ]; then
  (cd "$GATEWAY_DIR" && npm install)
fi

nohup "$ROOT_DIR/scripts/run-core.sh" >"$RUN_DIR/rust.log" 2>&1 &
echo $! > "$RUN_DIR/rust.pid"

nohup "$ROOT_DIR/scripts/run-gateway.sh" >"$RUN_DIR/gateway.log" 2>&1 &
echo $! > "$RUN_DIR/gateway.pid"

for _ in $(seq 1 40); do
  if curl --noproxy '*' -fsS "http://$CORE_BIND/healthz" >/dev/null 2>&1 && curl --noproxy '*' -fsS "http://$BIND/healthz" >/dev/null 2>&1; then
    break
  fi
  sleep 0.5
done

if ! kill -0 "$(cat "$RUN_DIR/rust.pid")" >/dev/null 2>&1; then
  echo "rust process exited early; recent rust log:"
  tail -n 120 "$RUN_DIR/rust.log" || true
  exit 1
fi

if ! kill -0 "$(cat "$RUN_DIR/gateway.pid")" >/dev/null 2>&1; then
  echo "gateway process exited early; recent gateway log:"
  tail -n 120 "$RUN_DIR/gateway.log" || true
  exit 1
fi

echo "started rust pid=$(cat "$RUN_DIR/rust.pid")"
echo "started gateway pid=$(cat "$RUN_DIR/gateway.pid")"
echo "core health: http://$CORE_BIND/healthz"
echo "gateway health: http://$BIND/healthz"
echo "notify endpoint: POST http://$BIND/internal/notify"
echo "runtime_mode: $RUNTIME_MODE"
echo "acp_adapter: $ACP_ADAPTER"
