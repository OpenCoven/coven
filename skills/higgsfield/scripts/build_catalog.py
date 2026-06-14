#!/usr/bin/env python3
"""Build the content catalog: per-platform caption sidecars + an HTML index.

Reads specs/catalog-metadata.json. For each video writes, next to the MP4:
  <stem>.caption-tiktok.txt   (title line + hashtags, casual)
  <stem>.caption-youtube.txt  (TITLE: / DESCRIPTION: blocks, hashtags in desc)
  <stem>.caption-instagram.txt(caption + <=5 hashtags)
And renders outputs/catalog/index.html grouped by brand with thumbnails,
durations, and copy-ready captions.

Usage: python3 scripts/build_catalog.py
"""

from __future__ import annotations

import html
import json
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "outputs" / "catalog"
THUMBS = OUT / "thumbs"


def duration_of(path: Path) -> float:
    try:
        out = subprocess.run(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(path)],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        return float(out)
    except Exception:
        return 0.0


def thumb(path: Path, name: str) -> str | None:
    THUMBS.mkdir(parents=True, exist_ok=True)
    dst = THUMBS / f"{name}.jpg"
    proc = subprocess.run(
        ["ffmpeg", "-loglevel", "error", "-y", "-ss", "2", "-i", str(path),
         "-frames:v", "1", "-vf", "scale=180:-1", str(dst)],
        capture_output=True,
    )
    return dst.name if proc.returncode == 0 else None


def tags_for(video: dict, brand: dict, limit: int) -> list[str]:
    tags = list(dict.fromkeys(video.get("tags", []) + brand.get("tags_base", [])))
    return tags[:limit]


def write_sidecars(mp4: Path, video: dict, brand: dict) -> None:
    stem = mp4.with_suffix("")
    title, desc = video["title"], video["desc"]
    tt = " ".join(f"#{t}" for t in tags_for(video, brand, 5))
    Path(f"{stem}.caption-tiktok.txt").write_text(f"{desc} {tt}\n", encoding="utf-8")
    yt_tags = " ".join(f"#{t}" for t in tags_for(video, brand, 3))
    Path(f"{stem}.caption-youtube.txt").write_text(
        f"TITLE:\n{title}\n\nDESCRIPTION:\n{desc}{brand.get('yt_suffix','')}\n\n{yt_tags}\n",
        encoding="utf-8",
    )
    ig = " ".join(f"#{t}" for t in tags_for(video, brand, 5))
    Path(f"{stem}.caption-instagram.txt").write_text(f"{title}\n\n{desc}\n\n{ig}\n", encoding="utf-8")


def main() -> int:
    meta = json.loads((ROOT / "specs" / "catalog-metadata.json").read_text(encoding="utf-8"))
    brands, videos = meta["brands"], meta["videos"]
    OUT.mkdir(parents=True, exist_ok=True)

    rows: dict[str, list[str]] = {b: [] for b in brands}
    missing = []
    for i, v in enumerate(videos):
        # resolve possibly-globbed run dirs (runs get timestamps appended)
        raw = v["file"]
        path = ROOT / raw
        if not path.exists():
            # run dirs are timestamped and clips live under clips/ — match by basename
            name = Path(raw).name
            hits = sorted(p for p in (ROOT / "outputs").rglob(name) if p.is_file())
            if hits:
                path = hits[-1]
            else:
                missing.append(raw)
                continue
        brand = brands[v["brand"]]
        write_sidecars(path, v, brand)
        d = duration_of(path)
        t = thumb(path, f"v{i:02d}")
        img = f'<img src="thumbs/{t}">' if t else ""
        rows[v["brand"]].append(
            f'<tr><td>{img}</td><td><b>{html.escape(v["title"])}</b><br>'
            f'{html.escape(v["desc"])}<br><code>{html.escape(str(path.relative_to(ROOT)))}</code></td>'
            f"<td>{d:.0f}s</td></tr>"
        )

    sections = []
    for bid, brand in brands.items():
        if rows[bid]:
            sections.append(f"<h2>{html.escape(brand['name'])} ({len(rows[bid])})</h2>"
                            f"<table>{''.join(rows[bid])}</table>")
    page = ("<!DOCTYPE html><html><head><meta charset='utf-8'><title>Content Catalog</title>"
            "<style>body{font-family:Georgia,serif;max-width:980px;margin:2rem auto;background:#16161d;"
            "color:#e8e4d8}h1,h2{color:#f0c674}table{border-collapse:collapse;width:100%}"
            "td{border:1px solid #3a3a46;padding:.5rem;vertical-align:top}img{border-radius:6px}"
            "code{background:#23232e;font-size:.8rem;padding:.1rem .3rem}</style></head><body>"
            "<h1>Content Catalog</h1><p>Each video has .caption-tiktok/.caption-youtube/"
            ".caption-instagram sidecar files next to it. Drag video, paste caption, toggle AI label, post.</p>"
            + "".join(sections) + "</body></html>")
    (OUT / "index.html").write_text(page, encoding="utf-8")
    print(f"catalog: {OUT/'index.html'}")
    if missing:
        print("MISSING (not yet rendered or path changed):")
        for m in missing:
            print("  -", m)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
