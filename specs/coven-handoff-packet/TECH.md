# Coven Handoff Packet — TECH

**Status:** Draft v1 · 2026-05-26
**Companion to:** [PRODUCT.md](./PRODUCT.md)

## Wire schema (`coven.handoff.v1`)

```jsonc
{
  "schema": "coven.handoff.v1",
  "trigger": "harness_initiated" | "user_initiated" | "daemon_fallback",
  "from": {
    "harness": "claude",                       // ArtifactKind provenance
    "run_id": "abc-123-...",
    "ended_at": 1748296900
  },
  "to": {
    "harness": "claude" | "codex" | "human" | "<future>",
    "hint": "string|null"                      // optional free-form ("the rust-tests-expert subagent")
  },

  "task_context": {
    "original_goal": "<redacted prose, required, non-empty>",
    "constraints": [
      "<redacted prose>"
    ],
    "scope_notes": "<redacted prose, may be empty>"
  },

  "current_state": {
    "last_action": "<redacted prose, required, non-empty>",
    "loaded_context_summary": "<redacted prose>",
    "open_questions": [ "<redacted prose>" ]
  },

  "files_touched": [
    {
      "path": "crates/coven-cli/src/store.rs",
      "changed_file_artifact_id": "evt_12345",   // ref into session's changed_file events
      "summary": "added events.producer_* columns"
    }
  ],

  "risks": [
    {
      "kind": "incomplete_edit" | "deferred_followup" | "known_bug"
            | "missing_permission" | "unverified_assumption" | "other",
      "detail": "<redacted prose, required>",
      "blocking_for_next_step": true | false
    }
  ],

  "verification": {
    "latest_verdicts": [
      { "verification_artifact_id": "evt_22222", "tool": "cargo test", "verdict": "pass", "at": 1748296800 }
    ],
    "stale": true | false,                     // true if files_touched changed since latest verdict
    "notes": "<redacted prose>"
  },

  "next_action": {
    "instruction": "<redacted prose, required, single-step>",
    "do_not_do": [ "<redacted prose>" ],       // optional explicit anti-list
    "expected_outcome": "<redacted prose>"
  },

  "meta": {
    "session_id": "ses_abc",
    "created_at": 1748296901,
    "redaction_version": 1
  }
}
```

Fields marked "required" are non-empty after trim. Validation on write rejects packets that violate this.

## Rust types

```rust
// crates/coven-cli/src/handoff.rs (new)

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "schema")]
pub enum HandoffPacket {
    #[serde(rename = "coven.handoff.v1")]
    V1(HandoffPacketV1),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HandoffPacketV1 {
    pub trigger: HandoffTrigger,
    pub from: HandoffEndpoint,
    pub to: HandoffEndpoint,
    pub task_context: TaskContext,
    pub current_state: CurrentState,
    pub files_touched: Vec<FileTouched>,
    pub risks: Vec<Risk>,
    pub verification: VerificationBlock,
    pub next_action: NextAction,
    pub meta: HandoffMeta,
}

// ... per-field structs follow the JSON shape above ...

impl HandoffPacketV1 {
    pub fn validate(&self) -> Result<(), HandoffError> {
        if self.task_context.original_goal.trim().is_empty() {
            return Err(HandoffError::MissingField("task_context.original_goal"));
        }
        if self.current_state.last_action.trim().is_empty() {
            return Err(HandoffError::MissingField("current_state.last_action"));
        }
        if self.next_action.instruction.trim().is_empty() {
            return Err(HandoffError::MissingField("next_action.instruction"));
        }
        for risk in &self.risks {
            if risk.detail.trim().is_empty() {
                return Err(HandoffError::MissingField("risks[].detail"));
            }
        }
        Ok(())
    }
}
```

## API surface

### Emit a packet

```
POST /api/v1/sessions/:id/handoffs
Body: HandoffPacketV1 JSON
Response: { "event_id": "evt_NNNN", "packet": <stored, possibly-redacted-on-write> }
4xx: invalid schema / missing required fields / packet exceeds 64 KiB
```

Internally: validate → run redaction filter on prose fields → `store::insert_artifact(kind=Handoff, ...)`. The redaction filter MAY transform the prose; if it does, the response includes the post-redaction packet so the caller knows what was stored.

### Fetch packets for a session

