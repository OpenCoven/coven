---
name: 1password
description: >
  Manage secrets via 1Password CLI (op). Read, create, and inject secrets.
  Provides whitelisted access for other skills (ask-curl, GitHub, etc.)
  via op:// secret references and 1Password Environments.
tags: [secrets, 1password, security, env, credentials]
---

# 1Password Secret Management

Securely store and retrieve secrets. Never expose them in chat, logs, or files.

## Prerequisites

- `op` CLI installed: `brew install --cask 1password-cli`
- Signed in: `op signin` (or service account via `OP_SERVICE_ACCOUNT_TOKEN`)
- For biometric unlock: enable in 1Password desktop app → Settings → Developer → CLI integration

## Core Commands

### Read a secret

```bash
op read "op://VaultName/ItemName/FieldName"
```

### List vaults

```bash
op vault list --format json | jq '.[].name'
```

### List items in a vault

```bash
op item list --vault "Development" --format json | jq '.[].title'
```

### Create an item

```bash
op item create \
  --category login \
  --vault "Development" \
  --title "Service API Key" \
  --field "credential=secret_value_here"
```

### Search items

```bash
op item list --format json | jq '.[] | select(.title | test("github"; "i"))'
```

## Environment Files (Secret References)

Create env files with `op://` references — secrets are resolved at runtime, never stored on disk.

### Setup for ask-curl

```bash
mkdir -p ~/.config/ask-curl

# Create env file with secret references (NOT actual secrets)
cat > ~/.config/ask-curl/env.curl << 'ENV'
GITHUB_TOKEN=op://Development/GitHub PAT/credential
OPENAI_API_KEY=op://Development/OpenAI/api key
SLACK_WEBHOOK=op://Development/Slack Webhook/url
ENV
```

### Use with `op run`

```bash
# Inject secrets from env file into a command
op run --env-file=~/.config/ask-curl/env.curl -- curl -s \
  -H "Authorization: Bearer $GITHUB_TOKEN" \
  "https://api.github.com/user"

# Inject secrets into any command
op run --env-file=~/.config/ask-curl/env.curl -- node script.js
```

## Skill Whitelisting

Control which skills can access which secrets by using separate env files per skill:

```
~/.config/ask-curl/env.curl       # HTTP/API secrets for ask-curl
~/.config/github/env.gh           # GitHub-specific secrets
~/.config/openclaw/env.gateway    # Gateway/infra secrets
```

Each env file contains only the `op://` references that skill needs — principle of least privilege.

### Register a new secret for a skill

```bash
# 1. Store the secret in 1Password
op item create --vault "Development" --category login \
  --title "New Service" --field "api_key=<secret-value>"

# 2. Add the reference to the skill's env file
echo 'NEW_SERVICE_KEY=op://Development/New Service/api_key' >> ~/.config/ask-curl/env.curl
```

## 1Password Environments (Teams/Pro)

For team workflows, use 1Password Environments instead of local env files:

```bash
# List environments
op environment list

# Run with an environment
op run --environment "Production" -- ./deploy.sh
```

## Service Accounts (Automation)

For unattended/cron usage where biometric auth isn't available:

```bash
# Create a service account (one-time, from 1Password web)
# Then set the token:
export OP_SERVICE_ACCOUNT_TOKEN="ops_..."

# Now op commands work without interactive auth
op read "op://AutomationVault/Deploy Key/credential"
```

## Safety Rules

1. **NEVER display secret values** — use `op read` only in subshells or pipes
2. **NEVER store secrets in files** — only `op://` references in env files
3. **NEVER commit env files with real secrets** — only `op://` refs are safe to commit
4. **Always use `--vault`** when creating items to avoid putting secrets in the wrong vault
5. **Verify before creating** — `op item list --vault X` to check for duplicates first
6. **Use separate vaults** for different trust levels (Development, Production, Personal)
