---
summary: "The rescue loop, including the OpenClaw repair flow."
read_when:
  - Looking up patch
title: "coven patch"
description: "Reference for coven patch: targeted rescue and repair flows, including the openclaw subcommand that drives the OpenClaw harness rescue loop."
---

`coven patch` runs guided repair flows for known project targets. The first target is OpenClaw.

## OpenClaw

Use `coven patch openclaw` when an OpenClaw source checkout needs a local repair pass through a supported harness.

```sh
coven patch openclaw
coven patch openclaw "fix Codex auth profile order after invalidated OAuth token"
coven patch openclaw "fix failing gateway auth test" --repo ~/Documents/GitHub/openclaw/openclaw --harness codex
```

The command selects an OpenClaw repository in this order:

1. `--repo <path>`, when provided.
2. The nearest OpenClaw source checkout above the current directory.
3. The stored OpenClaw repository location in the local Coven store.

When a non-dry-run patch session launches, Coven records the canonical OpenClaw repo path in `<COVEN_HOME>/coven.sqlite3`. Future `coven patch openclaw ...` runs can then be started from outside the OpenClaw checkout without repeating `--repo`.

If the stored path becomes invalid, pass a fresh `--repo <path>` to replace it.

## Safety

The OpenClaw patch flow does not commit or push. It records a Coven session, launches the selected harness in the chosen repo, runs verification after the harness exits, and reports changed files plus verification status.

Use `--dry-run` to inspect the selected repo, prompt brief, and verification profile without launching a harness or updating the stored repo location.
