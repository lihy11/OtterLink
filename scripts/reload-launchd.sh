#!/usr/bin/env bash
set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
  echo "launchd reload is only supported on macOS"
  exit 1
fi

UID_VALUE="$(id -u)"

launchctl kickstart -k "gui/$UID_VALUE/com.otterlink.core"
launchctl kickstart -k "gui/$UID_VALUE/com.otterlink.gateway"
