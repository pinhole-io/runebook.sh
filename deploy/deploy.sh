#!/usr/bin/env bash
# Provision the install host, then publish the script.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
"${SCRIPT_DIR}/provision.sh"
"${SCRIPT_DIR}/setup-server.sh"
