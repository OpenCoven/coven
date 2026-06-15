#!/usr/bin/env python3
"""One-command episode pipeline: narration -> keyframes -> clips -> assembled video.

Designed so a less capable operator (human or model) can produce an episode by
writing ONE spec file and running:

  python3 scripts/episode_pipeline.py specs/<episode>.pipeline.json [--from STAGE]

Stages run in order: narrate, keyframes, clips, assemble. Each stage writes its
outputs under outputs/episodes/<name>/ and the pipeline records progress in
outputs/episodes/<name>/pipeline_state.json, so a crashed/interrupted run can be
resumed with --from <stage> (or it auto-skips completed stages).

Spec format:
{
  "name": "ep02-socrates",
  "narration": { "engine": "edge", "voice": "...", "rate": "-5%",
                 "segments": [{"id": "01", "text": "..."}] },
  "keyframes": { "model": "nano_banana_2", "defaults": {...},
                 "images": [{"id": "s01", "out": "s01.png", "prompt": "...", "refs": [...]}] },
  "clips":     { "model": "kling3_0", "defaults": {...},
                 "scenes": [{"id": "s01", "image": "<auto>", "prompt": "...", "params": {...}}] },
  "assemble":  { "captions": true, "ambient_vol": 0.25 }
}

Set narration.engine to "elevenlabs" and provide voice_id/model_id to use
scripts/elevenlabs_narrate.py instead of Edge TTS.

Conventions enforced:
- narration segments and clip scenes are matched by sorted order; counts must match.
- if a scene's "image" is "<auto>", it is filled with the keyframe whose id matches.
- clip durations: if a scene has no explicit duration param, narration duration
  + 1s rounded up is used (capped at 15 for kling3_0).
"""

from __future__ import annotations

import argparse
import json
import math
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPTS = ROOT / "scripts"
STAGES = ["narrate", "keyframes", "clips", "assemble"]


def sh(cmd: list[str]) -> subprocess.CompletedProcess[str]:
    print(f"\n$ {' '.join(str(c) for c in cmd)}", flush=True)
    return subprocess.run([str(c) for c in cmd], cwd=ROOT, text=True)


def ffprobe_duration(path: Path) -> float:
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(path)],
        capture_output=True, text=True, check=True,
    ).stdout.strip()
    return float(out)


def load_state(state_path: Path) -> dict:
    if state_path.exists():
        return json.loads(state_path.read_text(encoding="utf-8"))
    return {"completed": []}


