#!/usr/bin/env bash
# higgsfield-generate.sh — runtime-portable image generation via Higgsfield API
# Usage: PROMPT="..." SLUG="episode-slug" bash higgsfield-generate.sh
# Required env: HIGGSFIELD_API_KEY, HIGGSFIELD_API_SECRET, PROMPT
# Optional env: SLUG, ASPECT_RATIO (default 16:9), RESOLUTION (default 720p), OUTDIR
set -euo pipefail

: "${HIGGSFIELD_API_KEY:?HIGGSFIELD_API_KEY not set}"
: "${HIGGSFIELD_API_SECRET:?HIGGSFIELD_API_SECRET not set}"
: "${PROMPT:?PROMPT not set}"

SLUG="${SLUG:-image}"
ASPECT="${ASPECT_RATIO:-16:9}"
RES="${RESOLUTION:-720p}"
OUTDIR="${OUTDIR:-./higgsfield-out}"
mkdir -p "$OUTDIR"

echo "→ Submitting generation..."
RESPONSE=$(curl -s -X POST 'https://platform.higgsfield.ai/higgsfield-ai/soul/standard' \
  -H "Authorization: Key $HIGGSFIELD_API_KEY:$HIGGSFIELD_API_SECRET" \
  -H "Content-Type: application/json" \
  -d "{\"prompt\": \"$PROMPT\", \"aspect_ratio\": \"$ASPECT\", \"resolution\": \"$RES\"}")

REQUEST_ID=$(echo "$RESPONSE" | jq -r '.request_id')
[ "$REQUEST_ID" = "null" ] && echo "Submission failed: $RESPONSE" && exit 1
echo "→ Request ID: $REQUEST_ID"

echo "→ Polling for completion..."
for i in $(seq 1 30); do
  STATUS_RESP=$(curl -s "https://platform.higgsfield.ai/requests/$REQUEST_ID/status" \
    -H "Authorization: Key $HIGGSFIELD_API_KEY:$HIGGSFIELD_API_SECRET")
  STATUS=$(echo "$STATUS_RESP" | jq -r '.status')
  printf "  [%02d/30] %s\n" "$i" "$STATUS"
  if [ "$STATUS" = "completed" ]; then
    IMAGE_URL=$(echo "$STATUS_RESP" | jq -r '.images[0].url')
    OUTFILE="$OUTDIR/${SLUG}-$(date +%Y%m%d-%H%M%S).jpg"
    curl -s -o "$OUTFILE" "$IMAGE_URL"
    echo "✓ Saved: $OUTFILE"
    exit 0
  fi
  [ "$STATUS" = "failed" ] || [ "$STATUS" = "nsfw" ] && echo "✗ Failed: $STATUS" && exit 1
  sleep 5
done

echo "✗ Timed out after 2.5 minutes"
exit 1
