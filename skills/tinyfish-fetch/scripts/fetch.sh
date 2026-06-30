#!/usr/bin/env bash
#
# TinyFish Fetch API helper.
#
# Usage:
#   fetch.sh <url> [<url> ...] [--format markdown|html|screenshot] [--country CC]
#
# Examples:
#   fetch.sh https://example.com
#   fetch.sh https://example.com https://example.org --format html
#   fetch.sh https://example.com --format markdown --country US

set -euo pipefail

if [ $# -lt 1 ]; then
  echo "Usage: fetch.sh <url> [<url> ...] [--format FMT] [--country CC]" >&2
  exit 1
fi

if [ -z "${TINYFISH_API_KEY:-}" ]; then
  echo "Error: TINYFISH_API_KEY environment variable not set" >&2
  exit 1
fi

URLS=()
FORMAT="markdown"
COUNTRY=""

while [ $# -gt 0 ]; do
  case "$1" in
    --format)
      FORMAT="$2"
      shift 2
      ;;
    --country)
      COUNTRY="$2"
      shift 2
      ;;
    --*)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
    *)
      URLS+=("$1")
      shift
      ;;
  esac
done

if [ ${#URLS[@]} -lt 1 ]; then
  echo "Error: at least one URL is required" >&2
  exit 1
fi

URLS_JSON=$(printf '%s\n' "${URLS[@]}" | python3 -c 'import json,sys; print(json.dumps([l.strip() for l in sys.stdin if l.strip()]))')

if [ -n "$COUNTRY" ]; then
  BODY=$(python3 -c "import json,sys; print(json.dumps({'urls': json.loads(sys.argv[1]), 'format': sys.argv[2], 'proxy_config': {'country': sys.argv[3]}}))" "$URLS_JSON" "$FORMAT" "$COUNTRY")
else
  BODY=$(python3 -c "import json,sys; print(json.dumps({'urls': json.loads(sys.argv[1]), 'format': sys.argv[2]}))" "$URLS_JSON" "$FORMAT")
fi

exec curl -s -X POST "https://api.fetch.tinyfish.ai" \
  -H "X-API-Key: ${TINYFISH_API_KEY}" \
  -H "Content-Type: application/json" \
  -d "$BODY"
