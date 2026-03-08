#!/usr/bin/env bash
set -euo pipefail

: "${BIND:=127.0.0.1:3000}"
: "${BRIDGE_NOTIFY_TOKEN:?missing BRIDGE_NOTIFY_TOKEN}"

open_id="${1:-}"
base_url="http://$BIND/internal/notify"

send() {
  local text="$1"
  local title="${2:-}"
  local body
  body="$(jq -nc --arg text "$text" --arg title "$title" --arg open_id "$open_id" '
    {
      text: $text,
      title: ($title | select(length > 0)),
      open_id: ($open_id | select(length > 0))
    }
  ')"
  curl -sS -X POST "$base_url" \
    -H "content-type: application/json" \
    -H "x-notify-token: $BRIDGE_NOTIFY_TOKEN" \
    -d "$body"
  echo
}

send "[text] 这是一条普通文本测试。"
send "[post] 这是一条带标题的富文本测试。" "Gateway Notify"
