---
summary: "Companion-safe, generation-fenced session handoff protocol."
read_when:
  - Building a companion session-takeover client
  - Adding a harness handoff adapter
title: "Session handoff"
description: "The coven.handoff.v1 daemon contract for a durable, redacted, one-writer companion session takeover."
---

The `sessionHandoff` health capability advertises Coven's
`coven.handoff.v1` handoff contract. It transfers a **redacted context
record**, not credentials, a live terminal, or authority to execute its text.
Receiving clients must render the record as untrusted context and verify it
against their active request and repository state.

## Protocol

1. The source writes `POST /api/v1/sessions/:id/handoffs` with a
   `coven.handoff.v1` packet. Coven validates the packet, redacts secret-like
   values, stores it durably, and records a source event cursor plus a
   portable workspace snapshot.
2. A destination claims the generation with
   `POST /api/v1/sessions/:id/handoffs/:handoffId/claim`. It supplies
   `expectedGeneration`, a stable claimant id, an idempotency key, and its
   workspace snapshot. The compare-and-swap rejects stale generations,
   changed source transcript, workspace divergence, another claimant, and
   in-flight source input.
3. On a successful claim, source `POST /input` is fenced. The source caller
   must quiesce its harness and acknowledge with `POST .../:handoffId/ack`;
   Coven verifies the cursor has not moved.
4. The destination imports with `POST .../:handoffId/continuations`.
   Coven records destination, source session, and generation durably and
   returns the packet inside a fixed untrusted-context prelude.

Claims retried by the same claimant and idempotency key return the original
claim. Continuation imports are idempotent per handoff and destination.

## Packet and workspace rules

The packet schema is documented in
[`specs/coven-handoff-packet/TECH.md`](https://github.com/OpenCoven/coven/tree/main/specs/coven-handoff-packet).
Required prose fields are non-empty and the serialized packet is limited to
64 KiB. Stored packets use Coven's privacy redactor.

Workspace snapshots contain a SHA-256 identifier of the Git origin (never an
absolute path or raw remote), commit, branch, dirty state, and changed paths.
Handoff is rejected when either side cannot provide a portable Git snapshot or
does not exactly match the snapshot captured at offer time.

## Transport and trust boundary

These routes are local Unix-socket API routes. A mobile companion must reach
them only through a separately paired, mutually authenticated transport. Do
not expose this API directly to a LAN, browser, relay, or Tailscale listener:
the local socket's same-user trust boundary does not authenticate a remote
caller. The handoff protocol provides durable ownership fencing; it does not
replace remote transport authentication or repository authorization.

## Failure recovery

All state is in SQLite, so offered/claimed/acknowledged/continued state
survives daemon restart. Read `GET /api/v1/sessions/:id/handoffs?latest=true`
and retry only with the same claimant and idempotency key. If the transcript
or workspace changed, emit a fresh generation rather than overriding it.
