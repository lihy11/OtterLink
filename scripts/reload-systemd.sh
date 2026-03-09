#!/usr/bin/env bash
set -euo pipefail

systemctl reload remoteagent-core.service
systemctl reload remoteagent-gateway.service
