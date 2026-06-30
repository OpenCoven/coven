#!/bin/bash
# Fetch and cache OpenClaw docs for the openclaw-dev agent
# Run periodically (daily via cron/heartbeat) to keep docs fresh

CACHE_DIR="${HOME}/.openclaw/workspace/cache"
CACHE_FILE="${CACHE_DIR}/openclaw-docs.txt"
DOCS_URL="https://docs.openclaw.ai/llms-full.txt"
MAX_AGE_HOURS=24

mkdir -p "$CACHE_DIR"

# Check if cache is fresh enough
if [ -f "$CACHE_FILE" ]; then
  age_seconds=$(( $(date +%s) - $(stat -f %m "$CACHE_FILE" 2>/dev/null || stat -c %Y "$CACHE_FILE" 2>/dev/null) ))
  age_hours=$(( age_seconds / 3600 ))
  if [ "$age_hours" -lt "$MAX_AGE_HOURS" ]; then
    echo "Cache is fresh (${age_hours}h old, max ${MAX_AGE_HOURS}h). Skipping fetch."
    exit 0
  fi
fi

echo "Fetching OpenClaw docs from ${DOCS_URL}..."
if curl -sL "$DOCS_URL" -o "${CACHE_FILE}.tmp"; then
  mv "${CACHE_FILE}.tmp" "$CACHE_FILE"
  size=$(wc -c < "$CACHE_FILE" | tr -d ' ')
  echo "Cached ${size} bytes to ${CACHE_FILE}"
else
  echo "Fetch failed. Keeping existing cache (if any)." >&2
  rm -f "${CACHE_FILE}.tmp"
  exit 1
fi
