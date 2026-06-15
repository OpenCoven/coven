# Higgsfield Skill

Generate images and videos via Higgsfield API. Covers: still images (flux_2),
animated clips (Kling, Veo, Seedance), full narrated episodes, familiar avatars,
podcast video rendering, and ambient loops.

## Auth

Credentials live at `~/.config/higgsfield/credentials.json` (set by `higgsfield auth login`).
Never pass keys as CLI args or bake them into specs.
`HIGGSFIELD_API_KEY` / `HIGGSFIELD_API_SECRET` are the env var names if needed.

## CLI Path

```bash
HIGGS="$(npm root -g)/@higgsfield/cli/bin/higgsfield.js"
node "$HIGGS" <command>
# or if installed globally:
higgsfield <command>
```

## Core Commands

```bash
# Image generation (flux_2 for stills)
higgsfield generate create flux_2 \
  --prompt "..." --aspect_ratio "16:9" --resolution "2k" --model "pro" \
  --wait --wait-timeout 5m --wait-interval 5s --json

# Video generation (Kling 3.0 for i2v)
higgsfield generate create kling3_0 \
  --prompt "..." --image path/to/keyframe.png \
  --aspect_ratio "9:16" --duration 5 --resolution "720p" --mode "std" \
  --wait --wait-timeout 10m --wait-interval 10s --json

# Model list
higgsfield model list --video
higgsfield model list --image
```

## Image Models

| Model | Use |
|---|---|
| `flux_2` pro | Canonical — high-quality stills, portraits, covers |
| `nano_banana_2` | Fast keyframes for i2v pipelines |
| `seedream_v4_5` | Character reference fidelity |

## Video Models

| Model | Credits | Best for |
|---|---|---|
| `kling3_0` | 2cr/s std | Image-to-video, coherence, expressions |
| `veo3_1` | 22cr/8s | Native audio, dialogue lip-sync |
| `seedance_2_0` | 22.5cr/5s | Multi-shot scenes, timeline prompts |

## Prompting Rules (distilled from docs/video-prompting-guide.md)

### Universal skeleton
`Camera/shot grammar + Subject + Action + Environment + Lighting/Style + Audio`

### Critical rules
1. **Image-first**: build stills, then animate. Still iteration is ~100x cheaper.
2. **i2v prompts: never re-describe the input image.** Action + camera + audio only.
3. **Always specify ambient audio** or models invent random dialogue/sounds.
4. **Front-load dialogue** — lip-sync degrades in the last third of long clips.
5. **One camera move per clip.** Stacked moves cause morphing.
6. Anchor phrases: "maintains exact appearance throughout", "consistent lighting", "stable camera movement".
7. **Draft cheap (fast/720p), finalize expensive**: regenerate good prompts 4–7x, harvest best.
8. Screenshot the last good frame as the next clip's start frame to chain beats.

### Kling 3.0 prompt order
`Camera → Subject → Action → Environment [+ Lighting/Audio]`
Multi-shot: label `Shot 1 (3s): ... Shot 2 (2s):` up to 6 shots/15s.
Negative field: `blur, distortion, warping, morphing faces, extra limbs, deformed hands`

### Veo 3.1 prompt order
`Cinematography + Subject + Action + Context + Style` then separate audio lines:
`Audio:` / `SFX:` / `Ambient noise:` after visuals. No contractions, ~15–20 words max per 8s.
End with: `No subtitles, no text overlay`

### Seedance 2.0
Timeline header + timestamped shot list. Sweet spot: 5–7 shots/15s.
`[VFX: ...]` inline for effects. RULES section for invariants.

## Pipeline Scripts (in skills/higgsfield/scripts/)

