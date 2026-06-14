#!/usr/bin/env python3
"""Generate narration segments + word-timing subtitles from an episode script.

Spec format:
{
  "voice": "en-US-AndrewMultilingualNeural",
  "rate": "-5%",
  "out_dir": "outputs/episodes/ep02/audio",
  "segments": [
    {"id": "01", "text": "..."},
    {"id": "02", "text": "...", "voice": "en-US-AvaNeural"}
  ]
}

Writes segNN.mp3 + segNN.vtt per segment and prints durations.
Requires edge-tts (default lookup: ~/.venvs/yt/bin/edge-tts, override with
EDGE_TTS_BIN).
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EDGE_TTS = os.environ.get("EDGE_TTS_BIN", str(Path.home() / ".venvs/yt/bin/edge-tts"))


def duration_of(path: Path) -> float:
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(path)],
        capture_output=True, text=True, check=True,
    ).stdout.strip()
    return float(out)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("spec", type=Path)
    args = parser.parse_args()
    spec = json.loads(args.spec.read_text(encoding="utf-8"))
    out_dir = Path(spec.get("out_dir", "outputs/audio"))
    out_dir = out_dir if out_dir.is_absolute() else ROOT / out_dir
    out_dir.mkdir(parents=True, exist_ok=True)

    total = 0.0
    for seg in spec["segments"]:
        seg_id = str(seg["id"])
        mp3 = out_dir / f"seg{seg_id}.mp3"
        vtt = out_dir / f"seg{seg_id}.vtt"
        cmd = [
            EDGE_TTS,
            "--voice", seg.get("voice", spec.get("voice", "en-US-AndrewMultilingualNeural")),
            f"--rate={seg.get('rate', spec.get('rate', '-5%'))}",
            "--text", seg["text"],
            "--write-media", str(mp3),
            "--write-subtitles", str(vtt),
        ]
        subprocess.run(cmd, check=True)
        dur = duration_of(mp3)
        total += dur
        print(f"seg{seg_id}: {dur:.1f}s  {seg['text'][:60]}...")
    print(f"\ntotal narration: {total:.1f}s -> {out_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
