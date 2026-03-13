#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$ROOT_DIR"
cargo test

cd "$ROOT_DIR/gateway"
npm test

cd "$ROOT_DIR"
if [ -f .run/feishu.env ]; then
  # shellcheck disable=SC1091
  source "$ROOT_DIR/scripts/lib/common-env.sh"
  remoteagent_source_env "$ROOT_DIR"
  FEISHU_DISABLE_WS=1 ./scripts/start-longconn.sh
  curl --noproxy '*' -fsS http://127.0.0.1:7211/healthz >/dev/null
  curl --noproxy '*' -fsS http://127.0.0.1:1127/healthz >/dev/null
  ./scripts/stop-longconn.sh
fi

echo "local tests completed"
