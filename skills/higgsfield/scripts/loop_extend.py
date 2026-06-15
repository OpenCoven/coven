#!/usr/bin/env python3
"""Extend short ambient clips into long seamless loops (ffmpeg only, free).

Boomerang method: forward + reversed copy concatenated, repeated until the
target duration is reached. Works best on clips with locked cameras and
cyclical motion. Audio is crossfaded per segment to avoid clicks.

Usage:
  python3 scripts/loop_extend.py <clip.mp4> --target 60 --out <out.mp4>
  python3 scripts/loop_extend.py <dir-of-mp4s> --target 60 --out-dir outputs/loops
"""

from __future__ import annotations

import argparse
import math
import subprocess
import sys
import tempfile
from pathlib import Path


def duration_of(path: Path) -> float:
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(path)],
        capture_output=True, text=True, check=True,
    ).stdout.strip()
    return float(out)


def boomerang_extend(src: Path, target_s: float, out: Path) -> None:
    tmp = Path(tempfile.mkdtemp(prefix="loop_"))
    fwd = tmp / "fwd.mp4"
    rev = tmp / "rev.mp4"
    # normalize forward copy (constant fps, clean audio)
    subprocess.run(
        ["ffmpeg", "-loglevel", "error", "-y", "-i", str(src),
         "-vf", "fps=30,format=yuv420p", "-af", "afade=t=in:d=0.05,afade=t=out:st=9999:d=0",
         "-c:v", "libx264", "-crf", "19", "-c:a", "aac", "-b:a", "160k", str(fwd)],
        check=True,
    )
    # reversed copy (video and audio reversed; reversed audio of ambient beds
    # still sounds ambient)
    subprocess.run(
        ["ffmpeg", "-loglevel", "error", "-y", "-i", str(fwd),
         "-vf", "reverse", "-af", "areverse",
         "-c:v", "libx264", "-crf", "19", "-c:a", "aac", "-b:a", "160k", str(rev)],
        check=True,
    )
    pair_d = duration_of(fwd) + duration_of(rev)
    reps = max(1, math.ceil(target_s / pair_d))
    concat_list = tmp / "list.txt"
    concat_list.write_text("".join(f"file '{fwd}'\nfile '{rev}'\n" for _ in range(reps)), encoding="utf-8")
    out.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["ffmpeg", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0",
         "-i", str(concat_list), "-t", str(target_s), "-c", "copy", str(out)],
        check=True,
    )
    print(f"{src.name} -> {out} ({duration_of(out):.1f}s)")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("src", type=Path, help="clip or directory of clips")
    ap.add_argument("--target", type=float, default=60.0)
    ap.add_argument("--out", type=Path, help="output file (single-clip mode)")
    ap.add_argument("--out-dir", type=Path, help="output dir (directory mode)")
    args = ap.parse_args()

    if args.src.is_dir():
        out_dir = args.out_dir or Path("outputs/loops")
        for clip in sorted(args.src.glob("*.mp4")):
            boomerang_extend(clip, args.target, out_dir / f"{clip.stem}_loop{int(args.target)}s.mp4")
    else:
        out = args.out or args.src.with_name(f"{args.src.stem}_loop{int(args.target)}s.mp4")
        boomerang_extend(args.src, args.target, out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
