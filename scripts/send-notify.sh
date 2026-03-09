#!/usr/bin/env bash
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "usage: $0 'message text' [open_id] [title]"
  exit 1
fi

: "${BIND:=127.0.0.1:1127}"
: "${BRIDGE_NOTIFY_TOKEN:?missing BRIDGE_NOTIFY_TOKEN}"

text="$1"
open_id="${2:-}"
title="${3:-}"

body="$(jq -nc --arg text "$text" --arg open_id "$open_id" --arg title "$title" '
  {
    text: $text,
    open_id: ($open_id | select(length > 0)),
    title: ($title | select(length > 0))
  }
')"

curl -sS -X POST "http://$BIND/internal/notify" \
  -H "content-type: application/json" \
  -H "x-notify-token: $BRIDGE_NOTIFY_TOKEN" \
  -d "$body"
echo
