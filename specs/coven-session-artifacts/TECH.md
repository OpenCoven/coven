# Coven Session Artifacts — TECH

**Status:** Draft v1 · 2026-05-26
**Companion to:** [PRODUCT.md](./PRODUCT.md)

## Event-kind enum (closed)

Replace the free-form `events.kind: TEXT` discriminator with a closed enum at the Rust layer. SQLite still stores TEXT for forward compatibility; the Rust side rejects unknown kinds on insert.

```rust
// crates/coven-cli/src/artifacts.rs (new)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Transcript,
    Event,
    Command,
    ChangedFile,
    Verification,
    Handoff,
    Summary,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str { /* matches the PRODUCT table */ }
    pub fn from_str(s: &str) -> Option<Self> { /* … */ }
    pub fn redaction_class(self) -> RedactionClass {
        match self {
            ArtifactKind::Transcript | ArtifactKind::Verification
            | ArtifactKind::Handoff   | ArtifactKind::Summary
            | ArtifactKind::Command   | ArtifactKind::Event => RedactionClass::TextRedact,
            ArtifactKind::ChangedFile => RedactionClass::EncryptedArtifactRef,
        }
    }
}
```

`RedactionClass` is the bridge into the trust-layer spec — `insert_event()` calls `kind.redaction_class()` to decide which write path to take.

## JSON shape per artifact

All payloads share two top-level keys:

```jsonc
{
  "schema": "coven.artifact.v1",   // bumps on shape change
  "kind": "transcript",            // ArtifactKind as snake_case
  // ...kind-specific fields below
}
```

### `transcript`

```jsonc
{
  "schema": "coven.artifact.v1",
  "kind": "transcript",
  "role": "user" | "assistant" | "system",
  "text": "<redacted prose>",
  "tokens": { "input": 1234, "output": 567 }   // optional, when harness reports
}
```

### `event` (lifecycle markers)

```jsonc
{
  "schema": "coven.artifact.v1",
  "kind": "event",
  "subtype": "session_start" | "session_end" | "harness_ready"
           | "user_interrupt" | "harness_crashed" | "custom:<id>",
  "detail": { /* free-form, redacted */ }
}
```

`subtype` is enumerated for known markers; `custom:<id>` namespace is allowed for harness extensions. CastCodes lists `custom:*` events without special treatment.

### `command`

```jsonc
{
  "schema": "coven.artifact.v1",
  "kind": "command",
  "command": "cargo test --workspace",
  "argv": ["cargo", "test", "--workspace"],   // post-parse
  "cwd": "/abs/path",                         // already in provenance; duplicated for replay convenience
  "started_at": 1748296800,
  "finished_at": 1748296812,
  "exit_code": 0,
  "stdout": "<redacted>",
  "stderr": "<redacted>",
  "stdout_artifact_ref": null,                // if truncated; ref into sensitive_artifacts
  "stderr_artifact_ref": null
}
```

When `stdout`/`stderr` exceeds 64 KiB (configurable), it routes to `sensitive_artifacts` and the inline field becomes `null` with a `*_artifact_ref`. CastCodes shows the inline text directly; the artifact ref is decrypt-on-demand and Unix-socket-only.

### `changed_file`

```jsonc
{
  "schema": "coven.artifact.v1",
  "kind": "changed_file",
  "path": "crates/coven-cli/src/store.rs",   // project-root-relative
  "action": "created" | "modified" | "deleted" | "renamed",
  "rename_from": null | "old/path",
  "byte_count_before": 12345,
  "byte_count_after": 12890,
  "sha256_before": "abc...",                 // hex; null if action=created
  "sha256_after":  "def...",                 // hex; null if action=deleted
  "pre_artifact_ref":  "<artifact_id>",       // encrypted snapshot
  "post_artifact_ref": "<artifact_id>"        // encrypted snapshot
}
```

Path is project-relative (not absolute) so the artifact is portable. CastCodes never receives the snapshot bodies over TCP — only this JSON. The pre/post hashes let CastCodes render a "size delta" + "did this file change" without decrypting.

### `verification`

```jsonc
{
  "schema": "coven.artifact.v1",
  "kind": "verification",
  "tool": "cargo test" | "tsc" | "eslint" | "bun test" | "<custom>",
  "command": "cargo test --workspace",       // the actual invocation
  "verdict": "pass" | "fail" | "skip" | "error",
  "started_at": 1748296800,
  "finished_at": 1748296830,
  "summary": "112 passed, 3 failed",         // redacted
  "output": "<redacted, truncated>",         // first 8 KiB
  "output_artifact_ref": null,
  "exit_code": 1
}
```

A `verification` always has a verdict. If the harness ran something but didn't declare a verdict, it's a `command`, not a `verification`.

### `handoff`

Schema lives in `coven-handoff-packet`; this event embeds it as the payload (without the outer `schema`/`kind` wrapper duplication — the inner packet has its own `schema: "coven.handoff.v1"`).

```jsonc
{
  "schema": "coven.artifact.v1",
  "kind": "handoff",
  "packet": { /* coven.handoff.v1 packet, see coven-handoff-packet TECH */ }
}
```

