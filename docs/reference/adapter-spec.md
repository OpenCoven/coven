---
summary: "The versioned contract every Coven harness adapter must satisfy."
read_when:
  - Authoring a new adapter
  - Updating harness streaming or cancellation behavior
title: "Adapter contract"
description: "Coven harness contract v1: negotiated capabilities, input, streamed output, lifecycle, cancellation, errors, compatibility, and conformance fixtures."
---

# Adapter contract

`coven.harness.v1` is the compatibility boundary between Coven's Rust-owned
runner and a harness adapter. It prevents a harness integration from depending
on hidden runner behavior while preserving Coven's authority over project-root
validation, policy, process ownership, persistence, and the daemon API.

The contract is deliberately narrower than a harness's native protocol. An
adapter translates one external CLI into this contract; it never delegates
authority decisions back to a client or to untrusted adapter output.

## Negotiation

Before a v1 adapter is admitted, the runner and adapter exchange an offer:

```json
{
  "contract": "coven.harness",
  "versions": ["coven.harness.v1"],
  "requiredExtensions": [],
  "optionalExtensions": ["input.image.v1"]
}
```

The runner selects the highest mutually supported version. There is no
implicit downgrade: a missing intersection fails with
`unsupported_contract_version` before any harness argv is constructed or a
process is spawned. A different contract name fails with
`contract_name_mismatch`.

The current v1 extensions are `input.image.v1` and `output.tool-use.v1`.
Their use is optional. An unknown optional extension is ignored; an unknown
required extension fails closed with `unknown_required_extension`; and a known
required extension that the peer does not offer fails with
`required_extension_unavailable`.

## Required v1 behavior

These behaviors are part of the v1 base contract and do not need extension
names:

- The adapter reports a `ready` frame for the Coven session id.
- Text input is a runner `input` frame with a session id, request id, and
  typed content. It is accepted only while the session is live. Image content
  requires the negotiated `input.image.v1` extension.
- Output is framed, ordered, and tagged with the Coven session id. `text`,
  `raw`, and `semantic` output are distinct kinds; raw output is evidence, not
  a success signal.
- Output and terminal frames use a strictly increasing `sequence` value.
- The adapter emits exactly one terminal frame: `completed`, `failed`, or
  `cancelled`. A `failed` terminal includes a non-empty machine-readable
  `error.code` and a human-readable `error.message`.
- A cancellation request is explicit. The adapter may acknowledge it with
  `cancellation_acknowledged` and must then terminate `cancelled`, or it may
  terminate `cancelled` directly. Coven may force-stop its owned process tree
  after the cancellation deadline; that is recorded as forced cancellation,
  never as a clean completion.

The runner retains any output emitted before a failure or cancellation. Missing
terminal frames, frames after terminal, mismatched session ids, non-monotonic
sequences, malformed required fields, and unknown frame types are protocol
failures. They must mark the run failed rather than becoming a
success-shaped fallback.

## Frame examples

The runner sends a structured input or cancellation request:

```json
{"type":"input","session_id":"session-1","request_id":"turn-1","content":{"type":"text","text":"Fix the failing test."}}
{"type":"cancel","session_id":"session-1","request_id":"cancel-1"}
```

The adapter then replies with its lifecycle and output frames:

```json
{"type":"ready","session_id":"session-1"}
{"type":"output","session_id":"session-1","sequence":1,"kind":"semantic","text":"I found the failing test."}
{"type":"terminal","session_id":"session-1","sequence":2,"outcome":"completed"}
```

Failure preserves prior frames and closes explicitly:

```json
{"type":"terminal","session_id":"session-1","sequence":2,"outcome":"failed","error":{"code":"provider_error","message":"Provider rejected the turn."}}
```

Fields added in a later version are optional by default. V1 readers ignore an
unknown field on a known frame, but never ignore a missing base field or an
unknown `type`. This makes compatible additions safe without treating spelling
errors in required protocol elements as capability.

## Version and migration policy

`coven.harness.v1` is additive within v1. Removing or changing a required
field, frame type, lifecycle meaning, cancellation meaning, or error behavior
requires a new contract version. The daemon API version (`coven.daemon.v1`) and
the harness contract version are independent: the first governs client-to-
daemon requests, while this contract governs the runner-to-adapter boundary.

Existing manifest/PTY recipes remain supported during migration through the
legacy compatibility bridge. They are not treated as v1 adapters merely
because their command arguments happen to work. New adapters should implement
and advertise v1; existing adapters graduate after their golden fixture passes
and their real smoke test proves the declared lifecycle and cancellation path.
There is no flag-day release: legacy recipes retain their conservative,
one-shot behavior until they opt into the negotiated contract.

## Conformance suite

The committed golden vectors live at
`crates/coven-cli/tests/fixtures/harness-contract/v1/`. They cover Codex,
Claude Code, Copilot CLI, Coven Code, and an external adapter. The Rust suite
checks negotiation, forward-compatible optional fields, required-field failure,
monotonic lifecycle ordering, partial failure, and cancellation acknowledgement.

Run the focused suite with:

```bash
cargo test -p coven-cli harness_contract
```

Golden fixtures contain only synthetic session ids and output. Do not put real
paths, prompts, transcripts, credentials, or provider responses in them.
