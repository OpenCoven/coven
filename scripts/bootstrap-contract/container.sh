#!/usr/bin/env bash
set -euo pipefail

# This script runs only inside the disposable pinned image. The host source is
# read-only, and no Docker socket or host home is mounted.
if ! apt-get update > /tmp/contract-prerequisites.log 2>&1 ||
   ! apt-get install -y --no-install-recommends python3 git ca-certificates bash util-linux >> /tmp/contract-prerequisites.log 2>&1; then
  printf '%s\n' '{"status":"failed","code":"container-prerequisites"}' > /evidence/result.json
  exit 1
fi
mkdir -p /work/repo
cp -a /source/. /work/repo/
chown -R 0:0 /work/repo
cd /work/repo
exec python3 scripts/bootstrap-contract/inside.py --output /evidence
