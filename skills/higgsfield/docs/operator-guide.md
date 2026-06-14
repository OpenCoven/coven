# Operator Guide

Start here when running the kit.

## Decision Tree

| Goal | Use |
|---|---|
| Recreate the language / grimoire / Kant style | `specs/language-grimoire-kant.edge.pipeline.json` |
| Use premium ElevenLabs narration | `specs/language-grimoire-kant.elevenlabs.pipeline.json` |
| Make a shorter philosopher story | `specs/marcus-aurelius.pipeline.json` |
| Make magical ambient loops | `specs/ambient-fantasy-keyframes.example.json` then `specs/ambient-fantasy-clips.example.json` |
| Learn prompt rules | `docs/video-prompting-guide.md` |
| Review before posting | `docs/quality-review.md` |

## Standard Episode Flow

```bash
python3 scripts/episode_pipeline.py specs/<episode>.pipeline.json --dry-run
python3 scripts/episode_pipeline.py specs/<episode>.pipeline.json
```

Stages:

1. Narration
2. Keyframes
3. Video clips
4. Assembly

The pipeline is resumable. If a stage fails, fix the failed prompt/spec and
resume with:

```bash
python3 scripts/episode_pipeline.py specs/<episode>.pipeline.json --from <stage>
```

Valid stages: `narrate`, `keyframes`, `clips`, `assemble`.

## Narration Options

Free path:

```json
"narration": {
  "engine": "edge",
  "voice": "en-US-AndrewMultilingualNeural",
  "rate": "-5%"
}
```

ElevenLabs path:

```json
"narration": {
  "engine": "elevenlabs",
  "voice_id": "PUT_YOUR_ELEVENLABS_VOICE_ID_HERE",
  "model_id": "eleven_multilingual_v2"
}
```

Use your own ElevenLabs API key in `.env`; never paste the key into a spec.

## Cost Discipline

- Run `--dry-run` first.
- Generate one keyframe/clip before a full batch.
- Review stills before animating.
- Regenerate one bad scene instead of the whole episode.

## What Good Looks Like

- The first frame explains the idea immediately.
- Written marks are soft and unreadable unless added in post.
- The style remains consistent scene to scene.
- Audio is clear on a phone speaker.
- The final video feels like a short story, not a slideshow of prompts.
