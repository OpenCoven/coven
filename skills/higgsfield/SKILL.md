---
name: higgsfield
description: Generate images and videos via the Higgsfield API. Runtime-portable — uses curl + jq only, no harness-specific wrappers.
---

# Higgsfield Image & Video Generation

Use this skill to generate images or short videos from a text prompt using Higgsfield's generation models. Designed to be called with a fully-constructed prompt — do not ask the user for one; construct it from context (episode title, excerpt, visual brief, etc.) before invoking.

## When To Use It

- Generating podcast episode cover art
- Creating social media visuals (tweet cards, announcement images)
- Producing short video clips for OpenCoven content
- Any image/video generation task where a text prompt is available

Do **not** use this skill to browse Higgsfield's UI, manage accounts, train Soul IDs, or perform tasks that require browser interaction.

## Prerequisites

These must be present before running any command in this skill:

1. **Environment variables:**
   - `HIGGSFIELD_API_KEY` — your Higgsfield API key
   - `HIGGSFIELD_API_SECRET` — your Higgsfield API secret
2. **Binaries:** `curl`, `jq` (standard on all supported runtimes)
3. **Output directory:** writable path for saving generated files (default: `./higgsfield-out/`)

Check readiness:
```bash
[ -n "$HIGGSFIELD_API_KEY" ] && [ -n "$HIGGSFIELD_API_SECRET" ] && which curl && which jq && echo "ready" || echo "missing prerequisites"
```

## Workflow

### 1. Generate an image

Submit a generation request:

```bash
mkdir -p ./higgsfield-out

RESPONSE=$(curl -s -X POST 'https://platform.higgsfield.ai/higgsfield-ai/soul/standard' \
  -H "Authorization: Key $HIGGSFIELD_API_KEY:$HIGGSFIELD_API_SECRET" \
  -H "Content-Type: application/json" \
  -d "{
    \"prompt\": \"$PROMPT\",
    \"aspect_ratio\": \"${ASPECT_RATIO:-16:9}\",
    \"resolution\": \"${RESOLUTION:-720p}\"
  }")

REQUEST_ID=$(echo "$RESPONSE" | jq -r '.request_id')
echo "Submitted: $REQUEST_ID"
```

Supported aspect ratios: `1:1`, `16:9`, `9:16`, `4:3`, `3:4`
Supported resolutions: `480p`, `720p`, `1080p`

### 2. Poll for completion

```bash
for i in $(seq 1 30); do
  STATUS_RESP=$(curl -s "https://platform.higgsfield.ai/requests/$REQUEST_ID/status" \
    -H "Authorization: Key $HIGGSFIELD_API_KEY:$HIGGSFIELD_API_SECRET")
  STATUS=$(echo "$STATUS_RESP" | jq -r '.status')
  echo "[$i/30] status: $STATUS"
  if [ "$STATUS" = "completed" ]; then
    echo "$STATUS_RESP" > /tmp/higgsfield-last.json
    break
  fi
  if [ "$STATUS" = "failed" ] || [ "$STATUS" = "nsfw" ]; then
    echo "Generation failed: $STATUS"
    echo "$STATUS_RESP"
    exit 1
  fi
  sleep 5
done
```

Max wait: 30 × 5s = 2.5 minutes. Increase `seq 1 30` if generating video (may take longer).

### 3. Download to disk

Always download immediately — hosted URLs may expire:

```bash
IMAGE_URL=$(cat /tmp/higgsfield-last.json | jq -r '.images[0].url')
OUTFILE="./higgsfield-out/${SLUG:-image}-$(date +%Y%m%d-%H%M%S).jpg"
curl -s -o "$OUTFILE" "$IMAGE_URL"
echo "Saved: $OUTFILE"
```

For video, replace `.images[0].url` with `.videos[0].url` and adjust the extension.

---

## Episode Cover Prompt Template

When generating podcast episode covers, construct the prompt from the episode's title and excerpt before calling the skill. Use this template:

```
Dark, atmospheric, arcane editorial photograph. {THEME_PHRASE}. Ink black background,
deep violet and amethyst light gradients, minimal geometric sigil or abstract floating
element that evokes {VISUAL_METAPHOR}. No text, no faces. Cinematic lighting, high
contrast, 16:9 aspect ratio, editorial quality. OpenCoven aesthetic: precise, symbolic,
powerful not loud.
```

**Per-episode mappings:**
| Episode | Theme phrase | Visual metaphor |
|---------|-------------|-----------------|
| what-is-a-familiar | "the concept of a named, persistent AI companion" | a spirit presence |
| the-stack-beneath-the-familiar | "layers of technical infrastructure supporting intelligence" | architectural strata |
| should-a-familiar-ever-act-without-asking | "the tension between initiative and permission" | a held door |
| the-cave | "a home for AI familiars, a dark interior with interior glow" | a cave mouth with violet light within |
| the-harness-layer | "scaffolding that enables agents to operate in bounded space" | interlocking geometric frames |

---

## Output

After a successful run, the skill produces:
- A local image file at `./higgsfield-out/<slug>-<timestamp>.jpg`
- `/tmp/higgsfield-last.json` — raw API response for inspection

Pass the local file path to downstream steps (R2 upload, episode manifest, etc.).

---

## Full Single-Command Script

For convenience, a complete generate-and-download in one pass:

```bash
#!/usr/bin/env bash
# Usage: PROMPT="..." SLUG="episode-slug" bash higgsfield-generate.sh
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
```

Save as `coven/skills/higgsfield/higgsfield-generate.sh`.

---

## Notes

- This skill is **runtime-portable**: it uses only `bash`, `curl`, and `jq`. No harness-specific tools, no SDK dependencies, no MCP wrappers.
- Auth is environment-only. Do not bake credentials into commands or scripts.
- Prompt construction happens **outside** this skill. The skill is the transport layer. Creative decisions (what to generate, how to describe it) belong to the calling familiar's workflow.
- For video generation, swap the endpoint to the appropriate video model route and adjust polling time (videos take longer than images).

## Related

- Higgsfield API docs: https://docs.higgsfield.ai
- Higgsfield CLI (optional, heavier): `npm install -g @higgsfield/cli`
- Coven visual system: `coven/skills/opencoven-design/SKILL.md`
