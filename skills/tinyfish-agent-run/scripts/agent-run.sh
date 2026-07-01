#!/usr/bin/env bash
#
# TinyFish Agent SSE streamer.
#
# Usage:
#   agent-run.sh <url> <goal>
#
# Example:
#   agent-run.sh https://scrapeme.live/shop "Extract the first 2 product names."
#
# Emits one JSON object per line on stdout — the raw SSE event payload.

set -euo pipefail

if [ $# -lt 2 ]; then
  echo "Usage: agent-run.sh <url> <goal>" >&2
  exit 1
fi

if [ -z "${TINYFISH_API_KEY:-}" ]; then
  echo "Error: TINYFISH_API_KEY environment variable not set" >&2
  exit 1
fi

URL="$1"
GOAL="$2"

BODY=$(python3 -c "import json,sys; print(json.dumps({'url': sys.argv[1], 'goal': sys.argv[2]}))" "$URL" "$GOAL")

# Each SSE frame starts with "data: ". Strip that prefix, drop blank lines and
# keepalive comments, and print the JSON payload one-per-line so callers can
# consume events as they stream in.
curl -sN -X POST "https://agent.tinyfish.ai/v1/automation/run-sse" \
  -H "X-API-Key: ${TINYFISH_API_KEY}" \
  -H "Content-Type: application/json" \
  -H "Accept: text/event-stream" \
  -d "$BODY" \
  | awk '
      /^data: / {
        line = substr($0, 7)
        if (length(line) > 0) {
          print line
          fflush()
        }
      }
  '
