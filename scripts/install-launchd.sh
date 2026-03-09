#!/usr/bin/env bash
set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
  echo "launchd install is only supported on macOS"
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEMPLATE_DIR="$ROOT_DIR/deploy/launchd"
AGENT_DIR="${LAUNCHD_AGENT_DIR:-$HOME/Library/LaunchAgents}"
ENV_FILE="${ENV_FILE:-$ROOT_DIR/.run/feishu.env}"
UID_VALUE="$(id -u)"

mkdir -p "$AGENT_DIR" "$ROOT_DIR/.run"

render_plist() {
  local src="$1"
  local dst="$2"
  sed \
    -e "s|__ROOT_DIR__|$ROOT_DIR|g" \
    -e "s|__ENV_FILE__|$ENV_FILE|g" \
    "$src" > "$dst"
}

CORE_PLIST="$AGENT_DIR/com.remoteagent.core.plist"
GATEWAY_PLIST="$AGENT_DIR/com.remoteagent.gateway.plist"

render_plist "$TEMPLATE_DIR/com.remoteagent.core.plist" "$CORE_PLIST"
render_plist "$TEMPLATE_DIR/com.remoteagent.gateway.plist" "$GATEWAY_PLIST"

launchctl bootout "gui/$UID_VALUE" "$CORE_PLIST" >/dev/null 2>&1 || true
launchctl bootout "gui/$UID_VALUE" "$GATEWAY_PLIST" >/dev/null 2>&1 || true

launchctl bootstrap "gui/$UID_VALUE" "$CORE_PLIST"
launchctl bootstrap "gui/$UID_VALUE" "$GATEWAY_PLIST"

launchctl kickstart -k "gui/$UID_VALUE/com.remoteagent.core"
launchctl kickstart -k "gui/$UID_VALUE/com.remoteagent.gateway"

cat <<EOF
installed and started launchd agents:
  $CORE_PLIST
  $GATEWAY_PLIST

env file:
  $ENV_FILE

status:
  launchctl print gui/$UID_VALUE/com.remoteagent.core
  launchctl print gui/$UID_VALUE/com.remoteagent.gateway

logs:
  tail -f "$ROOT_DIR/.run/core.launchd.log"
  tail -f "$ROOT_DIR/.run/gateway.launchd.log"
EOF