```
GET /api/v1/sessions/:id/handoffs              → all, ordered by created_at
GET /api/v1/sessions/:id/handoffs?latest=true  → most recent only
GET /api/v1/sessions/:id/handoffs/:event_id    → one specific packet
```

All three exposed on TCP `/v1/*` with the same shape (redacted prose; same artifact-ref rules).

### Convenience: emit via CLI

```
coven handoff --session <id> --to <harness|human> --from-prompt
   Reads packet JSON from stdin, validates, posts to the daemon.

coven handoff inspect --session <id>
   Pretty-prints latest packet to terminal.
```

Both are thin wrappers over the HTTP API and run over the Unix socket.

## Harness ingestion

Add to `crates/coven-cli/src/harness.rs`:

```rust
pub trait HarnessAdapter {
    fn render_handoff_into_prompt(
        &self,
        packet: &HandoffPacketV1,
        existing_prompt: Option<&str>,
    ) -> HarnessLaunchArgs;
}
```

Per-harness behavior:

- **Claude:** Renders the packet as a structured system+user message pair. The packet's `next_action.instruction` becomes the first user turn; the other fields become a system message labelled `<inherited_context>`. Files-touched paths are included by ref; the adapter does **not** auto-attach file contents (Claude reads them on demand via its own tools).
- **Codex:** Codex runs single-turn under stream mode; the packet becomes a prelude in the prompt with the six labelled sections. `next_action.instruction` is the directive; everything else is context.
- **Human:** No adapter — CastCodes renders the packet directly (see `castcodes-session-replay`).

Each adapter is a small pure function. No side effects, no I/O. The orchestration layer (or the user) decides when to call it.

## Daemon fallback packet

When a harness exits without writing a `summary` and without writing a `handoff`, the daemon constructs a `daemon_fallback` packet:

- `trigger: "daemon_fallback"`
- `from`: from session record + last event provenance
- `to`: `{ harness: "human", hint: "session ended without summary; review needed" }`
- `task_context.original_goal`: from session title or first user transcript event
- `current_state.last_action`: from last non-event artifact ("ran cargo test" or "wrote file X")
- `risks`: includes `{ kind: "other", detail: "session ended unexpectedly; assume work is incomplete", blocking_for_next_step: true }`
- `verification.latest_verdicts`: scanned from the session's verification events
- `next_action.instruction`: "Review the last few transcript events and decide whether to resume or abandon."

This guarantees the acceptance criterion "every session that took an action has a handoff or summary."

## Validation behavior

`POST /api/v1/sessions/:id/handoffs` returns:

- `200` + body: stored.
- `400` + `{ "error": "missing_field", "field": "<dotted-path>" }`: required field empty.
- `400` + `{ "error": "schema_mismatch", "got_schema": "..." }`: caller sent a non-`coven.handoff.v1` envelope.
- `413` + `{ "error": "too_large", "limit_bytes": 65536 }`: packet body over 64 KiB. Harnesses should summarize rather than dump.

The 64 KiB cap is intentional: a packet that needs more than that is a sign the harness should be writing a `summary` and starting a new session, not a handoff.

## Test plan

- **Schema round-trip:** every required field present → validates → serializes → deserializes → equals original.
- **Validation failures:** each required field individually emptied → validation returns the correct `MissingField` variant.
- **End-to-end:** synthetic Claude→Codex chain. Emit packet from Claude adapter, ingest in Codex adapter, assert Codex prompt contains `next_action.instruction` text and the six labelled sections.
- **Daemon fallback:** start a session, kill the child mid-turn, observe a `daemon_fallback` handoff written by the daemon's child-watcher.
- **Redaction:** packet with `Authorization: Bearer xxx` in `current_state.loaded_context_summary` → stored copy has `[REDACTED]`, returned response shows the redacted form.

## Dependencies

- `coven-session-artifacts` for the `handoff` artifact kind and the event-emission path.
- `coven-trust-layer` for redaction at emit time.
- `coven-gateway-wire` for TCP `/v1/sessions/:id/handoffs` exposure (read-only over TCP; emit is Unix-socket-only in v1).

## Out of scope for v1

- Multi-recipient handoffs (`to` is a single harness).
- Signed packets (no cryptographic attestation of the emitting harness).
- Automatic routing (`to.harness` is set by the emitter; the daemon does not pick).
- Cross-session handoffs (handoff is always within one session id).
