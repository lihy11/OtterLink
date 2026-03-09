#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT_DIR/target/release/feishu-acp-bridge-demo"
ENV_FILE="${REMOTEAGENT_ENV_FILE:-$ROOT_DIR/.run/feishu.env}"

if [ -f "$ENV_FILE" ]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

export CORE_BIND="${CORE_BIND:-127.0.0.1:7211}"
export GATEWAY_EVENT_URL="${GATEWAY_EVENT_URL:-http://127.0.0.1:1127/internal/gateway/event}"
export ACP_ADAPTER="${ACP_ADAPTER:-claude_code}"
export CODEX_WORKDIR="${CODEX_WORKDIR:-$ROOT_DIR/workspace}"

cd "$ROOT_DIR"

if [ -x "$BIN" ]; then
  exec "$BIN"
fi

exec cargo run --bin feishu-acp-bridge-demo
