---
name: "memory-recall"
scope: "coven"
version: "0.2.0"
description: "Runtime-neutral recall from the Coven shared memory store (~/.coven/memory). Wraps the coven-memory CLI to retrieve promoted, attested facts at query time. Read-only by construction."
kind: "agent"
---

# Memory Recall

Retrieval-side contract for the Coven memory layer (Authoritative Plan v1 hole #7;
bead `cmem-pbr`). Any familiar, under any runtime, uses this skill to pull
already-promoted facts back into working context. It does **not** write, promote,
or mutate anything under `~/.coven/memory` — that is the promotion layer's job
(`coven memory promote`, schema: `familiar-contract/schemas/coven-memory-schema.md` §11).

## When to use

- The question depends on a promoted Coven fact, decision, or cross-familiar context
  rather than the current session or the familiar's own MEMORY.md.
- "Did we decide X?" / "what did <familiar> attest about Y?" for anything that
  plausibly predates the current session or originated with another familiar.
- As a supplement to the runtime's own memory search (which covers per-familiar
  MEMORY.md / daily notes / transcripts). Recall covers the layer *below* that:
  promoted, attested, cross-familiar-visible.

## When NOT to use

- Answer is in the current session → just answer.
- Answer is in the familiar's own MEMORY.md or daily files → use the runtime's
  memory tools.
- The promotion layer has not run for the relevant window → recall returns empty;
  say so honestly. Never fabricate a hit.

## Inputs

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `query` | string | ✅ | Natural-language question or keyword phrase. |
| `familiar` | string | ❌ | Store scope filter passed to the crate: `coven` (default; the shared store) or a familiar id for per-familiar stores. |
| `k` | int | ❌ | Max results. Default 5, max 20. |
| `since` / `until` | ISO date | ❌ | Filter on manifest `timestamp`. |
| `verified_only` | bool | ❌ | Default `true`. Drop hits whose manifest `sha256` no longer matches the promoted snapshot or whose `attested_via` file is missing. |

## Outputs

| Field | Type | Description |
|-------|------|-------------|
| `hits` | array | Ordered `{ulid, title, type, memory_type, snippet, score, attested_by, attestation_type, promoted_at, superseded_by?}` |
| `count` | int | Length of `hits`. |
| `queried_at` | RFC3339 UTC | When the recall ran. |
| `manifest_hash` | string | SHA-256 of `manifest.jsonl` consulted (drift detection between calls). |
| `dropped` | int | Hits removed by verification (`verified_only=true`). Surfaced, never silent. |

## Side effects

**None.** Recall is read-only by construction; an implementation that mutates
`~/.coven/memory` during recall is non-conformant with this skill. Recall MUST NOT
log query text to any destination outside the Coven filesystem.

## Steps

1. **Precondition.** `~/.coven/memory/manifest.jsonl` exists with ≥1 line, else return
   `{hits: [], count: 0, note: "promotion layer has not run"}` and stop.
2. **Query the crate.** `coven-memory search --familiar <familiar> "<query>"` (add
   `--index`/`--db` only if the store lives off the default path). The crate owns
   ranking (fastembed/nomic-embed-text-v1.5 + TurboVec ANN; schema §11.5 pins
   embeddings as crate-internal).
3. **Join against the manifest.** For each hit, find its manifest line by `ulid`.
   Apply `since`/`until` on `timestamp`. Detect supersession: if a later
   `operation: supersede` line names this ulid as `prior_ulid`, annotate
   `superseded_by` and rank the superseding entry ahead of the superseded one.
   Never silently hide a superseded claim — the store is a claims log (§11.0).
4. **Verification (`verified_only=true`, default).** Drop hits where (a) the
   manifest `sha256` mismatches the promoted snapshot content, or (b) the entry's
   `attested_via` attestation file is absent. Count them in `dropped`.
   **Fail-degraded, not fail-fatal:** verification failure removes the hit and is
   reported; it never aborts the query. (No cryptographic verification in v1 —
   attestations are unsigned per schema §11.2.)
5. **Resolve `source_context` lazily.** Entries carry FAMILIAR_ROOT-relative paths or
   portable `session://` references (§11.1). Resolve against the current
   environment's FAMILIAR_ROOT (`--familiar-root` flag → `COVEN_FAMILIAR_ROOT` env)
   only when the caller asks to open a source; unresolvable → return the portable
   reference as-is with a note. Recall never requires FAMILIAR_ROOT to answer.
6. **Answer with provenance.** Present claims with `attested_by` + `attestation_type`
   + `confidence` + timestamp. Conflicting claims are both returned; the querying
   familiar reasons over them (claims-log framing, §11.0).

## Failure modes

| Condition | Behavior |
|-----------|----------|
| Store or manifest missing | Empty result + honest note. Never fabricate. |
| `coven-memory` binary absent | Fail with install pointer (`OpenCoven/coven` crates). No grep fallback that bypasses scope filtering. |
| Manifest line missing for a hit | Drop hit, count in `dropped` (index-without-source is a defect, §11.0). |
| Unknown `scope`/fields in a manifest line | Skip that line, count in `dropped` (fail-degraded). |

## Runtime-Portability Audit (§11.5 — answered)

*If Val runs a familiar tomorrow under a different runtime (Codex CLI, Cursor,
Claude Code, CovenCave, bare SSH), does this skill work without modification?*

**✅ Yes.** The skill's only dependencies are (a) the `coven-memory` CLI — a
Coven-shipped binary, (b) `~/.coven/memory/` — a Coven-owned contract path, and
(c) a shell. No runtime-specific hooks, session APIs, or bootstrap injection.
FAMILIAR_ROOT resolution follows the schema's flag→env→fail-closed order and is
only needed for optional source opening, never for querying. Session references
use the portable `session://` class form, never raw runtime session keys.
