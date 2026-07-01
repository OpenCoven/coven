#!/usr/bin/env bash
# Check for plaintext secrets in OpenClaw config and common locations.
# NEVER outputs actual secret values — only counts and field paths.
set -euo pipefail

CONFIG="${OPENCLAW_CONFIG:-$HOME/.openclaw/openclaw.json}"
FINDINGS=0

echo "=== Plaintext Secret Scan ==="
echo "Config: $CONFIG"
echo ""

# Check if config exists
if [[ ! -f "$CONFIG" ]]; then
  echo "SKIP: Config file not found at $CONFIG"
  exit 0
fi

# Check config file permissions
PERMS=$(stat -f '%Lp' "$CONFIG" 2>/dev/null || stat -c '%a' "$CONFIG" 2>/dev/null || echo "unknown")
echo "Config file permissions: $PERMS"
if [[ "$PERMS" != "600" && "$PERMS" != "640" && "$PERMS" != "unknown" ]]; then
  echo "WARN: Config file permissions are $PERMS (recommended: 600)"
  FINDINGS=$((FINDINGS + 1))
fi

# Check for plaintext tokens in config (field paths only, never values)
echo ""
echo "--- Config Field Scan ---"

# Use python3 to safely inspect JSON without exposing values
python3 -c "
import json, sys, re

def scan(obj, path=''):
    if isinstance(obj, dict):
        for k, v in obj.items():
            p = f'{path}.{k}' if path else k
            if isinstance(v, str) and len(v) > 8:
                lower_k = k.lower()
                secret_keys = ['token', 'secret', 'password', 'apikey', 'api_key', 'bottoken', 'botToken', 'auth']
                if any(sk in lower_k for sk in secret_keys):
                    if not v.startswith('op://'):
                        print(f'WARN: Plaintext secret at {p} (length={len(v)})')
            scan(v, p)
    elif isinstance(obj, list):
        for i, v in enumerate(obj):
            scan(v, f'{path}[{i}]')

with open(sys.argv[1]) as f:
    config = json.load(f)
    scan(config)
" "$CONFIG" 2>/dev/null || echo "SKIP: Could not parse config as JSON"

# Check identity directory permissions
echo ""
echo "--- Identity Directory ---"
ID_DIR="$HOME/.openclaw/identity"
if [[ -d "$ID_DIR" ]]; then
  DIR_PERMS=$(stat -f '%Lp' "$ID_DIR" 2>/dev/null || stat -c '%a' "$ID_DIR" 2>/dev/null || echo "unknown")
  echo "Identity dir permissions: $DIR_PERMS"
  if [[ "$DIR_PERMS" != "700" && "$DIR_PERMS" != "unknown" ]]; then
    echo "WARN: Identity directory permissions are $DIR_PERMS (recommended: 700)"
    FINDINGS=$((FINDINGS + 1))
  fi
else
  echo "INFO: Identity directory not found (may be normal)"
fi

# Check for secrets in environment (count only)
echo ""
echo "--- Environment Variable Scan ---"
SECRET_ENV_COUNT=$(env | grep -icE '^(.*TOKEN|.*SECRET|.*KEY|.*PASSWORD)=' 2>/dev/null || echo "0")
echo "Environment variables matching secret patterns: $SECRET_ENV_COUNT"

# Check shell history (count only, never display)
echo ""
echo "--- Shell History Scan ---"
for HIST_FILE in "$HOME/.zsh_history" "$HOME/.bash_history"; do
  if [[ -f "$HIST_FILE" ]]; then
    COUNT=$(grep -cE '(token=|key=|secret=|password=|Bearer [A-Za-z0-9])' "$HIST_FILE" 2>/dev/null || true)
    COUNT="${COUNT:-0}"
    COUNT=$(echo "$COUNT" | tr -d '[:space:]')
    echo "$(basename "$HIST_FILE"): $COUNT lines with potential secret patterns"
    if [[ "$COUNT" -gt 0 ]]; then
      FINDINGS=$((FINDINGS + 1))
    fi
  fi
done

echo ""
echo "=== Scan Complete: $FINDINGS potential issues ==="
