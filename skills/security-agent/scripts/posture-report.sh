#!/usr/bin/env bash
# Generate a security posture summary for OpenClaw deployment.
# Collects data from multiple sources without exposing secrets.
set -euo pipefail

DATE=$(date '+%Y-%m-%d %H:%M %Z')
echo "# Security Posture Report — $DATE"
echo ""

# 1. OpenClaw Security Audit
echo "## 1. OpenClaw Security Audit"
if command -v openclaw &>/dev/null; then
  AUDIT_JSON=$(openclaw security audit --json 2>/dev/null || echo '{"error": true}')
  if echo "$AUDIT_JSON" | python3 -c "import sys,json; d=json.load(sys.stdin); print(f\"Summary: {d.get('summary',{}).get('critical',0)} critical, {d.get('summary',{}).get('warn',0)} warn, {d.get('summary',{}).get('info',0)} info\")" 2>/dev/null; then
    :
  else
    echo "Could not parse audit JSON — running text mode:"
    openclaw security audit 2>&1 | grep -E '(CRITICAL|WARN|INFO|Summary)' || echo "Audit unavailable"
  fi
else
  echo "SKIP: openclaw CLI not found"
fi
echo ""

# 2. OpenClaw Version
echo "## 2. Version Status"
if command -v openclaw &>/dev/null; then
  openclaw update status 2>&1 | grep -E '(Channel|Update|Install)' || echo "Version check unavailable"
else
  echo "SKIP: openclaw CLI not found"
fi
echo ""

# 3. Host Security
echo "## 3. Host Security"

# OS
echo "### OS"
uname -mrs 2>/dev/null || echo "Unknown OS"

# Firewall (macOS)
echo "### Firewall"
if [[ "$(uname -s)" == "Darwin" ]]; then
  /usr/libexec/ApplicationFirewall/socketfilterfw --getglobalstate 2>/dev/null || echo "Could not check firewall"
else
  ufw status 2>/dev/null || firewall-cmd --state 2>/dev/null || echo "Firewall status unknown"
fi

# Disk encryption
echo "### Disk Encryption"
if [[ "$(uname -s)" == "Darwin" ]]; then
  fdesetup status 2>/dev/null || echo "Could not check FileVault"
else
  lsblk -f 2>/dev/null | grep -i crypt && echo "Encryption detected" || echo "No LUKS encryption detected"
fi

# Listening ports (count only)
echo "### Listening Ports"
if [[ "$(uname -s)" == "Darwin" ]]; then
  PORT_COUNT=$(lsof -nP -iTCP -sTCP:LISTEN 2>/dev/null | tail -n +2 | wc -l | tr -d ' ')
  echo "Listening TCP ports: $PORT_COUNT"
else
  PORT_COUNT=$(ss -ltnp 2>/dev/null | tail -n +2 | wc -l | tr -d ' ')
  echo "Listening TCP ports: $PORT_COUNT"
fi

echo ""
echo "## 4. Summary"
echo "Report generated at $DATE"
echo "Run 'openclaw security audit --fix' to apply safe defaults (OpenClaw-only, does not change host OS)."
