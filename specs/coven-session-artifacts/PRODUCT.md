# Coven Session Artifacts — PRODUCT

**Status:** Draft v1 · 2026-05-26
**Owner:** Coven runtime
**Acceptance target:** "A Coven-backed task has inspectable provenance" + "CastCodes can show useful session artifacts without exposing unsafe details."

## Problem

A "Coven session" today is a soup of events with type discriminators encoded in a free-form `kind` field and arbitrary JSON in `payload_json`. There's no enumerated set of artifacts that callers (CastCodes, future SDKs, the user reviewing their own work) can rely on. The acceptance criteria require something stronger: a fixed, named list of durable artifacts produced by every session, each with a known shape, each with provenance attached.

This spec defines that list.

## The seven artifacts

Every Coven session produces zero or more of each artifact below. The kinds are closed (new artifact types require a spec update; harnesses can't invent new ones). Each artifact is an `events` row of the corresponding `kind`, with a JSON payload conforming to the shape in TECH.md.

| # | Kind | Purpose | Emitted by | Required for "complete" session? |
|---|---|---|---|---|
| 1 | `transcript` | Human-readable user/assistant turns. The thing you'd quote in a postmortem. | Every harness | Yes — at least one |
| 2 | `event` | Lifecycle markers (session start, harness ready, user interrupt, exit). Not user-readable prose. | Daemon + harness | Yes — at minimum `start` and `end` |
| 3 | `command` | Shell command (or tool call resembling one) the agent ran, with args, cwd, exit code, captured stdout/stderr. | Harness (via tool-use trace) | No — sessions can be conversation-only |
| 4 | `changed_file` | A file the session touched, with path, pre/post hash, byte counts, and (encrypted) snapshots. | Daemon, by diffing project root before/after | No |
| 5 | `verification` | Output of an explicit verification step (test run, type check, lint, build, smoke check). Distinct from `command` because it carries a pass/fail verdict the agent or user declared. | Harness or user-triggered | No, but strongly encouraged before `summary` |
| 6 | `handoff` | A structured packet handed off to another harness or human. Schema defined in `coven-handoff-packet`. | Harness when it decides to hand off | No (only present in multi-harness sessions) |
| 7 | `summary` | The agent's final "what I did" — short prose plus a structured task-completion verdict. Closes the session. | Harness (or daemon if harness exited without writing one) | Yes — at most one, at the end |

Notes:

- These are the only first-class artifacts CastCodes will render. Anything else a harness wants to record goes into `event` with a custom subtype, and CastCodes will list it without special treatment.
- `command` is distinct from `verification` because verifications carry an explicit verdict the rest of the system can trust. A `command` is "the agent did X"; a `verification` is "X was tested and the result was P/F."

## Provenance, on every event

Every artifact carries provenance metadata so a reader can answer "who/what/when produced this" without inferring from session-level fields:

- `producer_harness` — `"claude"`, `"codex"`, or future identifier.
- `producer_run_id` — the harness's own session/run id (Claude's conversation UUID, Codex's session id). Multiple events from the same harness turn share this.
- `producer_cwd` — working directory at the moment of emission.
- `created_at` — already present.
- `redaction_version` — already present (added in trust-layer spec).

Provenance is **never redacted** because it's metadata about the run, not content from the run. A "leaked" cwd in provenance is acceptable; a leaked secret in `payload_json` is not.

### Why per-event provenance instead of per-session

A v1 session is single-harness in most cases. But:

- Long-lived sessions can have the agent spawn sub-processes whose output is captured under the same session id — provenance on the event lets a reader tell agent output from subprocess output.
- A future multi-harness session (Codex finishes context-gathering, hands off to Claude for implementation) reuses one session id with two `producer_harness` values across events. Per-event provenance is the only way to render that correctly.

Per-session fields (`sessions.harness`) become "the harness that opened this session" — useful but not authoritative for individual events.

## "Inspectable" — what that means concretely

A session is inspectable when, from a single API call, a reader can build:

1. A chronological event list grouped by `producer_harness` + `producer_run_id`.
2. A list of every command the session ran with its exit code.
3. A list of every file the session changed with path + size delta + content hash.
4. The verification ledger (pass/fail count + per-verification verdict).
5. The handoff history (zero-or-more handoff packets, in order).
6. The final summary, if present.

The `/v1/sessions/:id/manifest` endpoint (TECH.md) returns exactly that view as one JSON document. CastCodes' session-replay page builds from a single fetch.

## What artifacts are NOT

- They are not the source of truth for the underlying objects. The actual files on disk are. Coven's `changed_file` snapshots are a record of what Coven *saw*, not a substitute for `git`.
- They are not signed by any harness. Provenance is "Coven recorded this kind of event from this harness," not "the harness cryptographically attested to this content." If we need attestation later, it becomes a separate spec.
- They are not idempotent across harness restarts. If a Claude session crashes mid-turn and is resumed, the second turn emits new events; the first turn's events are not retroactively edited.

## What CastCodes renders from this

Cross-reference to `castcodes-session-replay` PRODUCT, but in short:

- A header strip showing the session row + provenance summary ("Claude run abc123, then Codex run def456").
- A timeline of artifacts grouped by kind, with the seven kinds above as filter chips.
- For `command`: command, exit code, output (redacted text only over TCP).
- For `changed_file`: path + size delta + hash, with a link that asks the local CLI to decrypt-and-show on the user's machine (decrypt itself is Unix-socket-only).
- For `verification`: green/red verdict + linked output.
- For `handoff`: structured render of the packet (delegated to `castcodes-handoff-render` spec, future).
- For `summary`: prose + task-completion verdict.

## Acceptance for v1

The session-artifact contract is "v1 done" when:

1. The seven kinds above are emitted by both Claude and Codex harness paths for a representative session that includes at least one command, one changed file, one verification, and a summary.
2. `GET /api/v1/sessions/:id/manifest` returns the structured view above and is unit-tested for shape stability.
3. CastCodes can fetch a session's manifest over `/v1/*` and the response contains zero secret-pattern matches (regression-tested).
4. Provenance columns are populated on 100% of new events; `coven doctor` reports any session with NULL provenance and the migration date it predates.
5. The `summary` artifact, when present, is the last event in the session by `created_at`.
