#!/usr/bin/env python3
"""Assemble an episode: per-scene clips + narration segments -> one vertical video.

Usage:
  python3 scripts/assemble_episode.py --clips-dir outputs/runs/<run>/clips \
      --audio-dir outputs/episodes/<ep>/audio --out outputs/episodes/<ep>/<ep>.mp4 \
      [--ambient-vol 0.25] [--captions]

Pairing: clips sorted by name are matched to segNN.mp3 sorted by name.
Each scene is trimmed to max(clip, narration+0.4s tail) ... actually to
narration+0.4s, capped at clip duration. Narration is mixed over the clip's
own ambient audio (ducked). Optional karaoke-style captions from segNN.vtt.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import tempfile
from pathlib import Path


def ffprobe_duration(path: Path) -> float:
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(path)],
        capture_output=True, text=True, check=True,
    ).stdout.strip()
    return float(out)


def parse_vtt(path: Path) -> list[tuple[float, float, str]]:
    """Return [(start_s, end_s, text)] cues."""
    cues = []
    if not path.exists():
        return cues
    ts = re.compile(r"(\d+):(\d+):(\d+)[.,](\d+)\s*-->\s*(\d+):(\d+):(\d+)[.,](\d+)")
    lines = path.read_text(encoding="utf-8").splitlines()
    i = 0
    while i < len(lines):
        m = ts.search(lines[i])
        if m:
            g = [int(x) for x in m.groups()]
            start = g[0] * 3600 + g[1] * 60 + g[2] + g[3] / 1000
            end = g[4] * 3600 + g[5] * 60 + g[6] + g[7] / 1000
            text_lines = []
            i += 1
            while i < len(lines) and lines[i].strip():
                text_lines.append(lines[i].strip())
                i += 1
            cues.append((start, end, " ".join(text_lines)))
        i += 1
    return cues


def group_cues(cues: list[tuple[float, float, str]], max_words: int = 3) -> list[tuple[float, float, str]]:
    """Split sentence-level cues into <=max_words chunks with proportional timing."""
    chunks = []
    for start, end, text in cues:
        words = text.split()
        if not words:
            continue
        n_chunks = max(1, (len(words) + max_words - 1) // max_words)
        per = (end - start) / n_chunks
        for i in range(n_chunks):
            seg_words = words[i * max_words:(i + 1) * max_words]
            chunks.append((start + i * per, start + (i + 1) * per, " ".join(seg_words)))
    return chunks


def esc_drawtext(text: str) -> str:
    return text.replace("\\", "\\\\").replace(":", "\\:").replace("'", "’").replace("%", "\\%")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--clips-dir", type=Path, required=True)
    ap.add_argument("--audio-dir", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--ambient-vol", type=float, default=0.25)
    ap.add_argument("--captions", action="store_true")
    ap.add_argument("--tail", type=float, default=0.45, help="seconds of breathing room after narration")
    ap.add_argument("--stretch-video", action="store_true",
                    help="slow the video to fit narration instead of trimming narration")
    args = ap.parse_args()

    clips = sorted(args.clips_dir.glob("*.mp4"))
    narrs = sorted(args.audio_dir.glob("seg*.mp3"))
    if len(clips) != len(narrs):
        raise SystemExit(f"{len(clips)} clips vs {len(narrs)} narration segments — must match.")

    tmp = Path(tempfile.mkdtemp(prefix="episode_"))
    scene_files = []
    for idx, (clip, narr) in enumerate(zip(clips, narrs), start=1):
        clip_d = ffprobe_duration(clip)
        narr_d = ffprobe_duration(narr)
        wanted_d = narr_d + args.tail
        stretch = 1.0
        if args.stretch_video and clip_d < wanted_d:
            stretch = wanted_d / clip_d  # slow video by this factor
            scene_d = wanted_d
        else:
            scene_d = min(clip_d, wanted_d)

        vf = "scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920,fps=30,format=yuv420p"
        if stretch > 1.0:
            vf = f"setpts={stretch:.4f}*PTS," + vf
        if args.captions:
            vtt = narr.with_suffix(".vtt")
            for start, end, text in group_cues(parse_vtt(vtt)):
                if start >= scene_d:
                    continue
                end = min(end, scene_d)
                vf += (
                    f",drawtext=text='{esc_drawtext(text.upper())}'"
                    f":fontfile=/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
                    f":fontsize=52:fontcolor=white:borderw=4:bordercolor=black"
                    f":x=(w-text_w)/2:y=h*0.72"
                    f":enable='between(t,{start:.3f},{end:.3f})'"
                )

        scene_out = tmp / f"scene{idx:02d}.mp4"
        cmd = [
            "ffmpeg", "-loglevel", "error", "-y",
            "-i", str(clip), "-i", str(narr),
            "-filter_complex",
            f"[0:v]{vf}[v];"
            f"[0:a]volume={args.ambient_vol}{f',atempo={1/stretch:.4f}' if stretch > 1.0 else ''}[amb];"
            f"[1:a]adelay=150|150[narr];"
            f"[amb][narr]amix=inputs=2:duration=first:normalize=0[a]",
            "-map", "[v]", "-map", "[a]",
            "-t", f"{scene_d:.3f}",
            "-c:v", "libx264", "-preset", "medium", "-crf", "19",
            "-c:a", "aac", "-b:a", "192k",
            str(scene_out),
        ]
        subprocess.run(cmd, check=True)
        scene_files.append(scene_out)
        print(f"scene {idx}: clip={clip.name} narr={narr.name} dur={scene_d:.1f}s")

    concat_list = tmp / "list.txt"
    concat_list.write_text("".join(f"file '{f}'\n" for f in scene_files), encoding="utf-8")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["ffmpeg", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0",
         "-i", str(concat_list), "-c", "copy", str(args.out)],
        check=True,
    )
    total = ffprobe_duration(args.out)
    print(f"\nassembled: {args.out} ({total:.1f}s)")
    manifest = {
        "out": str(args.out),
        "duration_s": round(total, 2),
        "scenes": [{"clip": c.name, "narration": n.name} for c, n in zip(clips, narrs)],
        "captions": args.captions,
        "ambient_vol": args.ambient_vol,
    }
    args.out.with_suffix(".assembly.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
