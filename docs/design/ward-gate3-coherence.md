---
title: "Ward Gate 3 — Coherence Review"
description: "Design for resolving held Ward proposals: stage Tier-1 holds beside the existing Tier-0 pending-proposal machinery, attach deterministic coherence probes, and keep the principal's decision as the only path to an applied write."
---

# Ward Gate 3 — Coherence Review Design

Status: Proposed — needs maintainer ratification of the open questions
Date: 2026-07-18
Scope: `crates/coven-cli` (`ward.rs`, `threads_gate.rs`, `api.rs`, `observe.rs`) plus
reference docs. Composes with — never duplicates — the coven-threads §5 staging
machinery that already exists. Tracking: #415 (sub-issue of #335).

## Summary

Gate 3 is the last unimplemented gate from RFC-0001: identity-coherence review
for held proposals. Today a hold is a dead end for Tier-1 targets and a
principal-judgment loop (without probes) for Tier-0 targets. This design
closes the gap by (a) giving Tier-1 holds the same staged-proposal lifecycle
Tier-0 already has, (b) attaching **deterministic** coherence probes to every
staged proposal so the principal decides with evidence, and (c) keeping the
principal's explicit approval as the only transition from *held* to *applied*.
Model-scored probes and any auto-approval are explicitly out of scope for v1.

## Non-goals

The following are stated non-goals for Gate 3 v1 **and for the Ward contract
generally**. Any future proposal to relax one of these is a Ward-spec-level
amendment (RFC-0001 change), not an implementation choice at the Gate 3 layer.

- **Model-scored probes as gate inputs.** Probes at Gate 3 may become
  model-scored in a later version, but their role remains advisory: they
  inform the principal's decision, they never gate the transition from
  *held* to *applied* on their own.
- **Auto-approval on any probe score, at any threshold.** No probe result —
  deterministic or model-scored, single-probe or aggregate — ever transitions
  a held proposal to *applied* without an explicit principal decision. The
  principal is the sole approver of identity and memory changes; this is a
  property of the Ward, not a tunable of Gate 3.
- **Probe evidence as a substitute for the principal's read.** Probes surface
  what a deterministic check can see (parse, size delta, protected-region
  touch, pattern lint). They do not summarize intent, drift risk, or
  identity coherence — those remain the principal's judgment.
- **Ward-side coherence policy.** Gate 3 evaluates whether a change is
  *reviewable* (probes ran, staleness known, principal decides). It does not
  encode what *coherent* means for a given familiar — that lives in the
  familiar's declared surface and the principal's contract with it.

## Current state (verified on main @ b98504c)

What already exists — Gate 3 must compose with all of it:

- **Ward holds, fail-closed** (`ward.rs`): `requires_coherence` covers
  `Verdict::RequiresCoherenceReview` (Tier 1) and
  `Verdict::AuthorizedProtectedChange` (Gate-1-authorized Tier 0).
  `Ward::apply` holds the whole proposal as a unit; nothing is written.
- **Tier-0 staging** (`threads_gate.rs`): on `DegradeToProposal` the gate
  stages the whole proposal at `~/.coven/pending/` as a
  `coven_threads_core::PendingProposal` (typed staged edits, writer, channel,
  thread, fray) and audits `proposal_submitted`.
- **Principal decision loop** (`api.rs::decide_threads_proposal`):
  `POST /api/v1/threads/proposals/:id/approve|reject` re-validates from disk
  (fail-closed 409s for corrupt/missing state), re-runs Gate 1–2 adjudication
  *and* the threads validator per target, audits every verdict, and on
  approval applies via `ward.apply_after_threads_approval`, advances the
  surface baselines, audits `proposal_approved`, and removes the pending file.
  Rejection audits `proposal_rejected` and removes the file.
- **Audit ledger**: `ward_audit` (append-only, UPDATE/DELETE-abort triggers)
  already carries `proposal_submitted` / `proposal_approved` /
  `proposal_rejected` / `validation_verdict` tags.
