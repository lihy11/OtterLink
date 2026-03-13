#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT_DIR/target/release/feishu-acp-bridge-demo"

# shellcheck disable=SC1091
source "$ROOT_DIR/scripts/lib/common-env.sh"
remoteagent_source_env "$ROOT_DIR"
remoteagent_default_runtime_env "$ROOT_DIR"

cd "$ROOT_DIR"

if [ -x "$BIN" ]; then
  exec "$BIN"
fi

exec cargo run --bin feishu-acp-bridge-demo
