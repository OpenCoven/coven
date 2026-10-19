---
title: "Windows supervisor input protocol implementation plan"
summary: "Bounded launch framing and independent controller acknowledgement for the audit helper."
read_when:
  - Implementing the independent Windows process supervisor
source_adjacent_reason: "Records the private native helper boundary and its unproven integration gates."
---

# Windows Supervisor Input Protocol

**Goal:** Parse a closed, bounded launch request without consuming or reflecting private input, forward exactly the declared body, and require an exact nonce acknowledgement before native execution may resume.

**Architecture:** `crates/coven-conformance-supervisor/src/protocol.rs` owns only framing. The native executable must establish a separate inherited controller lease before reading input, supervise all blocking operations independently, and retain the creation-time Job Object boundary. This module alone does not implement a lease, timeout, executable verification or process cleanup.

## Task 1: Specify and reproduce missing framing

- [x] Add eight initial common tests before implementing framing. The initial stub run compiled and failed five positive cases; three refusal cases passed. Preserve `/tmp/coven-1378-protocol-red.log` as development evidence, not native runtime evidence.
- [x] Reuse already-locked serde/serde_json packages. Do not update any existing package version, checksum or dependency edge.

Review identified Serde positional-array acceptance. A complete valid-array regression failed before the fix (`/tmp/coven-1378-protocol-object-red.log`); an explicit object-form check now precedes typed deserialization, retaining duplicate-field rejection and allowing valid leading whitespace.

The launch header is UTF-8 JSON followed by one LF. Its maximum size is 8192 bytes excluding LF. It has exactly these required fields:

| Field | Requirement |
| --- | --- |
| `schemaVersion` | `coven.automations.windows-job-supervisor.v1` |
| `kind` | `launch` |
| `nonce` | 32 lowercase hexadecimal bytes |
| `subjectSha256` | 64 lowercase hexadecimal bytes |
| `args` | At most 16 strings, no NUL; each within the command encoder's UTF-16 limit |
| `inputBytes` | Integer from zero through 64 MiB |
| `timeoutMs` | Integer from one through 600000 |

Unknown, duplicate, missing, mistyped, malformed or over-budget fields fail with `launch_invalid`. The header conveys a requested digest, not proof of selected executable identity or authenticated provenance. Executable paths remain outside this header.

## Task 2: Implement bounded common functions

- [x] `read_launch`: read at most 8193 bytes through the first LF using a limited reader; deserialize with unknown/duplicate-field rejection; validate identity, arguments and numeric limits. Preserve any buffered binary body in the caller's `BufRead` owner. Never reflect parser or I/O errors.
- [x] `forward_input`: reject an over-budget declaration before I/O; copy exactly the declared count using an 8192-byte stack buffer; require EOF, then flush. The caller closes target stdin after return. Truncated/surplus input and read/write/flush failure refuse the operation. A valid prefix can already have been forwarded before surplus is detected; native coordination must terminate the owned job on any failure and cannot accept a successful root exit until input completion is proven.
- [x] `read_ack`: consume exactly `GO <nonce>\n` (36 bytes), validating the expected nonce. Leave the descriptor open. Later bytes, EOF or error must trigger native lease failure; this function does not implement that watcher.
- [x] Retain fixed errors without payloads, paths or raw I/O details.

Tests cover binary body preservation behind buffering, exact header limit/one extra byte, every newline-truncated prefix, malformed JSON/UTF-8, closed field vocabulary, duplicates, identity/range/argument refusals, exact body forwarding across buffer boundaries, truncated/surplus/over-budget input, exact ACK and its truncations, and minimized I/O failures.

## Task 3: Verify and integrate

- [x] Run all common package tests, including existing command encoding: 14 passed initially; after the reviewed object-shape regression, 15 passed in `/tmp/coven-1378-protocol-object-green.log`.
- [x] Run host and Windows all-target Clippy with warnings denied, formatting and docs ownership against this source.
- [x] Complete independent source review and address required findings.
- [ ] Implement native executable/image verification and dedicated controller lease, independent input pump and coordinated cleanup.
- [ ] Prove native Windows success/failure, blocked I/O, launcher/controller death, timeout/cancellation/output refusal, descendants and final containment.
- [ ] Bind the actual helper artifact and subject to producer/consumer source evidence, then pass native CI before removing Windows refusal.
- [ ] Complete all required local and protected delivery gates before closing #1378.

Blocking reads and writes here have no wall-clock guarantee. Header, ACK, input-body and output waits all require native deadline and controller-closure supervision. No profile, SLO or release eligibility is certified by common parsing tests; #858 remains open.
