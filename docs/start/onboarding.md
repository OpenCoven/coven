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
4. Picks a harness (`codex`, `claude`, or `copilot`) and verifies its CLI.
5. Suggests the safest first command.

The older in-process TUI is available only as the deprecated temporary
compatibility fallback `COVEN_LEGACY_TUI=1`; see [Coven TUI](/start/coven-tui)
for its legacy behavior.
