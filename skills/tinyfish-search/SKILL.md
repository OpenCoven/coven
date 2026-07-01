---
name: tinyfish-search
description: Run web searches via TinyFish Search API and get structured JSON results (title, snippet, URL) ready for LLM consumption. Use when you need fresh web search results with reliable structure and optional geo/language targeting.
homepage: https://docs.tinyfish.ai/search-api
requires:
  env:
    - TINYFISH_API_KEY
---

# TinyFish Search

Search the web via the TinyFish Search API. Returns a JSON array of ranked results with title, snippet, site name, and URL.

Requires: `TINYFISH_API_KEY` environment variable.

## Pre-flight Check (REQUIRED)

Before calling the API, verify the key is present:

```bash
[ -n "$TINYFISH_API_KEY" ] && echo "TINYFISH_API_KEY is set" || echo "TINYFISH_API_KEY is NOT set"
```

If the key is not set, stop and ask the user to add it. Get one at <https://agent.tinyfish.ai/api-keys>. Do NOT fall back to other search tools.

## Basic Search

```bash
curl -G "https://api.search.tinyfish.ai" \
  -H "X-API-Key: $TINYFISH_API_KEY" \
  --data-urlencode "query=web automation tools"
```

## Geo / Language Targeting

```bash
curl -G "https://api.search.tinyfish.ai" \
  -H "X-API-Key: $TINYFISH_API_KEY" \
  --data-urlencode "query=boulangerie paris" \
  --data-urlencode "location=FR" \
  --data-urlencode "language=fr"
```

## Helper Script

`scripts/search.sh <query> [--location CC] [--language LL]` wraps the curl call:

```bash
scripts/search.sh "top trending openclaw youtube video"
scripts/search.sh "boulangerie paris" --location FR --language fr
```

## Response Shape

```json
{
  "query": "web automation tools",
  "results": [
    {
      "position": 1,
      "site_name": "example.com",
      "title": "Top 10 Web Automation Tools",
      "snippet": "A preview of the page content…",
      "url": "https://example.com/top-automation"
    }
  ],
  "total_results": 1234
}
```

Use `results[]` directly — each entry already has everything needed to cite or open the source.
