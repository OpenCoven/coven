---
summary: "Where automation lives in the Coven stack and how it relates to OpenMeow."
read_when:
  - Choosing where to put automation that calls Coven
title: "Automation overview"
---

Coven is the canonical shared local runtime for reusable automation. OpenMeow stays a chat UI and intent layer. The flow is:

```text
user -> OpenMeow -> Coven -> adapters -> desktop/apps
```

See [Automation](/automation) for the full surface.
