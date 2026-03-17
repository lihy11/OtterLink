#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_BIN_DIR="${INSTALL_BIN_DIR:-$HOME/.local/bin}"
OTTERLINK_WRAPPER_PATH="$INSTALL_BIN_DIR/otterlink"
RUST_VERSION="${RUST_VERSION:-1.94.0}"
NODE_VERSION="${NODE_VERSION:-22.22.1}"
NODE_DISTRO="${NODE_DISTRO:-node-v${NODE_VERSION}-linux-x64}"
NODE_PREFIX="${NODE_PREFIX:-$HOME/.local/$NODE_DISTRO}"
usage() {
  cat <<EOF
Usage: ./scripts/install-one-click.sh

Optional env:
  RUST_VERSION=$RUST_VERSION
  NODE_VERSION=$NODE_VERSION
  INSTALL_BIN_DIR=$INSTALL_BIN_DIR
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

ensure_command() {
  local name="$1"
  if ! command -v "$name" >/dev/null 2>&1; then
    echo "missing required command: $name"
    exit 1
  fi
}

append_path_line() {
  local file="$1"
  local line="$2"
  touch "$file"
  if ! grep -Fqx "$line" "$file"; then
    printf '\n%s\n' "$line" >>"$file"
  fi
}

refresh_path() {
  case ":$PATH:" in
    *":$HOME/.cargo/bin:"*) ;;
    *) export PATH="$HOME/.cargo/bin:$PATH" ;;
  esac
  case ":$PATH:" in
    *":$INSTALL_BIN_DIR:"*) ;;
    *) export PATH="$INSTALL_BIN_DIR:$PATH" ;;
  esac
}

install_rust_if_missing() {
  if command -v cargo >/dev/null 2>&1; then
    echo "cargo already available: $(cargo --version)"
    return
  fi

  ensure_command curl
  echo "installing Rust via rustup ($RUST_VERSION)"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "$RUST_VERSION"
  refresh_path
  "$HOME/.cargo/bin/rustup" toolchain install "$RUST_VERSION"
  "$HOME/.cargo/bin/rustup" default "$RUST_VERSION"
  append_path_line "$HOME/.profile" 'export PATH="$HOME/.cargo/bin:$PATH"'
  append_path_line "$HOME/.bash_profile" 'export PATH="$HOME/.cargo/bin:$PATH"'
  append_path_line "$HOME/.bashrc" 'export PATH="$HOME/.cargo/bin:$PATH"'
  echo "installed cargo: $("$HOME/.cargo/bin/cargo" --version)"
}

install_node_if_missing() {
  if command -v node >/dev/null 2>&1 && command -v npm >/dev/null 2>&1; then
    echo "node already available: $(node --version)"
    echo "npm already available: $(npm --version)"
    return
  fi

  ensure_command curl
  ensure_command tar
  mkdir -p "$INSTALL_BIN_DIR" "$(dirname "$NODE_PREFIX")"

  local archive_url="https://nodejs.org/dist/v${NODE_VERSION}/${NODE_DISTRO}.tar.xz"
  local archive_path
  archive_path="$(mktemp "/tmp/${NODE_DISTRO}.XXXXXX.tar.xz")"

  echo "installing Node.js from $archive_url"
  curl -fsSL "$archive_url" -o "$archive_path"
  rm -rf "$NODE_PREFIX"
  tar -xJf "$archive_path" -C "$(dirname "$NODE_PREFIX")"
  rm -f "$archive_path"

  ln -sf "$NODE_PREFIX/bin/node" "$INSTALL_BIN_DIR/node"
  ln -sf "$NODE_PREFIX/bin/npm" "$INSTALL_BIN_DIR/npm"
  ln -sf "$NODE_PREFIX/bin/npx" "$INSTALL_BIN_DIR/npx"

  append_path_line "$HOME/.profile" 'export PATH="$HOME/.local/bin:$PATH"'
  append_path_line "$HOME/.bash_profile" 'export PATH="$HOME/.local/bin:$PATH"'
  append_path_line "$HOME/.bashrc" 'export PATH="$HOME/.local/bin:$PATH"'
  refresh_path

  echo "installed node: $("$INSTALL_BIN_DIR/node" --version)"
  echo "installed npm: $("$INSTALL_BIN_DIR/npm" --version)"
}

ensure_command git
install_rust_if_missing
install_node_if_missing
ensure_command cargo
ensure_command node
ensure_command npm

mkdir -p "$INSTALL_BIN_DIR" "$ROOT_DIR/.run" "$ROOT_DIR/workspace"

echo "building Rust release binary"
(cd "$ROOT_DIR" && cargo build --release)

echo "installing gateway dependencies"
(cd "$ROOT_DIR/gateway" && npm ci)

cat >"$OTTERLINK_WRAPPER_PATH" <<EOF
#!/usr/bin/env bash
set -euo pipefail
export OTTERLINK_ROOT="$ROOT_DIR"
exec node "$ROOT_DIR/scripts/otterlink-cli.js" "\$@"
EOF
chmod +x "$OTTERLINK_WRAPPER_PATH"

echo "installed CLI: $OTTERLINK_WRAPPER_PATH"
if [[ ":$PATH:" != *":$INSTALL_BIN_DIR:"* ]]; then
  echo "add $INSTALL_BIN_DIR to PATH to use \`otterlink\` directly"
fi

echo "preinstalling ACP runtimes when missing"
"$OTTERLINK_WRAPPER_PATH" install-acp all --if-missing

echo
if [ -t 0 ]; then
  read -r -p "run interactive configuration now? [Y/n]: " RUN_CONFIG
  RUN_CONFIG="${RUN_CONFIG:-Y}"
  if [[ "$RUN_CONFIG" =~ ^[Yy]$ ]]; then
    "$OTTERLINK_WRAPPER_PATH" configure
    read -r -p "start otterlink now? [Y/n]: " RUN_START
    RUN_START="${RUN_START:-Y}"
    if [[ "$RUN_START" =~ ^[Yy]$ ]]; then
      "$OTTERLINK_WRAPPER_PATH" start
      "$OTTERLINK_WRAPPER_PATH" status
    fi
  fi
fi

echo "ready"
echo "  otterlink configure"
echo "  otterlink start"
echo "  otterlink status"
