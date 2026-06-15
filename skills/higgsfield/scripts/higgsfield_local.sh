#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STATE_DIR="$ROOT/.higgsfield-local"

mkdir -p "$STATE_DIR/home" "$STATE_DIR/config" "$STATE_DIR/npm-cache"

export HOME="$STATE_DIR/home"
export XDG_CONFIG_HOME="$STATE_DIR/config"
export npm_config_cache="$STATE_DIR/npm-cache"

exec npx -y -p @higgsfield/cli higgsfield "$@"
