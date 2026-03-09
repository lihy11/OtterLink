#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GATEWAY_DIR="$ROOT_DIR/gateway"
ENV_FILE="${REMOTEAGENT_ENV_FILE:-$ROOT_DIR/.run/feishu.env}"

if [ -f "$ENV_FILE" ]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

export BIND="${BIND:-127.0.0.1:1127}"
export CORE_BASE_URL="${CORE_BASE_URL:-http://127.0.0.1:7211}"

cd "$GATEWAY_DIR"
exec node src/index.js