def save_state(state_path: Path, state: dict) -> None:
    state_path.parent.mkdir(parents=True, exist_ok=True)
    state_path.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("spec", type=Path)
    ap.add_argument("--from", dest="from_stage", choices=STAGES, default=None,
                    help="force re-run from this stage onward")
    ap.add_argument("--dry-run", action="store_true", help="print plan + costs, generate nothing")
    args = ap.parse_args()

    spec = json.loads(args.spec.read_text(encoding="utf-8"))
    name = spec["name"]
    ep_dir = ROOT / "outputs" / "episodes" / name
    audio_dir = ep_dir / "audio"
    kf_dir = ROOT / "outputs" / "keyframes" / name
    work_dir = ep_dir / "work"
    work_dir.mkdir(parents=True, exist_ok=True)
    state_path = ep_dir / "pipeline_state.json"
    state = load_state(state_path)
    if args.from_stage:
        keep = STAGES[: STAGES.index(args.from_stage)]
        state["completed"] = [s for s in state["completed"] if s in keep]

    # --- materialize sub-specs ---
    narr_spec = {**spec["narration"], "out_dir": str(audio_dir.relative_to(ROOT))}
    narr_path = work_dir / "narration.json"
    narr_path.write_text(json.dumps(narr_spec, indent=2), encoding="utf-8")

    kf_spec = {**spec["keyframes"], "name": f"{name}-keyframes",
               "out_dir": str(kf_dir.relative_to(ROOT))}
    kf_path = work_dir / "keyframes.json"
    kf_path.write_text(json.dumps(kf_spec, indent=2), encoding="utf-8")

    def materialize_clips() -> Path:
        clips = json.loads(json.dumps(spec["clips"]))  # deep copy
        kf_by_id = {img["id"]: kf_dir / img["out"] for img in spec["keyframes"]["images"]}
        narr_by_order = sorted(audio_dir.glob("seg*.mp3"))
        scenes = clips["scenes"]
        if narr_by_order and len(narr_by_order) != len(scenes):
            raise SystemExit(f"{len(narr_by_order)} narration segs vs {len(scenes)} scenes")
        for idx, scene in enumerate(scenes):
            if scene.get("image") == "<auto>":
                kf = kf_by_id.get(scene["id"])
                if not kf:
                    raise SystemExit(f"No keyframe with id {scene['id']} for <auto> image")
                scene["image"] = str(kf.relative_to(ROOT))
            params = scene.setdefault("params", {})
            if "duration" not in params and "duration" not in clips.get("defaults", {}):
                if narr_by_order:
                    dur = math.ceil(ffprobe_duration(narr_by_order[idx]) + 1)
                    params["duration"] = min(dur, 15)
        clips["name"] = name
        path = work_dir / "clips.json"
        path.write_text(json.dumps(clips, indent=2), encoding="utf-8")
        return path

    if args.dry_run:
        print(f"PLAN for {name}: stages={[s for s in STAGES if s not in state['completed']]}")
        sh([sys.executable, SCRIPTS / "keyframe_batch.py", "cost", kf_path])
        if (audio_dir / "seg01.mp3").exists():
            sh([sys.executable, SCRIPTS / "higgsfield_flex_batch.py", "cost", materialize_clips()])
        else:
            print("(run narrate stage first for exact clip durations/cost)")
        return 0

    narration_engine = str(spec["narration"].get("engine", "edge")).lower()
    narr_script = "elevenlabs_narrate.py" if narration_engine == "elevenlabs" else "narrate.py"

    # --- narrate ---
    if "narrate" not in state["completed"]:
        if sh([sys.executable, SCRIPTS / narr_script, narr_path]).returncode != 0:
            raise SystemExit("narrate stage failed")
        state["completed"].append("narrate")
        save_state(state_path, state)

    # --- keyframes ---
    if "keyframes" not in state["completed"]:
        if sh([sys.executable, SCRIPTS / "keyframe_batch.py", "generate", kf_path]).returncode != 0:
            raise SystemExit("keyframes stage failed")
        manifest = json.loads((kf_dir / "manifest.json").read_text(encoding="utf-8"))
        errors = [i for i in manifest["images"] if i.get("error")]
        if errors:
            raise SystemExit(f"keyframe failures: {[e['id'] for e in errors]} — rerun: "
                             f"python3 scripts/keyframe_batch.py generate {kf_path} --only <ids>, "
                             f"then resume with --from clips")
        state["completed"].append("keyframes")
        save_state(state_path, state)

    # --- clips ---
    if "clips" not in state["completed"]:
        clips_path = materialize_clips()
        if sh([sys.executable, SCRIPTS / "higgsfield_flex_batch.py", "generate", clips_path]).returncode != 0:
            raise SystemExit("clips stage failed")
        runs = sorted((ROOT / "outputs" / "runs").glob(f"{name}-*"))
        if not runs:
            raise SystemExit("no run directory found after clips stage")
        run_dir = runs[-1]
        run_manifest = json.loads((run_dir / "manifest.json").read_text(encoding="utf-8"))
        errors = [s for s in run_manifest["scenes"] if s.get("error")]
        if errors:
            raise SystemExit(f"clip failures: {[e['id'] for e in errors]} — rerun with --only, "
                             f"then resume with --from assemble")
        state["completed"].append("clips")
        state["run_dir"] = str(run_dir.relative_to(ROOT))
        save_state(state_path, state)

    # --- assemble ---
    if "assemble" not in state["completed"]:
        run_dir = ROOT / state["run_dir"]
        asm = spec.get("assemble", {})
        cmd = [sys.executable, SCRIPTS / "assemble_episode.py",
               "--clips-dir", run_dir / "clips",
               "--audio-dir", audio_dir,
               "--out", ep_dir / f"{name}.mp4",
               "--ambient-vol", str(asm.get("ambient_vol", 0.25))]
        if asm.get("captions", True):
            cmd.append("--captions")
        if sh(cmd).returncode != 0:
            raise SystemExit("assemble stage failed")
        state["completed"].append("assemble")
        save_state(state_path, state)

    print(f"\nDONE: {ep_dir / (name + '.mp4')}")
    print("Review against docs/quality-review.md before posting.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
