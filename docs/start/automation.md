---
summary: "Where automation lives in the Coven stack and how it relates to OpenMeow."
read_when:
  - Choosing where to put automation that calls Coven
title: "Automation overview"
description: "How Coven fits into automation flows as the shared local runtime under OpenMeow chat, with the user, OpenMeow, Coven, and adapters all aligned."
---

Coven is the canonical shared local runtime for reusable automation. OpenMeow stays a chat UI and intent layer. The flow is:

```text
user -> OpenMeow -> Coven -> adapters -> desktop/apps
```

See [Automation](/automation) for the full surface.
