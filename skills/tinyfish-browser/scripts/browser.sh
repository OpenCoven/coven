#!/usr/bin/env bash
#
# TinyFish Browser API helper.
#
# Usage:
#   browser.sh <url>
#
# Example:
#   browser.sh https://example.com

set -euo pipefail

if [ $# -lt 1 ]; then
  echo "Usage: browser.sh <url>" >&2
  exit 1
fi

if [ -z "${TINYFISH_API_KEY:-}" ]; then
  echo "Error: TINYFISH_API_KEY environment variable not set" >&2
  exit 1
fi

URL="$1"

BODY=$(python3 -c "import json,sys; print(json.dumps({'url': sys.argv[1]}))" "$URL")

exec curl -s -X POST "https://api.browser.tinyfish.ai" \
  -H "X-API-Key: ${TINYFISH_API_KEY}" \
  -H "Content-Type: application/json" \
  -d "$BODY"
