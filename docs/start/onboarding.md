---
summary: "Guided first run, project selection, harness verification, and ritual safety."
read_when:
  - Walking a teammate through their first Coven setup
title: "Onboarding"
description: "The coven onboarding flow: confirm COVEN_HOME, run doctor, validate a project root, pick a harness, and launch your first supervised session."
---

`coven` opens the interactive menu by default. The onboarding flow:

1. Confirms `$COVEN_HOME` and creates it if missing.
2. Runs `coven doctor` and surfaces install hints.
3. Asks for the project root and validates it.
4. Picks a harness (`codex` or `claude`) and verifies its CLI.
5. Suggests the safest first command.

See [Coven TUI](/start/coven-tui) for the slash-command palette.
