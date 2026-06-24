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

See [Automation](/automation) for the full surface.