- **Observability**: `GET /api/v1/familiars/:id/ward` + `coven familiars <id>`
  (#417) expose the declared surface; `GET /api/v1/threads/weaves` exposes weave
  state.

## The gaps

1. **Tier-1 (`Reviewed`) holds have no resolution path.** The threads gate
   only stages Tier-0 targets. A Tier-1 edit reaches `Ward::apply`, is held
   (`202 { disposition: "held" }`), and evaporates — nothing is staged, no
   pending file exists, no approve verb can ever resolve it.
2. **No coherence evidence.** "Coherence review" currently means "the
   principal reads the diff cold". RFC-0001's Gate 3 calls for a probe set
   that checks a proposed identity/memory change against the familiar's
   declared invariants before it is applied.
3. **No pending-proposal read surface.** `~/.coven/pending/` files and the
   approve/reject routes exist, but there is no `GET /api/v1/threads/proposals`
   list, no CLI verb, and no reference doc — a principal cannot discover
   what is waiting without `ls`.

## Design

### G3.1 — Stage Tier-1 holds as pending proposals

When `Ward::apply` would hold a proposal **solely** for
`RequiresCoherenceReview` (no blocked targets, no Tier-0 targets — those
already route through the threads gate first), the `/api/v1/familiars/:id/edits`
handler stages it at `~/.coven/pending/` using the same `PendingProposal`
shape, marked so the decide path knows which re-apply primitive to use:

- Reuse `stage_pending_proposal` with a `review_kind: "coherence"` marker
  (additive field; Tier-0 staging writes `review_kind: "authority"`).
  Absent field ⇒ `authority`, so existing pending files stay readable.
- Audit `proposal_submitted` exactly as the Tier-0 path does.
- Response stays `202`, now with `disposition: "staged"` + `pendingPath` +
  `proposalId` instead of a bare `held` — the caller learns where the
  proposal went and how to resolve it. (`held` remains the shape for mixed
  proposals until G3.4 lands; see Compatibility.)

### G3.2 — Deterministic coherence probes

A probe set runs at **staging time** and its results ride inside the pending
file (`probes: [...]`), the list route, and the CLI so the principal decides
with evidence:

- **v1 probes are deterministic, offline, and advisory**:
  - `parse`: the staged contents parse as the surface's declared format
    (TOML/JSON/Markdown front-matter) when one is declared.
  - `size-delta`: bytes and line delta vs the current surface content.
  - `protected-region`: for Tier-1 files that embed protected blocks
    (e.g. fenced `<!-- ward:protected -->` regions), the staged change does
    not touch those regions.
  - `pattern-lint`: configurable forbidden/required regex list.
- Declared per familiar in `ward.toml` under `[[probe]]` entries
  (surface glob + probe id + parameters). No `[[probe]]` entries ⇒ probes
  report `unscored`, never a pass.
- **Fail-closed semantics**: a probe that errors reports `unscored` with the
  error; probes never gate automatically in v1 — they inform the principal.
  An auto-approve threshold is explicitly rejected for v1 (open question 4
  records the future shape).

### G3.3 — Pending-proposal observability

- `GET /api/v1/threads/proposals` — list pending proposals: id, familiar,
  review kind, staged-at, targets, probe summary. Missing dir ⇒ `[]`.
- `GET /api/v1/threads/proposals/:id` — one proposal with full probe detail.
- CLI: `coven ward pending [--json]` and `coven ward pending <id> [--json]`
  via the observe.rs pattern (`--json` = exact daemon body), plus
  `coven ward approve <id>` / `coven ward reject <id> [--note ...]` wrapping
  the existing POST routes. Reference page `docs/reference/cli-ward.md`
  documents the lifecycle; `api.md` gains the route rows.

### G3.4 — Resolution for coherence proposals

`decide_threads_proposal` grows a branch on `review_kind`:

- `authority` (today's flow): unchanged.
- `coherence`: re-run Gate 1–2 adjudication fail-closed (as today), **skip**
  the threads validator (Tier-1 surfaces are not woven), re-run the probe
  set against current disk state (staleness check — a surface that drifted
  since staging demotes the probe report and is surfaced in the response),
  then apply via a new `ward.apply_after_coherence_approval(&edits, &auth)`
  that clears `RequiresCoherenceReview` only — `AuthorizedProtectedChange`
  and blocked verdicts still refuse, mirroring
  `apply_after_threads_approval`'s narrow bypass. Audit
  `proposal_approved`/`proposal_rejected` with the probe summary in the
  decision detail.

### Compatibility and invariants

- The write path stays single: `POST /api/v1/familiars/:id/edits` remains the only
  arbitrary-file write surface; approval routes only re-drive it through the
  Ward's own primitives. No new write authority is created.
- All-or-nothing holds are preserved: mixed proposals (Tier-0 + Tier-1)
  stage through the *authority* lane as a unit, exactly as today; the
  coherence lane only takes proposals whose sole hold reason is Tier-1
  review.
- Staging-write hardening invariants (randomized `create_new` staging,
  `prepare_staging_parent` no-follow walk) are untouched — approval re-uses
  `write_atomic` unchanged.
- `ward_audit` stays append-only with existing event tags; probe evidence
  rides in existing text columns. (Gate-4 apply-record persistence is
  separate work: #414, blocked on coven-threads#5.)

## Implementation decomposition (one PR each)

1. `PendingProposal.review_kind` marker + Tier-1 staging in
   `/api/v1/familiars/:id/edits` + `proposal_submitted` audit + tests
   (staged/held/mixed shapes).
2. Probe engine (`ward_probes.rs`): the four deterministic probes +
   `[[probe]]` config parsing + results embedded at staging time + tests.
3. Read surface: proposals list/detail routes + `coven ward pending` +
   `cli-ward.md` / `api.md` docs (observe.rs pattern).
4. Coherence resolution: `apply_after_coherence_approval` +
   `decide_threads_proposal` branch + staleness re-probe + tests.
5. CLI decide verbs (`coven ward approve|reject`) + reference docs +
   release notes.

## Open questions (maintainer sign-off needed before PR 1)

1. **Approve-route authentication.** `decide_threads_proposal` authorizes by
   reachability (daemon socket). With `--allow-host` (#399) exposing the TCP
   API through a trusted proxy, should approval require the principal's key
   fingerprint in the body (verified against `ward.toml`) before Gate 3
   lands, or is socket/proxy trust the accepted v1 boundary?
2. **Veto window.** RFC-0001 §5.6 names `proposal_vetoed`. v1 applies
   immediately on approve with no window; is a delayed-apply veto window
   in scope for Gate 3 or deferred with the model-scored probes?
3. **Tier-1 weave representation.** This design deliberately keeps Tier-1
   surfaces out of the weave (no thread ⇒ no threads-validator run on the
   coherence lane). Alternative: weave Tier-1 surfaces with a distinct
   channel so drift-fraying covers them too — heavier, and it moves the
   design into coven-threads-core territory (new upstream work).
4. **Model-scored probe integration path.** The Non-goals section states
   that no probe — deterministic or model-scored — ever auto-approves a
   held proposal; the principal is always the sole approver. The remaining
   open question is *how* model-scored probes, when they arrive, are
   surfaced as advisory evidence: as a separate `advisory_probes` block
   distinct from deterministic `probes` (so principals can see at a glance
   which evidence is machine-judged vs. deterministic), or interleaved with
   a `kind: "advisory"` marker per entry. This is a presentation decision,
   not a policy one — the policy is settled in Non-goals.

## References

- RFC-0001 Familiar Contract (`familiar-contract/rfcs/`), §5.4 (fail-closed
  gates), §5.6 (audit events).
- `specs/coven-familiar-spec/PRODUCT.md` — four-gate design.
- `OpenCoven/coven-threads` `specs/PHASE-0-DESIGN.md` §3.4, §5, §6.
- `crates/coven-cli/src/ward.rs` module docs (threat model, hold semantics).
- `crates/coven-cli/src/threads_gate.rs` module docs (staging, audit).
- #335 (umbrella), #414 (Gate-4 persistence), #416/#417 (ward observability),
  coven-threads#5 (audit event-type extension).
