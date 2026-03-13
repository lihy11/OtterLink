#!/usr/bin/env bash
set -euo pipefail

systemctl reload otterlink-core.service
systemctl reload otterlink-gateway.service
