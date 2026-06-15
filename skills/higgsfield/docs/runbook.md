# Production Runbook

## Core Rule

Never spend paid credits without a cost check, and never batch a new idea
before one test clip passes review.

## Narrated Philosophy / Magic Episode

Use this for language, grimoire, Kant, ancient philosophy, myth, or
history-of-ideas shorts.

```bash
python3 scripts/episode_pipeline.py specs/language-grimoire-kant.edge.pipeline.json --dry-run
python3 scripts/episode_pipeline.py specs/language-grimoire-kant.edge.pipeline.json
```

ElevenLabs version:

```bash
python3 scripts/episode_pipeline.py specs/language-grimoire-kant.elevenlabs.pipeline.json
```

### Writing The Spec

1. Script first. Each narration segment should map to one visual scene.
2. Segment 1 should be a hook, not background.
3. Keyframes should describe only what should physically appear.
4. Avoid readable text in generated images. Use soft, unreadable marks.
5. Clip prompts should describe action, one camera move, ambient audio, and invariants.
6. Keep scenes 9:16 and phone-native unless you have a reason not to.

### Prompt Pattern

For keyframes:

```text
Dark Romantic 19th-century oil painting, visible brushwork: [subject],
[setting], [light], [composition]. Soft unreadable marks only. No text.
```

For image-to-video clips:

```text
[Subject/action]. [One camera move]. Audio: [ambient sound], no speech.
Maintains exact appearance and painted style throughout. No subtitles,
no text overlay.
```

## Ambient Magical Loops

Generate a still, animate it, then extend it for free:

```bash
python3 scripts/keyframe_batch.py cost specs/ambient-fantasy-keyframes.example.json --limit 1
python3 scripts/keyframe_batch.py generate specs/ambient-fantasy-keyframes.example.json --only b01
python3 scripts/higgsfield_flex_batch.py cost specs/ambient-fantasy-clips.example.json --only b01
python3 scripts/higgsfield_flex_batch.py generate specs/ambient-fantasy-clips.example.json --only b01
python3 scripts/loop_extend.py outputs/runs/<run>/clips/<clip>.mp4 --target 60
```

Loop prompt rules:

- Locked camera.
- Slow cyclical motion.
- Seamless re-entry.
- Material-specific audio.
- No readable text.

## Repair One Bad Scene

Do not regenerate the whole episode if one scene fails.

```bash
python3 scripts/keyframe_batch.py generate outputs/episodes/<name>/work/keyframes.json --only <scene-id>
python3 scripts/higgsfield_flex_batch.py generate outputs/episodes/<name>/work/clips.json --only <scene-id>
python3 scripts/assemble_episode.py --clips-dir outputs/runs/<run>/clips \
  --audio-dir outputs/episodes/<name>/audio \
  --out outputs/episodes/<name>/<name>.mp4
```

## Review Gate

Before posting or batching, check:

- First frame communicates the topic.
- No warped faces, hands, props, or text.
- The style stays consistent.
- Narration is intelligible on phone speakers.
- Captions, if used, do not cover important visuals.
