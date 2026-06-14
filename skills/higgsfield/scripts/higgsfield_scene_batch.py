#!/usr/bin/env python3
"""Generate generic Higgsfield scene batches from scenes[]."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import subprocess
import sys
import time
import urllib.request
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
HF = ROOT / "scripts" / "higgsfield_local.sh"
OUTPUTS = ROOT / "outputs" / "runs"


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
    if not isinstance(data.get("scenes"), list):
        raise SystemExit(f"Spec missing scenes[]: {path}")
    return data


def scene_id(scene: dict) -> str:
    return str(scene.get("id") or scene.get("slug") or "scene")


def selected(scenes: list[dict], only: set[str] | None, limit: int | None) -> list[dict]:
    out = [scene for scene in scenes if not only or scene_id(scene) in only]
    if limit is not None:
        out = out[:limit]
    return out


def command_for(spec: dict, scene: dict) -> list[str]:
    model = scene.get("model") or spec.get("model") or os.environ.get("HIGGSFIELD_VIDEO_MODEL", "").strip()
    if not model:
        raise SystemExit("Set HIGGSFIELD_VIDEO_MODEL or add model to the spec.")
    cmd = [
        str(HF),
        "generate",
        "create",
        model,
        "--prompt",
        scene["video_prompt"],
        "--aspect_ratio",
        scene.get("aspect_ratio") or spec.get("aspect_ratio") or os.environ.get("HIGGSFIELD_ASPECT_RATIO", "9:16"),
        "--duration",
        str(scene.get("duration_seconds", 5)),
        "--resolution",
        scene.get("resolution") or spec.get("resolution") or os.environ.get("HIGGSFIELD_RESOLUTION", "720p"),
        "--mode",
        scene.get("mode") or spec.get("mode") or os.environ.get("HIGGSFIELD_MODE", "fast"),
        "--json",
    ]
    image = scene.get("image", "").strip()
    if image:
        image_path = (ROOT / image).resolve() if not Path(image).is_absolute() else Path(image)
        if not image_path.exists():
            raise SystemExit(f"Missing image for scene {scene_id(scene)}: {image_path}")
        cmd.extend(["--image", str(image_path)])
    return cmd


def job_id_from(response: object) -> str:
    if isinstance(response, list) and response:
        return str(response[0])
    if isinstance(response, dict):
        for key in ("id", "job_id", "request_id"):
            if response.get(key):
                return str(response[key])
    raise RuntimeError(f"No job id in create response: {response}")


def wait_for_job(job_id: str, timeout_s: int, interval_s: int) -> dict:
    deadline = time.time() + timeout_s
    last_status = ""
    while time.time() < deadline:
        proc = run([str(HF), "generate", "get", job_id, "--json"])
        if proc.returncode != 0:
            print(proc.stderr.strip() or proc.stdout.strip(), flush=True)
            time.sleep(interval_s)
            continue
        result = json.loads(proc.stdout)
        status = str(result.get("status", "")).lower()
        if status != last_status:
            print(f"  {job_id}: {status or 'unknown'}", flush=True)
            last_status = status
        if status in {"completed", "succeeded", "success", "done"}:
            return result
        if status in {"failed", "error", "cancelled", "canceled", "nsfw", "rejected", "blocked"}:
            raise RuntimeError(f"Job {job_id} failed: {json.dumps(result, indent=2)}")
        time.sleep(interval_s)
    raise TimeoutError(f"Timed out waiting for {job_id}")


def download(url: str, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(url, timeout=180) as resp:
        path.write_bytes(resp.read())


def dry_run(spec_path: Path, only: set[str] | None, limit: int | None) -> None:
    spec = load_spec(spec_path)
    for scene in selected(spec["scenes"], only, limit):
        print(f"\n{scene_id(scene)} {scene.get('slug', '')}")
        print(" ".join(command_for(spec, scene)))
        if scene.get("acceptance"):
            print("acceptance:")
            for check in scene["acceptance"]:
                print(f"  - {check}")


def generate(spec_path: Path, only: set[str] | None, limit: int | None, no_wait: bool) -> None:
    spec = load_spec(spec_path)
    stamp = dt.datetime.now(dt.UTC).strftime("%Y%m%d-%H%M%S")
    run_dir = OUTPUTS / f"scenes-{stamp}"
    clips_dir = run_dir / "clips"
    run_dir.mkdir(parents=True, exist_ok=True)
    manifest = {"created_utc": stamp, "spec": str(spec_path), "scenes": []}
    timeout_s = int(os.environ.get("HIGGSFIELD_WAIT_TIMEOUT_SECONDS", "1800"))
    interval_s = int(os.environ.get("HIGGSFIELD_WAIT_INTERVAL_SECONDS", "10"))

    for scene in selected(spec["scenes"], only, limit):
        key = scene_id(scene)
        print(f"\nGenerating {key} {scene.get('slug', '')}", flush=True)
        proc = run(command_for(spec, scene))
        create_path = run_dir / f"{key}_create.json"
        create_path.write_text(proc.stdout + proc.stderr, encoding="utf-8")
        if proc.returncode != 0:
            raise RuntimeError(proc.stderr.strip() or proc.stdout.strip())
        response = json.loads(proc.stdout)
        job_id = job_id_from(response)
        print(f"  job_id: {job_id}", flush=True)

        result_path = None
        clip_path = None
        if not no_wait:
            result = wait_for_job(job_id, timeout_s, interval_s)
            result_path = run_dir / f"{key}_result.json"
            result_path.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
            result_url = result.get("result_url")
            if result_url:
                clip_path = clips_dir / f"{key}_{scene.get('slug', 'scene')}.mp4"
                download(result_url, clip_path)
                print(f"  downloaded: {clip_path}", flush=True)

        manifest["scenes"].append(
            {
                "id": key,
                "slug": scene.get("slug"),
                "job_id": job_id,
                "create_output": str(create_path.relative_to(ROOT)),
                "result_output": str(result_path.relative_to(ROOT)) if result_path else None,
                "clip": str(clip_path.relative_to(ROOT)) if clip_path else None,
                "acceptance": scene.get("acceptance", []),
            }
        )

    (run_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"\nmanifest: {run_dir / 'manifest.json'}", flush=True)


def main() -> int:
    load_env()
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    for name in ("dry-run", "generate"):
        p = sub.add_parser(name)
        p.add_argument("spec", type=Path)
        p.add_argument("--only", default="", help="Comma-separated scene ids")
        p.add_argument("--limit", type=int)
        if name == "generate":
            p.add_argument("--no-wait", action="store_true")
    args = parser.parse_args()
    only = {part.strip() for part in args.only.split(",") if part.strip()} or None
    if args.cmd == "dry-run":
        dry_run(args.spec, only, args.limit)
    else:
        generate(args.spec, only, args.limit, args.no_wait)
    return 0


if __name__ == "__main__":
    sys.exit(main())