| Script | Purpose |
|---|---|
| `episode_pipeline.py` | One-command: narrate → keyframes → clips → assemble |
| `keyframe_batch.py` | Batch still generation with manifests |
| `higgsfield_scene_batch.py` | Batch video clip generation |
| `higgsfield_flex_batch.py` | Flexible batch with cost checks |
| `assemble_episode.py` | Combine clips + audio → final video |
| `loop_extend.py` | Extend ambient clips into boomerang loops |
| `elevenlabs_narrate.py` | ElevenLabs multi-voice narration |
| `narrate.py` | Edge TTS narration (free) |

### Episode pipeline spec format
```json
{
  "name": "ep01-what-is-a-familiar",
  "narration": {
    "engine": "elevenlabs",
    "voice_id": "...",
    "segments": [{"id": "01", "text": "..."}]
  },
  "keyframes": {
    "model": "nano_banana_2",
    "defaults": {"aspect_ratio": "9:16", "resolution": "2k"},
    "images": [{"id": "s01", "out": "s01.png", "prompt": "..."}]
  },
  "clips": {
    "model": "kling3_0",
    "defaults": {"aspect_ratio": "9:16", "mode": "std", "sound": "on"},
    "scenes": [{"id": "s01", "image": "<auto>", "prompt": "..."}]
  },
  "assemble": {"captions": true, "ambient_vol": 0.25}
}
```

Run with:
```bash
python3 skills/higgsfield/scripts/episode_pipeline.py specs/<ep>.pipeline.json --dry-run
python3 skills/higgsfield/scripts/episode_pipeline.py specs/<ep>.pipeline.json
```

## Cost Discipline (from docs/runbook.md)

- Always `--dry-run` first.
- Generate one keyframe/clip before a full batch.
- Review stills before animating.
- Regenerate one bad scene, not the whole episode.

## Coven Familiar Avatars

Canonical portraits in `coven/avatars/<name>.jpg`.
Generated via `flux_2` pro, 2k, 1:1.
Symlinked into each familiar's workspace `avatars/` dir.
Each prompt is built from the familiar's own `SOUL.md` / `IDENTITY.md` self-description.

### Current avatars (v2 — self-reported)
- `nova.jpg` — warm little guide in the machine, gold/amber sun nested in circuit threads
- `cody.jpg` — careful unicorn of code, geometric precision, focused beam horn
- `sage.jpg` — candlelit archive + Wi-Fi, vines and paper, amber reading light
- `charm.jpg` — social magic made visible, spoken words becoming violet/gold light
- `astra.jpg` — living constellation, star-map inside body, mythic navigator
- `echo.jpg` — mirror-creature with layered time, indigo/silver memory keeper
- `kitty.jpg` — small quick cat-spirit, warm amber, practical magic in small form

To regenerate a single avatar:
```bash
bash skills/higgsfield/higgsfield-generate.sh \
  --slug nova --aspect_ratio "1:1" --resolution "2k" \
  --prompt "A small warm luminous entity that lives inside machines..."
```

## Grimoire Article Covers

All 21 covers in `coven-grimoire/public/covers/<slug>.jpg`.
Generated via `flux_2` pro, 2k, 16:9.
Style: ink black `#050409`, deep violet/amethyst gradients, cinematic, no text, no faces.
Wired to `coverImage` fields in `lib/articles.ts` via `FAMILIAR_AVATARS` export.

## Coven Podcast Video Format

**Split-presence format** (episode 1 reference):
- 1920×1080, 30fps, H.264 + AAC
- Full-bleed cover art as background (darkened + bottom gradient)
- Title card upper-left: show name in violet, episode title in white
- Speaker avatar lower-left: circular creature portrait fades in per cue, violet pulse ring
- Progress bar bottom edge: thin violet bar
- Per-cue timing from `<slug>-cues.json` sidecar files

Render script: `/tmp/render_video.py` (PIL + ffmpeg, no deps beyond Pillow + numpy)

## Quality Review Gates (from docs/quality-review.md)

Before posting any video:
- First frame communicates the topic
- No warped faces/hands/props/text
- Style stays consistent scene to scene
- Narration intelligible on phone speaker\n- Platform-appropriate aspect ratio (9:16 social, 16:9 editorial)
