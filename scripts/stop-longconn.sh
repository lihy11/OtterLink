#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_DIR="$ROOT_DIR/.run"

for name in gateway rust; do
  pid_file="$RUN_DIR/$name.pid"
  if [ -f "$pid_file" ]; then
    pid="$(cat "$pid_file")"
    if kill -0 "$pid" >/dev/null 2>&1; then
      kill "$pid" >/dev/null 2>&1 || true
      echo "stopped $name pid=$pid"
    else
      echo "$name pid=$pid not running"
    fi
    rm -f "$pid_file"
  fi
done

pkill -f "target/debug/feishu-acp-bridge-demo|cargo run --bin feishu-acp-bridge-demo" >/dev/null 2>&1 || true
pkill -f "node src/index.js|npm start" >/dev/null 2>&1 || true

legacy_pid="$RUN_DIR/longconn.pid"
if [ -f "$legacy_pid" ]; then
  rm -f "$legacy_pid"
fi
