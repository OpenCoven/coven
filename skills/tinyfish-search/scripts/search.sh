#!/usr/bin/env bash
#
# TinyFish Search API helper.
#
# Usage:
#   search.sh <query> [--location COUNTRY_CODE] [--language LANG_CODE]
#
# Examples:
#   search.sh "top trending openclaw youtube video"
#   search.sh "boulangerie paris" --location FR --language fr

set -euo pipefail

if [ $# -lt 1 ]; then
  echo "Usage: search.sh <query> [--location CC] [--language LL]" >&2
  exit 1
fi

if [ -z "${TINYFISH_API_KEY:-}" ]; then
  echo "Error: TINYFISH_API_KEY environment variable not set" >&2
  exit 1
fi

QUERY="$1"
shift

LOCATION=""
LANGUAGE=""

while [ $# -gt 0 ]; do
  case "$1" in
    --location)
      LOCATION="$2"
      shift 2
      ;;
    --language)
      LANGUAGE="$2"
      shift 2
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

ARGS=(-G "https://api.search.tinyfish.ai"
  -H "X-API-Key: ${TINYFISH_API_KEY}"
  --data-urlencode "query=${QUERY}")

if [ -n "$LOCATION" ]; then
  ARGS+=(--data-urlencode "location=${LOCATION}")
fi
if [ -n "$LANGUAGE" ]; then
  ARGS+=(--data-urlencode "language=${LANGUAGE}")
fi

exec curl -s "${ARGS[@]}"
