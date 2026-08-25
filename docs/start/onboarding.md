---
summary: "Guided first run, project selection, harness verification, and ritual safety."
read_when:
  - Walking a teammate through their first Coven setup
title: "Onboarding"
description: "The coven onboarding flow: confirm COVEN_HOME, run doctor, validate a project root, pick a harness, and launch your first supervised session."
---

Bare `coven`, `coven chat`, and `coven tui` open the managed Coven interactive
UI powered by `coven-code`. On the first interactive run, Coven offers to
install the pinned engine if it is missing. The onboarding flow:

1. Confirms `$COVEN_HOME` and creates it if missing.
2. Runs `coven doctor` and surfaces install hints.
3. Asks for the project root and validates it.
4. Picks a harness (`codex`, `claude`, or `copilot`) and checks that its CLI is
   visible.
5. Suggests the safest first command.

Doctor does not log in to a provider or verify provider access. Complete the
provider-owned login in the same terminal:

```sh
coven setup codex
# or
coven setup claude
# or
coven setup copilot
```

These run `codex login`, `claude auth login`, and `copilot login`
respectively, after explicit consent. Use `coven setup all` to process all
three providers in order.

Provider verification is optional and separately consented because it uses the
network and may incur provider usage or cost:

```sh
coven setup codex --verify
# or, when login is already complete:
coven setup codex --verify-only
```

Setup requires a TTY and hands stdin, stdout, and stderr directly to the
provider. It does not capture provider output or emit machine JSON while the
provider runs. Release operators can write an atomic, redacted, fail-if-exists
report for one provider with `--report-json <path>`. See
[`coven setup`](/reference/cli-setup) for the full privacy and report contract.

The older in-process TUI is available only as the deprecated temporary
compatibility fallback `COVEN_LEGACY_TUI=1`; see [Coven TUI](/start/coven-tui)
for its legacy behavior.

## First session

After setup:

```sh
coven doctor
coven daemon start
cd /path/to/project
coven run codex "explain this repo in 5 bullets"
coven sessions
```
