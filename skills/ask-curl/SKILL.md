---
name: ask-curl
description: >
  AI-assisted cURL requests. Describe what you want in natural language and get
  a well-formed cURL command. Supports secret injection via 1Password (op://),
  request history, response parsing, and chained requests.
tags: [http, api, curl, requests, 1password]
---

# Ask cURL

Make HTTP requests by describing what you want. Secrets stay safe via 1Password references.

## Quick Reference

### From natural language to cURL

1. User describes: "GET my GitHub repos", "POST to Slack webhook with message 'deployed'"
2. Agent builds the cURL command with proper headers, auth, body
3. Secrets injected at runtime via `op run` — never exposed in shell history or logs

### Secret injection

Use 1Password secret references (`op://vault/item/field`) instead of raw tokens:

```bash
# BAD — token in plaintext
curl -H "Authorization: <raw token redacted>" https://api.github.com/user

# GOOD — secret injected at runtime by op
op run --env-file=.env.curl -- curl -H "Authorization: Bearer $GITHUB_TOKEN" https://api.github.com/user
```

### Execution flow

```
describe request → build curl → inject secrets (op run) → execute → parse response
```

## Building Requests

### Standard patterns

```bash
# GET with auth
curl -s -H "Authorization: Bearer $TOKEN" \
  -H "Accept: application/json" \
  "https://api.example.com/resource"

# POST JSON
curl -s -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"key": "value"}' \
  "https://api.example.com/resource"

# File upload
curl -s -X POST \
  -H "Authorization: Bearer $TOKEN" \
  -F "file=@/path/to/file" \
  "https://api.example.com/upload"

# With query params
curl -s -G \
  --data-urlencode "q=search term" \
  --data-urlencode "limit=10" \
  "https://api.example.com/search"
```

### Always include

- `-s` (silent — no progress bar)
- `-w '\n%{http_code}'` (append status code for validation)
- `-H "Accept: application/json"` (when expecting JSON)
- `--fail-with-body` (fail on HTTP errors but still show response body)
- `--max-time 30` (timeout — never hang forever)

## Secret Management

### With 1Password (`op://` references)

The preferred approach. Secrets never touch disk or shell history.

```bash
# Read a single secret
op read "op://Development/GitHub PAT/credential"

# Run curl with secrets injected from 1Password Environment
op run --env-file=~/.config/ask-curl/env.curl -- curl -s \
  -H "Authorization: Bearer $GITHUB_TOKEN" \
  "https://api.github.com/user"

# Inline single secret
curl -s -H "Authorization: Bearer $(op read 'op://Development/GitHub PAT/credential')" \
  "https://api.github.com/user"
```

### Environment file format (`~/.config/ask-curl/env.curl`)

```bash
# 1Password secret references — resolved at runtime by `op run`
GITHUB_TOKEN=op://Development/GitHub PAT/credential
OPENAI_API_KEY=op://Development/OpenAI/api key
SLACK_WEBHOOK=op://Development/Slack Webhook/url
ANTHROPIC_API_KEY=op://Development/Anthropic/api key
```

### Without 1Password (fallback)

If `op` is unavailable, fall back to env vars (warn the user):

```bash
# Check if op is available
if command -v op &>/dev/null; then
  TOKEN=$(op read "op://Development/GitHub PAT/credential")
else
  echo "⚠ 1Password CLI not available — using env var fallback"
  TOKEN="${GITHUB_TOKEN:?GITHUB_TOKEN not set}"
fi
```

## Response Handling

### Parse JSON responses

```bash
# Pipe to jq for extraction
curl -s ... | jq '.data[] | {id, name}'

# Pretty print
curl -s ... | jq .

# Extract single field
curl -s ... | jq -r '.access_token'
```

### Validate responses

```bash
# Check HTTP status
response=$(curl -s -w '\n%{http_code}' ...)
http_code=$(echo "$response" | tail -1)
body=$(echo "$response" | sed '$d')

if [ "$http_code" -ge 400 ]; then
  echo "Error $http_code: $body"
fi
```

## Chained Requests

For multi-step flows (auth → use token → process):

```bash
# Step 1: Get auth token
TOKEN=$(curl -s -X POST \
  -d "grant_type=client_credentials" \
  -d "client_id=$(op read 'op://Dev/OAuth App/client_id')" \
  -d "client_secret=$(op read 'op://Dev/OAuth App/client_secret')" \
  "https://auth.example.com/token" | jq -r '.access_token')

# Step 2: Use token
curl -s -H "Authorization: Bearer $TOKEN" \
  "https://api.example.com/protected/resource"
```

## Request History

Log requests to `~/.config/ask-curl/history.jsonl` for replay:

```bash
# Append to history (sanitized — no secrets)
echo '{"ts":"'$(date -u +%Y-%m-%dT%H:%M:%SZ)'","method":"GET","url":"https://api.example.com/user","status":200}' \
  >> ~/.config/ask-curl/history.jsonl

# Replay last request
tail -1 ~/.config/ask-curl/history.jsonl | jq -r '.curl_command'
```

## Safety Rules

1. **NEVER echo, log, or display secrets** — use `op read` inline or `op run`
2. **NEVER write tokens to files** — use `op://` references in env files
3. **Always confirm before POST/PUT/PATCH/DELETE** — show the command first, ask to proceed
4. **Sanitize history** — strip Authorization headers and tokens before logging
5. **Timeout all requests** — `--max-time 30` minimum
6. **Warn on non-HTTPS** — flag any `http://` URL (except localhost)
