#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

echo "workspace: $ROOT"
command -v python3 >/dev/null && echo "python3: ok"
command -v npx >/dev/null && echo "npx: ok"
if command -v ffmpeg >/dev/null; then
  echo "ffmpeg: ok"
else
  echo "ffmpeg: missing"
fi
if [ -f .env ]; then
  echo ".env: present"
  grep -q '^HF_API_KEY_ID=.' .env && echo "HF_API_KEY_ID: present" || echo "HF_API_KEY_ID: missing"
  grep -q '^HF_API_SECRET=.' .env && echo "HF_API_SECRET: present" || echo "HF_API_SECRET: missing"
  grep -q '^ELEVENLABS_API_KEY=.' .env && echo "ELEVENLABS_API_KEY: present" || echo "ELEVENLABS_API_KEY: optional/missing"
else
  echo ".env: missing; copy .env.example and fill it with your own keys"
fi
./scripts/higgsfield_local.sh --help >/dev/null 2>&1 && echo "higgsfield cli: reachable"
