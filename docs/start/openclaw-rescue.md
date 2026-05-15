---
summary: "Use Coven to repair a broken OpenClaw checkout without a healthy OpenClaw runtime."
read_when:
  - Your local OpenClaw is broken and you need a repair room
title: "OpenClaw rescue loop"
---

```bash
coven patch openclaw
coven patch openclaw "fix Codex auth profile order after invalidated OAuth token"
coven patch openclaw --repo ~/Documents/GitHub/openclaw/openclaw --harness codex --dry-run
```

`coven patch openclaw` detects the repo, asks what is broken, launches a supervised Codex or Claude Code session, runs verification, and reports changed files. Coven does not commit or push in v0.
