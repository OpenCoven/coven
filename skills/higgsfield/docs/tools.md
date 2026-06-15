# Tools

## Requirements

- Python 3
- Node/npm or `npx`
- `ffmpeg` and `ffprobe`
- Higgsfield subscription
- Optional: ElevenLabs account for premium narration

## Higgsfield

The wrapper keeps Higgsfield CLI state inside this folder:

```bash
./scripts/higgsfield_local.sh auth login
./scripts/higgsfield_local.sh account status
./scripts/higgsfield_local.sh model list --video
```

The local state folder is `.higgsfield-local/` and is ignored by git.
Do not share it.

## ElevenLabs

ElevenLabs is optional. Use your own account:

1. Create or choose a voice in ElevenLabs.
2. Copy that voice's ID into the ElevenLabs pipeline spec.
3. Put your API key in `.env` as `ELEVENLABS_API_KEY`.

Example:

```bash
cp .env.example .env
# edit .env with your own ELEVENLABS_API_KEY
python3 scripts/episode_pipeline.py specs/language-grimoire-kant.elevenlabs.pipeline.json
```

The ElevenLabs script writes MP3 narration segments. It does not create
word-timed VTT captions, so the ElevenLabs example disables burned captions.

## Free Narration

The Edge TTS path can generate MP3 plus VTT captions:

```bash
python3 scripts/episode_pipeline.py specs/language-grimoire-kant.edge.pipeline.json
```

If `edge-tts` is installed somewhere nonstandard, set `EDGE_TTS_BIN` in
`.env` or in your shell.

## Image + Video Generation

Use cost checks before generation:

```bash
python3 scripts/keyframe_batch.py cost specs/ambient-fantasy-keyframes.example.json --limit 1
python3 scripts/keyframe_batch.py generate specs/ambient-fantasy-keyframes.example.json --only b01
python3 scripts/higgsfield_flex_batch.py cost specs/ambient-fantasy-clips.example.json --only b01
python3 scripts/higgsfield_flex_batch.py generate specs/ambient-fantasy-clips.example.json --only b01
```

For full episodes, prefer the one-command pipeline:

```bash
python3 scripts/episode_pipeline.py specs/marcus-aurelius.pipeline.json --dry-run
python3 scripts/episode_pipeline.py specs/marcus-aurelius.pipeline.json
```

## Post Tools

- `scripts/assemble_episode.py` combines clips and narration into one vertical video.
- `scripts/loop_extend.py` turns short ambient clips into longer boomerang loops.

Generated files go under `outputs/`, which is ignored.