### `summary`

```jsonc
{
  "schema": "coven.artifact.v1",
  "kind": "summary",
  "text": "<redacted prose>",
  "verdict": "completed" | "partial" | "blocked" | "abandoned",
  "blockers": ["<redacted>"],                // optional, only when verdict=blocked
  "follow_ups": ["<redacted>"],              // optional
  "tokens_total": { "input": 12000, "output": 3400 }
}
```

`summary` MUST be the chronologically last event in a session. The daemon enforces this on write — if an event arrives after a `summary`, the `summary` row is updated to point at the new tail, and the late event is appended normally. (Rare; usually means the harness emitted a stray log line after declaring completion.)

## Manifest endpoint

`GET /api/v1/sessions/:id/manifest` (Unix socket — TCP-exposed at `/v1/sessions/:id/manifest`, same response shape):

```jsonc
{
  "session": { /* sessions row, redacted-safe fields */ },
  "provenance": [
    { "producer_harness": "claude", "producer_run_id": "abc-123",
      "first_event_at": 1748296800, "last_event_at": 1748296850, "event_count": 42 },
    { "producer_harness": "codex",  "producer_run_id": "def-456",
      "first_event_at": 1748296851, "last_event_at": 1748296900, "event_count": 19 }
  ],
  "artifacts": {
    "transcript":   { "count": 28, "events": [/* { id, created_at, role, preview } */] },
    "event":        { "count": 6,  "events": [/* { id, created_at, subtype } */] },
    "command":      { "count": 12, "events": [/* { id, created_at, command, exit_code } */] },
    "changed_file": { "count": 7,  "events": [/* { id, created_at, path, action, size_delta } */] },
    "verification": { "count": 3,  "events": [/* { id, created_at, tool, verdict } */] },
    "handoff":      { "count": 1,  "events": [/* { id, created_at, packet_summary } */] },
    "summary":      { "count": 1,  "events": [/* { id, created_at, verdict } */] }
  },
  "timeline_cursor": "evt_99999",   // opaque; pass to /events?after_seq= for full timeline
  "schema": "coven.manifest.v1"
}
```

Manifest is a *summary view* — it's small enough for CastCodes to render the session header instantly. Full event bodies come from the existing `/events?after_seq=` endpoint.

## Writing artifacts

Single insertion path:

```rust
// crates/coven-cli/src/store.rs
pub fn insert_artifact(
    &self,
    session_id: &str,
    kind: ArtifactKind,
    provenance: Provenance,        // { harness, run_id, cwd }
    payload: serde_json::Value,
) -> Result<EventId> {
    let policy = privacy::REDACTION_POLICY[kind];
    let (final_payload, ref_to_artifact) = match kind.redaction_class() {
        RedactionClass::TextRedact => (privacy::redact_value(&payload)?, None),
        RedactionClass::EncryptedArtifactRef => {
            // ChangedFile payloads carry pre/post bodies in a special field
            // that's stripped here and routed to sensitive_artifacts.
            let (json, refs) = encrypted_artifacts::route_changed_file(payload)?;
            (json, Some(refs))
        }
    };
    /* insert with provenance, redaction_version, expires_at … */
}
```

Harness adapters (`harness.rs`) translate harness-native event streams into `insert_artifact` calls. There is no other write path.

## Backfill plan

Existing events have free-form `kind` values like `"input"`, `"output"`, `"tool_call"`. A one-time backfill migration maps them:

| Old `kind` | New `kind` |
|---|---|
| `"input"` | `transcript` (role=user) |
| `"output"` | `transcript` (role=assistant) |
| `"tool_call"` | `command` (best-effort; verdict left null) |
| `"exit"` | `event` (subtype=session_end) |
| `"error"` | `event` (subtype=harness_crashed) |

Backfill ONLY rewrites the discriminator and adds `schema: "coven.artifact.v0-backfill"` to flag that the payload wasn't authored under v1 rules. Old `payload_json` content is left as-is — re-redacting old rows is out of scope (they were redacted under the rules of their era; `redaction_version` reflects that).

## Test plan

- **Round-trip tests** per kind: build a synthetic payload, insert, manifest-fetch, assert shape matches schema and provenance is populated.
- **Closed-enum tests:** insert with unknown kind → error at the Rust boundary; SQLite-level injection of an unknown kind → manifest returns it under a synthetic `unknown` bucket without breaking.
- **Manifest stability:** golden-file test of a fixture session's manifest output.
- **CastCodes contract test:** generated TypeScript types from the JSON schemas; CI fails if Rust serialization drifts.

## Dependencies

- `coven-trust-layer` for redaction enforcement at insert and pruning of expired rows.
- `coven-handoff-packet` for the inner packet shape used by `handoff` artifacts.
- `coven-gateway-wire` for the `/v1/sessions/:id/manifest` TCP exposure rules.

## Out of scope for v1

- Cryptographic harness attestation of artifact contents.
- Cross-session artifact deduplication (each session owns its own snapshot rows).
- Streaming manifest updates (CastCodes polls or re-fetches on `/events` cursor advance).
