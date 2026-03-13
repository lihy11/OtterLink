#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT_DIR/target/release/otterlink"

# shellcheck disable=SC1091
source "$ROOT_DIR/scripts/lib/common-env.sh"
otterlink_source_env "$ROOT_DIR"
otterlink_default_runtime_env "$ROOT_DIR"

cd "$ROOT_DIR"

if [ -x "$BIN" ]; then
  exec "$BIN"
fi

exec cargo run --bin otterlink
