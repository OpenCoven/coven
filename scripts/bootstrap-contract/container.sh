#!/usr/bin/env bash
set -euo pipefail

# Only fixed categories reach the public receipt. Keep package-manager output,
# which may include configured mirror URLs or private diagnostics, in the container.
report_prerequisite_failure() {
  local stage="$1" status="$2" log="$3" category=package-manager
  if LC_ALL=C grep -Eiq 'NO_PUBKEY|EXPKEYSIG|BADSIG|not signed|signatures.*verified|Release file.*not valid yet' "$log"; then
    category=index-authentication
  elif LC_ALL=C grep -Eiq 'Hash Sum mismatch|File has unexpected size' "$log"; then
    category=index-integrity
  elif LC_ALL=C grep -Eiq 'No space left on device|Read-only file system' "$log"; then
    category=storage
  elif LC_ALL=C grep -Eiq 'Could not get lock|Unable to acquire.*lock' "$log"; then
    category=package-manager-lock
  elif LC_ALL=C grep -Eiq 'Temporary failure resolving|Could not resolve' "$log"; then
    category=resolution
  elif LC_ALL=C grep -Eiq 'Could not connect|Failed to connect|Connection (timed out|failed|refused)|Could not handshake|Network is unreachable' "$log"; then
    category=transport
  elif LC_ALL=C grep -Eiq 'Unable to locate package|has no installation candidate|404[[:space:]]+Not Found' "$log"; then
    category=package-unavailable
  elif LC_ALL=C grep -Eiq 'Unmet dependencies|held broken packages' "$log"; then
    category=package-resolution
  fi
  printf '{"status":"failed","code":"container-prerequisites","stage":"%s","package_manager_exit_code":%s,"diagnostic":{"code":"%s"}}\n' "$stage" "$status" "$category"
}

install_prerequisites() {
  local log="$1" status
  if apt-get update > "$log" 2>&1; then
    :
  else
    status=$?
    report_prerequisite_failure apt-update "$status" "$log"
    return 1
  fi
  if apt-get install -y --no-install-recommends python3 git ca-certificates bash util-linux >> "$log" 2>&1; then
    return 0
  else
    status=$?
    report_prerequisite_failure apt-install "$status" "$log"
    return 1
  fi
}

# Sourcing exposes only the helpers for host-side failure/privacy tests.
if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then
  return 0
fi

# This entrypoint runs only inside the disposable pinned image. The host source
# is read-only, and no Docker socket or host home is mounted.
if prerequisite_report="$(install_prerequisites /tmp/contract-prerequisites.log)"; then
  :
else
  printf '%s\n' "$prerequisite_report" > /evidence/result.json
  exit 1
fi
mkdir -p /work/repo
cp -a /source/. /work/repo/
chown -R 0:0 /work/repo
cd /work/repo
exec python3 scripts/bootstrap-contract/inside.py --output /evidence
