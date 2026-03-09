#!/usr/bin/env bash
set -euo pipefail

if [ "$(uname -s)" != "Linux" ]; then
  echo "systemd install is only supported on Linux"
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SYSTEMD_DIR="$ROOT_DIR/deploy/systemd"
UNIT_TARGET_DIR="${UNIT_TARGET_DIR:-/etc/systemd/system}"
SERVICE_USER="${SERVICE_USER:-$(id -un)}"
SERVICE_GROUP="${SERVICE_GROUP:-$(id -gn)}"
ENV_FILE="${ENV_FILE:-/etc/remoteagent/remoteagent.env}"

if [ ! -d "$SYSTEMD_DIR" ]; then
  echo "missing $SYSTEMD_DIR"
  exit 1
fi

if [ "$(id -u)" -ne 0 ]; then
  echo "run as root so units can be installed into $UNIT_TARGET_DIR"
  exit 1
fi

mkdir -p "$UNIT_TARGET_DIR"

render_unit() {
  local src="$1"
  local dst="$2"
  sed \
    -e "s|__ROOT_DIR__|$ROOT_DIR|g" \
    -e "s|__SERVICE_USER__|$SERVICE_USER|g" \
    -e "s|__SERVICE_GROUP__|$SERVICE_GROUP|g" \
    -e "s|__ENV_FILE__|$ENV_FILE|g" \
    "$src" > "$dst"
}

render_unit "$SYSTEMD_DIR/remoteagent-core.service" "$UNIT_TARGET_DIR/remoteagent-core.service"
render_unit "$SYSTEMD_DIR/remoteagent-gateway.service" "$UNIT_TARGET_DIR/remoteagent-gateway.service"
cp "$SYSTEMD_DIR/remoteagent.target" "$UNIT_TARGET_DIR/remoteagent.target"

systemctl daemon-reload
systemctl enable remoteagent-core.service remoteagent-gateway.service remoteagent.target >/dev/null

cat <<EOF
installed systemd units:
  $UNIT_TARGET_DIR/remoteagent-core.service
  $UNIT_TARGET_DIR/remoteagent-gateway.service
  $UNIT_TARGET_DIR/remoteagent.target

next steps:
  1. create env file: $ENV_FILE
  2. build release binary: (cd $ROOT_DIR && cargo build --release)
  3. install gateway deps: (cd $ROOT_DIR/gateway && npm ci)
  4. start services: systemctl start remoteagent-core remoteagent-gateway
  5. or start target: systemctl start remoteagent.target
EOF
