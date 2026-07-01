---
name: tinyfish-browser
description: Spin up a remote browser session via the TinyFish Browser API and get a CDP URL for driving it. Use when you need an automatable Chromium session (Playwright/Puppeteer) that auto-expires after ~1h idle.
homepage: https://docs.tinyfish.ai/browser-api
requires:
  env:
    - TINYFISH_API_KEY
---

# TinyFish Browser

Create a remote Chromium browser session. Returns a `session_id`, a `cdp_url` for driving the browser over the DevTools Protocol, and an authenticated `base_url` for polling session state.

Requires: `TINYFISH_API_KEY` environment variable.

## Pre-flight Check (REQUIRED)

Before calling the API, verify the key is present:

```bash
[ -n "$TINYFISH_API_KEY" ] && echo "TINYFISH_API_KEY is set" || echo "TINYFISH_API_KEY is NOT set"
```

If the key is not set, stop and ask the user to add it. Get one at <https://agent.tinyfish.ai/api-keys>. Do NOT fall back to other browser tools.

## Create a Session

```bash
curl -X POST "https://api.browser.tinyfish.ai" \
  -H "X-API-Key: $TINYFISH_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{ "url": "https://example.com" }'
```

## Helper Script

`scripts/browser.sh <url>` wraps the curl call:

```bash
scripts/browser.sh https://example.com
```

## Response Shape

```json
{
  "session_id": "sess_abc123",
  "cdp_url": "wss://browser.tinyfish.ai/devtools/browser/…",
  "base_url": "https://api.browser.tinyfish.ai/sessions/sess_abc123"
}
```

## Using the Session

- `cdp_url` is a DevTools Protocol websocket — connect with Playwright/Puppeteer/chrome-remote-interface to drive the page.
- `base_url` is an **authenticated** polling endpoint (requires the `X-API-Key` header). It is NOT browsable in a normal web view.

## Session Lifecycle

Sessions auto-close after ~1 hour of idleness. There is no explicit terminate endpoint — avoid creating one session per user action; reuse sessions keyed by target URL when possible.
