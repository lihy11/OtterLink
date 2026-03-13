#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GATEWAY_DIR="$ROOT_DIR/gateway"

# shellcheck disable=SC1091
source "$ROOT_DIR/scripts/lib/common-env.sh"
otterlink_source_env "$ROOT_DIR"
otterlink_default_runtime_env "$ROOT_DIR"

cd "$GATEWAY_DIR"
exec node src/index.js
