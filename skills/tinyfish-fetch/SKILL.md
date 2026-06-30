---
name: tinyfish-fetch
description: Fetch web pages via TinyFish Fetch API and get clean markdown, HTML, or screenshots back — handles JS rendering and proxying. Use when you need to read a specific URL's content reliably.
homepage: https://docs.tinyfish.ai/fetch-api
requires:
  env:
    - TINYFISH_API_KEY
---

# TinyFish Fetch

Fetch one or more URLs via the TinyFish Fetch API. Returns rendered page content (markdown by default), optionally with screenshots, behind an optional proxy.

Requires: `TINYFISH_API_KEY` environment variable.

## Pre-flight Check (REQUIRED)

Before calling the API, verify the key is present:

```bash
[ -n "$TINYFISH_API_KEY" ] && echo "TINYFISH_API_KEY is set" || echo "TINYFISH_API_KEY is NOT set"
```

If the key is not set, stop and ask the user to add it. Get one at <https://agent.tinyfish.ai/api-keys>. Do NOT fall back to other fetch tools.

## Basic Fetch

```bash
curl -X POST "https://api.fetch.tinyfish.ai" \
  -H "X-API-Key: $TINYFISH_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "urls": ["https://example.com"],
    "format": "markdown"
  }'
```

`format` accepts `markdown` (default), `html`, or `screenshot`.

## Multiple URLs

```bash
curl -X POST "https://api.fetch.tinyfish.ai" \
  -H "X-API-Key: $TINYFISH_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "urls": [
      "https://example.com",
      "https://example.org"
    ],
    "format": "markdown"
  }'
```

## Proxy / Geo Targeting

```bash
curl -X POST "https://api.fetch.tinyfish.ai" \
  -H "X-API-Key: $TINYFISH_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "urls": ["https://example.com"],
    "format": "markdown",
    "proxy_config": { "country": "US" }
  }'
```

## Helper Script

`scripts/fetch.sh <url> [<url> ...] [--format markdown|html|screenshot] [--country CC]` wraps the curl call:

```bash
scripts/fetch.sh https://example.com
scripts/fetch.sh https://example.com https://example.org --format html
scripts/fetch.sh https://example.com --format markdown --country US
```

## Response Shape

```json
{
  "results": [
    {
      "url": "https://example.com",
      "status": 200,
      "content": "# Example Domain\n\nThis domain is for use…",
      "format": "markdown"
    }
  ]
}
```

Read `results[].content` for the rendered page. Screenshot format returns base64-encoded PNG data.
