---
summary: "Where automation lives in the Coven stack and how it relates to the chat/intake client."
read_when:
  - Choosing where to put automation that calls Coven
title: "Coven and chat/intake automation"
description: "How Coven fits into automation flows as the shared local runtime under the chat/intake client, with the user, the chat/intake client, Coven, and adapters all aligned."
---

Coven is the canonical shared local runtime for reusable automation. The chat/intake client stays a chat UI and intent layer. The flow is:

```text
user -> chat/intake client -> Coven -> adapters -> desktop/apps
```

## Coven-native routine automations

Recurring routine work (`coven.automations`) is **owned by Coven, end to
end**: Coven stores the canonical routine definitions in its own store (never
a harness home), its scheduler plans and fences every occurrence in durable
state, pins the accepted definition revision and delivery inputs, records the
session and run before spawning, and delivers outputs itself. Output delivery
first reserves the run and occurrence with an idempotent token and content
digest; a crash or post-rename durability failure remains visibly ambiguous
and is never replayed automatically. Output chunks are streamed into a synced
sibling spool while hashing, so aggregate output size does not become an
in-memory buffer. The reservation digest binds the target and final byte
stream, independent of how event-writer batches split those bytes. Spool
identity and phase are persisted before creation; a restarted daemon removes
only the exact recorded `.coven-delivery-*` sibling and never scans or deletes
unrelated files. Unix commits sync the file and parent directory; Windows
uses a write-through atomic replacement. At a run deadline Coven records one
termination request and asks the runtime to kill the session, but keeps the
overlap fence until terminal session evidence arrives; an unproven kill stays
explicitly ambiguous. Output-loss markers refuse delivery rather than
committing surviving fragments, and unresolved automation sessions pin their
events beyond ordinary log retention. Settlement is process-serialized so a
concurrent tick cannot reinterpret another live spool as crash debris; unsafe
or undeletable stale artifacts degrade only their own run and never block
daemon startup. Degraded recovery keeps the run unresolved, and therefore its
event-retention pin and session-deletion fence, until terminal evidence is
captured in the same settlement transaction. A pre-rename cleanup failure
retains the recorded spool pointer until unlink and parent-directory sync are
both confirmed. An external
runtime may execute an already-claimed occurrence, but it never owns the
schedule and never owns the record — runtimes are replaceable workers. The
`coven.scheduler` capability stays reserved for multi-host routing decisions
and is not the recurring-work surface.

Until the authority work tracked in #857 lands, automation create, update,
delete, run, tick, and import operations are accepted only over the
owner-gated local IPC transport. Loopback TCP does not establish process-owner
authority and is read-only for this surface. The current routine format also
does not bind a fresh authority or approval proof per run.

Manual run-now creates one occurrence and one attempt. Coven does not
automatically retry it or reinterpret a prior attempt; explicit retry and
operator-recovery policy remains deferred to the versioned automations
protocol work.

Use the canonical [CLI reference](https://docs.opencoven.ai/docs/cli) for
scriptable commands and the [local API guide](https://docs.opencoven.ai/docs/reference/api)
for programmatic clients.
