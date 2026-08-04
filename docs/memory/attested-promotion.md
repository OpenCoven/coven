---
summary: "The durable authority contract for future attested-memory promotion."
read_when:
  - Implementing attested memory promotion
  - Adding a memory writer or recovery path
title: "Attested memory promotion"
---

Attested promotion is an archival authority operation. It is not a familiar-memory import or restore: import/restore manages reversible file copies, while promotion records an independently verified fact for semantic recall.

There is intentionally no `coven memory promote` command yet. The Rust contract in `coven_memory::promotion` must be implemented by any future writer before that command or API is exposed.

## Identity and privacy

- The canonical claim-log identity is an uppercase ULID. It is portable and is used for attestation and supersession links.
- A portable reference may be a relative path or `session://<familiar>/<date>/<slug>`. Runtime session keys, absolute and Windows drive-relative paths, and traversal segments are rejected.
- Privacy is explicit (`public`, `private`, or `restricted`); a caller may not infer it from a filename or familiar.
- `verified` claims carry snapshot and evidence SHA-256 digests. Claims without adequate evidence must remain `needs-review`.
- Existing mobile memory DTOs use UUIDs. A promotion is omitted from that API until it has an explicit UUID projection; ULIDs are never put into its UUID supersession fields.

## Durable publication and recovery

The journal is append-only. A writer must sync every file and its parent directory before recording the next phase:

1. Prepare the journal, then create and sync the snapshot and redacted attestation artifacts.
2. Commit the SQLite rows as `pending`, bound to the claim ULID.
3. Write, sync, and atomically replace the TurboVec index.
4. Atomically publish the manifest, then mark the SQLite rows visible.

Readers expose a record only when its row is visible and a valid manifest names it. On restart, reconciliation rolls forward only when every required artifact validates; it discards transactions that never committed metadata and otherwise requires manual recovery. This prevents a crash from exposing metadata without a vector or manifest.

The repository privacy guard remains the publication check for source changes. Promotion must not copy or weaken it.
