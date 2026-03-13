#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_DIR="$ROOT_DIR/.run"
mkdir -p "$RUN_DIR"

# shellcheck disable=SC1091
source "$ROOT_DIR/scripts/lib/common-env.sh"
otterlink_source_env "$ROOT_DIR"
otterlink_default_runtime_env "$ROOT_DIR"

: "${APP_ID:?missing APP_ID}"
: "${APP_SECRET:?missing APP_SECRET}"
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
