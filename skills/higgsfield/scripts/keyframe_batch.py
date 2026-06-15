#!/usr/bin/env python3
"""Batch keyframe/image generation with manifests — companion to higgsfield_flex_batch.py.

Spec format:
{
  "name": "ep02-keyframes",
  "model": "nano_banana_2",                       # default; scenes may override
  "out_dir": "outputs/keyframes/ep02",
  "defaults": {"aspect_ratio": "9:16", "resolution": "2k"},
  "images": [
    {
      "id": "s01",
      "out": "s01_agora.png",                     # relative to out_dir
      "prompt": "...",
      "refs": ["outputs/keyframes/hero_ref.png"], # optional --image references
      "model": "seedream_v4_5",                   # optional override
      "params": {"resolution": "4k"}              # merged over defaults
    }
  ]
}

Commands: dry-run | cost | generate. Generate downloads results next to a
manifest.json in out_dir and writes 280px review thumbnails to out_dir/thumbs/.
All paths resolve against the repo root regardless of cwd.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import subprocess
import sys
import urllib.request
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
HF = ROOT / "scripts" / "higgsfield_local.sh"


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


def run(cmd: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, cwd=ROOT, text=True, capture_output=True, check=False)


def load_spec(path: Path) -> dict:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data.get("images"), list):
        raise SystemExit(f"Spec missing images[]: {path}")
    return data


def item_id(item: dict) -> str:
    return str(item.get("id") or item.get("out") or "image")


def selected(items: list[dict], only: set[str] | None, limit: int | None) -> list[dict]:
    out = [item for item in items if not only or item_id(item) in only]
    if limit is not None:
        out = out[:limit]
    return out


def resolve(path_str: str) -> Path:
    p = Path(path_str)
    return p if p.is_absolute() else (ROOT / p).resolve()


def command_for(spec: dict, item: dict, verb: str) -> list[str]:
    model = item.get("model") or spec.get("model")
    if not model:
        raise SystemExit("Spec needs a model (top-level or per image).")
    params: dict = {**spec.get("defaults", {}), **item.get("params", {})}
    cmd = [str(HF), "generate", verb, model, "--prompt", item["prompt"]]
    for key, value in params.items():
        cmd.extend([f"--{key}", str(value)])
    for ref in item.get("refs", []):
        ref_path = resolve(ref)
        if not ref_path.exists():
            if verb == "cost":
                # earlier batch items may produce this ref; cost without it
                print(f"  (note: ref not yet generated, costing without it: {ref_path.name})")
                continue
            raise SystemExit(f"Missing reference for {item_id(item)}: {ref_path}")
        cmd.extend(["--image", str(ref_path)])
    if verb == "create":
        cmd.extend(["--wait", "--json"])
    return cmd


def result_url_from(text: str) -> str | None:
    match = re.search(r'"result_url":\s*"([^"]+)"', text)
    return match.group(1) if match else None


def download(url: str, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(url, timeout=180) as resp:
        path.write_bytes(resp.read())


def thumbnail(src: Path, out_dir: Path) -> Path | None:
    thumbs = out_dir / "thumbs"
    thumbs.mkdir(parents=True, exist_ok=True)
    dst = thumbs / src.name
    proc = run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(src), "-vf", "scale=280:-1", str(dst)])
    return dst if proc.returncode == 0 else None


def dry_run(spec: dict, items: list[dict]) -> None:
    for item in items:
        print(f"\n{item_id(item)}")
        print(" ".join(command_for(spec, item, "create")))


def cost(spec: dict, items: list[dict]) -> None:
    total = 0.0
    for item in items:
        proc = run(command_for(spec, item, "cost"))
        out = (proc.stdout + proc.stderr).strip()
        match = re.search(r"([\d.]+)\s*credits", out)
        if proc.returncode != 0 or not match:
            print(f"{item_id(item)}: ERROR {out}")
            continue
        credits = float(match.group(1))
        total += credits
        print(f"{item_id(item)}: {credits:g} credits")
    print(f"\nTOTAL: {total:g} credits")


def generate(spec: dict, items: list[dict], spec_path: Path) -> None:
    out_dir = resolve(spec.get("out_dir", "outputs/keyframes"))
    out_dir.mkdir(parents=True, exist_ok=True)
    stamp = dt.datetime.now(dt.UTC).strftime("%Y%m%d-%H%M%S")
    manifest = {"created_utc": stamp, "spec": str(spec_path), "images": []}
    failures = 0

    for item in items:
        key = item_id(item)
        out_path = out_dir / item.get("out", f"{key}.png")
        print(f"\nGenerating {key} -> {out_path.name}", flush=True)
        try:
            cmd = command_for(spec, item, "create")
        except SystemExit as exc:
            failures += 1
            print(f"  FAILED: {exc}", flush=True)
            manifest["images"].append({"id": key, "error": str(exc)})
            continue
        proc = run(cmd)
        url = result_url_from(proc.stdout + proc.stderr)
        if proc.returncode != 0 or not url:
            failures += 1
            err = (proc.stderr or proc.stdout).strip()[-300:]
            print(f"  FAILED: {err}", flush=True)
            manifest["images"].append({"id": key, "error": err})
            continue
        download(url, out_path)
        thumb = thumbnail(out_path, out_dir)
        print(f"  saved: {out_path}", flush=True)
        manifest["images"].append(
            {
                "id": key,
                "file": str(out_path.relative_to(ROOT)),
                "thumb": str(thumb.relative_to(ROOT)) if thumb else None,
                "prompt": item["prompt"],
                "refs": item.get("refs", []),
            }
        )
        (out_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    (out_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"\nmanifest: {out_dir / 'manifest.json'} ({failures} failures)", flush=True)


def main() -> int:
    load_env()
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    for name in ("dry-run", "cost", "generate"):
        p = sub.add_parser(name)
        p.add_argument("spec", type=Path)
        p.add_argument("--only", default="")
        p.add_argument("--limit", type=int)
    args = parser.parse_args()
    spec = load_spec(args.spec)
    only = {part.strip() for part in args.only.split(",") if part.strip()} or None
    items = selected(spec["images"], only, args.limit)
    if args.cmd == "dry-run":
        dry_run(spec, items)
    elif args.cmd == "cost":
        cost(spec, items)
    else:
        generate(spec, items, args.spec)
    return 0


if __name__ == "__main__":
    sys.exit(main())
