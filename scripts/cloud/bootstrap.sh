#!/usr/bin/env bash
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# Prefer the environment's configured Python; never silently install a parser
# dependency or continue with a partial interpretation of repository TOML.
for candidate in python3 python3.14 python3.13 python3.12 python3.11; do
  if command -v "$candidate" >/dev/null 2>&1 && "$candidate" -c 'import tomllib' >/dev/null 2>&1; then
    export PYTHONDONTWRITEBYTECODE=1
    exec "$candidate" "$script_dir/bootstrap.py" "$@"
  fi
done
printf '%s\n' '{"development_ready":false,"blocked_stage":"python","diagnostic":"Python 3.11+ is required in the managed setup phase"}'
exit 1
