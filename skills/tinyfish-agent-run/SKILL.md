---
name: tinyfish-agent-run
description: Run a TinyFish automation agent against a URL and stream live progress events (STARTED / PROGRESS / COMPLETE) as one-line JSON events. Use when you want to execute an autonomous browsing goal and surface each step as it happens.
homepage: https://docs.tinyfish.ai/agent-api
requires:
  env:
    - TINYFISH_API_KEY
---

# TinyFish Agent Run

Execute an autonomous browsing goal against a URL and stream live events. Each SSE event is re-emitted on stdout as a single JSON line so a caller (subagent, TUI, etc.) can display progress as it arrives.

Requires: `TINYFISH_API_KEY` environment variable.

## Pre-flight Check (REQUIRED)

```bash
[ -n "$TINYFISH_API_KEY" ] && echo "TINYFISH_API_KEY is set" || echo "TINYFISH_API_KEY is NOT set"
```

If the key is not set, stop and ask the user to add it. Get one at <https://agent.tinyfish.ai/api-keys>.

## Streaming Run

```bash
scripts/agent-run.sh <url> <goal>
```

Example:

```bash
scripts/agent-run.sh https://scrapeme.live/shop "Extract the first 2 product names and prices. Return JSON."
```

Under the hood this POSTs to `https://agent.tinyfish.ai/v1/automation/run-sse` with:

```json
{
  "url": "https://scrapeme.live/shop",
  "goal": "Extract the first 2 product names and prices. Return JSON."
}
```

and the header `X-API-Key: $TINYFISH_API_KEY`.

## Output Format

One JSON object per line on stdout:

```json
{"type":"STARTED","run_id":"abc123"}
{"type":"PROGRESS","run_id":"abc123","purpose":"Navigating to https://scrapeme.live/shop"}
{"type":"PROGRESS","run_id":"abc123","purpose":"Reading product list"}
{"type":"COMPLETE","run_id":"abc123","status":"COMPLETED","result":{...}}
```

A caller can:
- Render each `PROGRESS.purpose` as a live status line.
- Detect navigation by matching `purpose` against `Navigating to <url>` and surface the URL to an embedded browser view.
- Parse `COMPLETE.result` for the final payload.

## Event Types

- `STARTED` — `{type, run_id}` — emitted once when the run begins.
- `PROGRESS` — `{type, run_id, purpose}` — one per agent step. `purpose` is a short human-readable description.
- `COMPLETE` — `{type, run_id, status, result}` — emitted once when the run finishes. `status` is typically `COMPLETED`.
