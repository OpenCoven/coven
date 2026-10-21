#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
if ! command -v python3 >/dev/null 2>&1; then
  printf '%s\n' 'FAIL verification: missing executable python3' >&2
  exit 127
fi
exec python3 "$repo_root/scripts/dev_verify.py" "$@"
