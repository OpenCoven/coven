# Coven Handoff Packet — PRODUCT

**Status:** Draft v1 · 2026-05-26
**Owner:** Coven runtime
**Acceptance target:** "Codex/Claude/future harness handoffs have a standard packet."

## Problem

Today, conversation state is opaque to Coven. Each harness owns its own session semantics: Claude keeps a conversation UUID and resumes via `--resume`, Codex prints a session id and resumes via `exec resume`. There is no neutral, structured object that can travel between harnesses — so multi-harness work today means "the user copies and pastes context manually," which neither scales nor leaves a record.

This spec defines the **handoff packet**: a single structured object that captures everything the next harness (or human) needs to continue the task safely.

## When a handoff happens

A handoff is emitted whenever:

1. A harness decides it's reached the boundary of its strengths and wants another harness to take the next step ("I've gathered enough context; hand off to Claude for the implementation").
2. A user invokes `coven handoff <from> -> <to>` explicitly (Unix-socket-only command).
3. A harness exits before declaring `summary`, and the daemon constructs a "partial-progress" handoff so the next run can pick up.

In all three cases the resulting artifact is identical in shape (see TECH.md). The trigger is recorded in the packet, not encoded in different schemas.

## What a packet contains (six fields)

The packet has exactly six fields the next harness MUST read, plus metadata. Each field has a clear question it answers:

| Field | Answers the question |
|---|---|
| **Task context** | "What is being attempted, and why?" Original user goal + accumulated constraints discovered since. |
| **Current state** | "Where did the previous harness stop?" Last action taken, last verification verdict, what's loaded into the agent's head. |
| **Files touched** | "What changed on disk?" List of `changed_file` artifact refs; the next harness can fetch full snapshots if needed. |
| **Risks** | "What's known to be risky or unfinished?" Half-completed edits, deferred follow-ups, known bugs, things the previous harness lacked permission for. |
| **Verification** | "What's the last known good state?" Latest `verification` artifact refs + their verdicts. Tells the next harness what they can rely on without re-running. |
| **Next action** | "What should the next harness do first?" Concrete, single-step instruction; not a plan. |

These six are mandatory. A packet missing any one is rejected by the daemon (a `handoff` event with an invalid packet fails the write, the harness gets an error, and falls back to emitting a `summary` with verdict `partial`).

## What a packet does NOT contain

- **No tokens, credentials, or session keys.** Even if the previous harness used `~/.coven/token` or held an env-var API key, those are not echoed into the packet. The next harness re-reads its own credentials.
- **No raw file contents.** Files are referenced by `changed_file` artifact id (see `coven-session-artifacts`). The next harness fetches snapshots through the normal artifact path (and gets the same trust-layer treatment).
- **No private prompting or jailbreak instructions.** Packets are inspectable by humans and by CastCodes. The "next action" field is plain instruction prose.
- **No harness-specific config** (no Claude-only flags, no Codex-only flags). The receiving harness adapter translates the packet into its own invocation arguments.

## Why this packet, not "just resume"

Each harness already has a "resume" semantics (Claude `--resume <uuid>`, Codex `exec resume`). Those are intra-harness; they restore one harness's view of one conversation. They don't transfer to another harness, and they don't make the state inspectable by anything other than the harness itself.

The handoff packet is **harness-agnostic, human-readable, machine-parseable, and inspectable in CastCodes**. It's what the runtime needs the moment a second harness gets involved.

## Flow

```
Claude session running
        │
        │ (decides to hand off)
        │
        ▼
Harness emits a `handoff` artifact (event row with packet JSON in payload)
        │
        ▼
Daemon validates packet shape, stores it under the current session
        │
        │ (user or orchestration starts the next harness for the same session)
        │
        ▼
Codex harness adapter fetches latest handoff packet via /api/v1/sessions/:id/handoffs?latest=true
        │
        │ (adapter translates packet into Codex prompt + flags)
        │
        ▼
Codex session begins, with provenance.producer_harness="codex" and
provenance.producer_run_id=<new codex session id>
```

The daemon does not auto-route. Step "user or orchestration starts the next harness" is explicit, by the user or by a future orchestration layer. v1 of orchestration is "the human reads the handoff and picks the next harness" — anything more autonomous is a follow-up spec.

## Multi-step chains

A single session can carry multiple handoffs (`Claude → Codex → Claude → Human`). Each handoff is its own event, in order. The `/handoffs` endpoint returns them as an ordered list. The session's `summary` (if present) closes the chain; if it's absent, the chain is open and the session is "in progress" regardless of which harness is active right now.

A handoff to a human is the same packet — `next_harness: "human"`. CastCodes renders it as a flagged inbox item.

## Inspectable, by design

Because the packet is structured prose with six labelled fields:

- A reviewer can read it in 30 seconds and tell whether the next step is safe.
- CastCodes can render it as a card with six labelled sections (no free interpretation).
- A diff between two consecutive packets shows what was learned during one harness's turn.
- Search across packets ("which sessions had a 'partial migration' risk") is straightforward.

## Acceptance for v1

The handoff packet is "v1 done" when:

1. The Rust type exists, has serde round-trips, and is validated by the daemon on write.
2. Both Claude and Codex harness adapters can **emit** a packet at session boundary (either by harness instruction or by daemon-constructed "partial-progress" fallback on crash).
3. Both Claude and Codex harness adapters can **ingest** a packet at session start and render it into harness-specific prompt + flags.
4. A two-step session (Claude → Codex) carrying one handoff has a manifest that shows two `provenance` entries and one `handoff` artifact, and the second harness's first transcript event references the packet's `next_action`.
5. CastCodes can render a handoff packet as the structured card described in `castcodes-session-replay`.
