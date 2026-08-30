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
state, dispatches runs with a fresh familiar + authority resolution, records
the run ledger, and delivers outputs itself. An external runtime may execute
an already-claimed occurrence, but it never owns the schedule and never owns
the record — runtimes are replaceable workers. The `coven.scheduler`
capability stays reserved for multi-host routing decisions and is not the
recurring-work surface.

Use the canonical [CLI reference](https://docs.opencoven.ai/docs/cli) for
scriptable commands and the [local API guide](https://docs.opencoven.ai/docs/reference/api)
for programmatic clients.
