#!/usr/bin/env bash
# Scan openclaw.json for known anti-patterns and misconfigurations.
# Outputs findings as structured text. Never displays secret values.
set -euo pipefail

CONFIG="${OPENCLAW_CONFIG:-$HOME/.openclaw/openclaw.json}"

if [[ ! -f "$CONFIG" ]]; then
  echo "ERROR: Config not found at $CONFIG"
  exit 1
fi

python3 << 'PYEOF'
import json, sys, os

config_path = os.environ.get("OPENCLAW_CONFIG", os.path.expanduser("~/.openclaw/openclaw.json"))

with open(config_path) as f:
    config = json.load(f)

findings = []

# Gateway checks
gw = config.get("gateway", {})
cui = gw.get("controlUi", {})

if cui.get("dangerouslyDisableDeviceAuth") is True:
    findings.append(("CRITICAL", "gateway.controlUi.dangerouslyDisableDeviceAuth is TRUE — device auth bypassed"))

if cui.get("dangerouslyAllowHostHeaderOriginFallback") is True:
    findings.append(("WARN", "gateway.controlUi.dangerouslyAllowHostHeaderOriginFallback is TRUE — DNS rebinding risk"))

origins = cui.get("allowedOrigins", [])
if "*" in origins:
    findings.append(("WARN", f"gateway.controlUi.allowedOrigins contains '*' — any browser origin allowed"))

proxies = gw.get("trustedProxies", [])
if not proxies and gw.get("mode") == "local":
    findings.append(("INFO", "gateway.trustedProxies is empty — header spoofing possible if behind a proxy"))

# Auth checks
auth = gw.get("auth", {})
if auth.get("mode") == "none":
    findings.append(("CRITICAL", "gateway.auth.mode is 'none' — no authentication"))
elif auth.get("mode") in ("token", "password"):
    token = auth.get("token", auth.get("password", ""))
    if token and not token.startswith("op://"):
        findings.append(("WARN", f"gateway.auth.{auth['mode']} is plaintext (length={len(token)}) — consider 1Password op:// ref"))

# Channel checks
channels = config.get("channels", {})
for ch_name, ch_config in channels.items():
    if not isinstance(ch_config, dict):
        continue
    if not ch_config.get("enabled", False):
        continue

    dm_policy = ch_config.get("dmPolicy", "pairing")
    if dm_policy == "open":
        findings.append(("WARN", f"channels.{ch_name}.dmPolicy is 'open' — anyone can DM"))

    group_policy = ch_config.get("groupPolicy", "allowlist")
    if group_policy == "open":
        findings.append(("WARN", f"channels.{ch_name}.groupPolicy is 'open' — any group can interact"))

    # Check for plaintext bot tokens
    for key in ["botToken", "token", "apiKey"]:
        val = ch_config.get(key, "")
        if val and isinstance(val, str) and not val.startswith("op://") and len(val) > 8:
            findings.append(("WARN", f"channels.{ch_name}.{key} is plaintext (length={len(val)})"))

# Agent/sandbox checks
agents = config.get("agents", {})
defaults = agents.get("defaults", {})
sandbox = defaults.get("sandbox", {})
if sandbox.get("mode") == "off" or not sandbox:
    findings.append(("INFO", "agents.defaults.sandbox.mode is off or unset — no sandboxing"))

fs_config = defaults.get("tools", {}).get("fs", {})
if fs_config.get("workspaceOnly") is False:
    findings.append(("INFO", "agents.defaults.tools.fs.workspaceOnly is false — full filesystem access"))

# Summary
critical = sum(1 for s, _ in findings if s == "CRITICAL")
warn = sum(1 for s, _ in findings if s == "WARN")
info = sum(1 for s, _ in findings if s == "INFO")

print(f"Config Audit: {critical} critical, {warn} warn, {info} info")
print()
for severity, message in findings:
    print(f"  [{severity}] {message}")

if not findings:
    print("  No issues found.")
PYEOF
