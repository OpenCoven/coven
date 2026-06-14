# Video Model Prompting Guide

Distilled June 2026 from official guides (Google Cloud/DeepMind Veo, fal.ai
Kling, Higgsfield Seedance/Cinema Studio) and 20 recent YouTube creator
tutorials (transcripts + on-screen prompt dissection). Full sources in the
research transcripts; this is the working reference.

## Universal Skeleton

`Camera/shot grammar + Subject + Action + Environment + Lighting/Style +
Audio (+ named-speaker dialogue, + negative prompt, + RULES for invariants)`

Cross-model doctrine:

1. **Image-first.** Build characters (multi-angle sheets), locations, props
   as stills (Nano Banana Pro / GPT Image 2), then animate. Image iteration
   is ~100x cheaper than video iteration.
2. **In image-to-video prompts, never re-describe the input image.**
   Describe only action + camera + audio. Redundant detail = reduced motion
   and drift.
3. **Always specify ambient audio** (or Kling invents random dialogue and
   Veo hallucinates studio audiences).
4. **Front-load dialogue** — lip-sync degrades in the last third of long
   clips. End with action.
5. **Draft cheap, finalize expensive**: 480p/Fast tiers for iteration;
   regenerate a good prompt 4-7x and harvest the best shots; screenshot the
   last good frame as the next start frame to fix individual beats.
6. **One camera move per clip.** Stacked moves cause morphing.
7. Anchor phrases that help i2v: "maintains exact appearance throughout",
   "consistent lighting", "stable camera movement".

## Kling 3.0 (our workhorse, 2 cr/s std)

- Order: Camera → Subject → Action → Environment (+ Lighting/Audio).
- Multi-shot: label "Shot 1 (3s): ... Shot 2 (2s): ..." in one prompt, up
  to 6 shots / 15s. Dialogue: `Character (tone): "line"`. All dialogue in
  the first 10 seconds.
- Elements: up to 7 reference images; 3 angles per character (triptych
  prompt) stops hallucinated sides. Multi-shot unavailable when both start
  AND end frames are set.
- Negative field: `blur, distortion, warping, morphing faces, extra limbs,
  deformed hands, unnatural physics, flickering`.
- Strengths: prompt coherence (beat Veo 3.1 in creator tests), expressions,
  multilingual per-character voices, POV/FPV, product label consistency.
- Storyboard trick: 3x3 grid image → "generate the frames in sequence as
  per the storyboard", 15s.

## Veo 3.1 (22 cr/8s — native audio)

- Front-load style keywords ("2D anime style. ..."). Prompt anatomy:
  cinematography + subject + action + context + style + audio block.
- Audio lines, each its own labeled sentence AFTER visuals: `Audio:`,
  `SFX:`, `Ambient noise:`, music = mood + instrument.
- Dialogue: `says: line` (colon, no quotes) + end with "No subtitles, no
  text overlay" (repeat it); no contractions; ~15-20 words max per 8s.
- POV trick: `[character] is holding a selfie stick (that's where the
  camera is)`; "slightly grainy, looks very film-like" de-AIs the look.
- Timestamp prompting: `[00:00-00:02] ... [00:02-00:04] ...` for multi-beat.
- Negatives: descriptive exclusion ("street empty of any modern objects")
  beats bare negation.
- Same prompt + new seed ≈ identical output — vary wording for variety.
- Higgsfield tiers: basic/high both 22 cr (8s); ultra 48; fast tier is the
  drafting tier elsewhere (no dialogue on Fast via Google's own apps).

## Seedance 2.0 (22.5 cr/5s/720p — multi-shot king)

- Timeline prompting: header block (FORMAT / SUBJECT with @image_N refs /
  WARDROBE / ENVIRONMENT with lettered sub-locations / MOOD / MUSIC / COLOR
  LOGIC / STYLE) + timestamped shot list (`SHOT 1 — 0:00 to 0:01, MCU,
  50mm, locked. ...`).
- Sweet spot 5-7 shots per 15s; prompt timestamps must match duration
  setting; ≤3 characters per generation; ~3,000 char limit on Higgsfield.
- `[VFX: ...]` brackets inline; RULES section for invariants; POV lock:
  "one continuous take, no camera cuts" or it invents angles.
- Realism: append `no 3D, no cartoon, no VFX`. Audio: explicit `NO MUSIC /
  NO SFX` or bracketed sound events.
- Extension: upload previous clip + "extend" prompt → seamless
  continuation keeping characters and voices.

## Historical recreation (antiquity)

- Lighting is the highest-leverage lever: torchlight, oil-lamp glow, golden
  hour, chiaroscuro, "lit only by torchlight and oil lamps — no electric
  light".
- Name materials/garments, not eras: wool chiton, himation, bronze cuirass,
  Corinthian helmet, Doric columns, amphorae, wax tablets.
- Formula: named dramatic act + precise location + explicit lighting +
  camera direction + style anchor ("painterly oil with chiaroscuro").
- Garbled prop text is unsolvable in-model: keep scrolls out of focus,
  describe texture not content, or render text in the still (Kling 3.0
  preserves input-image text best).
- Transition pairs beat single clips: same scene re-rendered zoomed/
  top-down via image model → first+last frame + named camera motion.

## ASMR / ambient / loops

- Anatomy: style+mood → narrative summary → DYNAMIC (action) → STATIC
  (setting lock) → AUDIO block (most important — physical, material-specific
  sound language) + quality suffix.
- Glass-fruit pattern: material transmutation + exact cut count + macro +
  shallow DOF + explicit foley + "no background distractions, same iconic
  camera angle as professional food ASMR videos".
- Loops: locked camera, symmetric centered composition, re-entry motion,
  "loopable motion, seamless transition"; same image as first AND last
  frame = perfect loop; or duplicate+reverse in the edit. 3-10s, 9:16.
- Fantasy-magical: glowing/translucent subject + particle system + slow
  ambient motion + volumetric light + slow-motion cue.

## Meta

- Every serious 2026 workflow is LLM-meta-prompted: idea + reference images
  → structured prompt → human edit → generate. (That's Claude's job here.)
- Model routing: Seedance = multi-shot scenes/action/VFX; Kling = i2v
  fidelity + coherence + cheap; Veo = ambience/music/dialogue ecosystem;
  Grok 1.5 = fast 2-person dialogue lip-sync, weak action.
