#!/usr/bin/env python3
"""Generate narration segments with ElevenLabs (same spec format as narrate.py).

Spec: {"voice_id": "...", "model_id": "eleven_multilingual_v2",
       "out_dir": "...", "segments": [{"id": "01", "text": "..."}]}
Requires ELEVENLABS_API_KEY in .env or environment.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import urllib.request
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def load_env() -> None:
    env_path = ROOT / ".env"
    if not env_path.exists():
        return
    for raw in env_path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        os.environ.setdefault(key.strip(), value.strip().strip("\"'"))


def duration_of(path: Path) -> float:
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(path)],
        capture_output=True, text=True, check=True,
    ).stdout.strip()
    return float(out)


def tts(api_key: str, voice_id: str, model_id: str, text: str, out: Path,
        stability: float, similarity: float, style: float) -> None:
    body = json.dumps({
        "text": text,
        "model_id": model_id,
        "voice_settings": {"stability": stability, "similarity_boost": similarity, "style": style},
    }).encode()
    req = urllib.request.Request(
        f"https://api.elevenlabs.io/v1/text-to-speech/{voice_id}",
        data=body,
        headers={"xi-api-key": api_key, "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=120) as resp:
        out.write_bytes(resp.read())


def main() -> int:
    load_env()
    api_key = os.environ.get("ELEVENLABS_API_KEY")
    if not api_key:
        raise SystemExit("ELEVENLABS_API_KEY not set")
    ap = argparse.ArgumentParser()
    ap.add_argument("spec", type=Path)
    args = ap.parse_args()
    spec = json.loads(args.spec.read_text(encoding="utf-8"))
    out_dir = Path(spec.get("out_dir", "outputs/audio"))
    out_dir = out_dir if out_dir.is_absolute() else ROOT / out_dir
    out_dir.mkdir(parents=True, exist_ok=True)

    total = 0.0
    for seg in spec["segments"]:
        seg_id = str(seg["id"])
        mp3 = out_dir / f"seg{seg_id}.mp3"
        tts(api_key,
            seg.get("voice_id", spec.get("voice_id")),
            seg.get("model_id", spec.get("model_id", "eleven_multilingual_v2")),
            seg["text"], mp3,
            float(spec.get("stability", 0.55)),
            float(spec.get("similarity_boost", 0.8)),
            float(spec.get("style", 0.35)))
        dur = duration_of(mp3)
        total += dur
        print(f"seg{seg_id}: {dur:.1f}s  {seg['text'][:55]}...")
    print(f"\ntotal narration: {total:.1f}s -> {out_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
