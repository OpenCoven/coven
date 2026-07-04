---
summary: "Browse, filter, and act on sessions."
read_when:
  - Looking up sessions
title: "coven sessions"
description: "Reference for coven sessions: list, inspect, filter, and export Coven harness sessions from the local SQLite ledger and append-only event log."
---

## Usage

```bash
coven sessions [--manage | --plain | --json] [--all]
coven sessions search <query> [--json]
```

`coven sessions` is the safest way to find a session id before attaching,
archiving, summoning, or sacrificing. It reads the local store under
`COVEN_HOME` and shows session records newest first.

## Modes

| Mode | Use when |
|---|---|
| `coven sessions` | You are in an interactive terminal and want the browser with actions. |
| `coven sessions --manage` | You want to force the interactive browser even when auto-detection would choose text. |
| `coven sessions --plain` | You need a copyable table for scripts, terminals, or diagnostics. |
| `coven sessions --json` | You need machine-readable output for clients or dashboards. |
| `coven sessions --all` | You need archived records as well as active records. Combine with `--plain` or `--json` when needed. |

The plain table includes the full session id, status, harness, ritual state, and
title. Empty active lists print the next commands to try:

```bash
coven doctor
coven run codex "explain this repo in 5 bullets"
coven sessions --all
```

## Searching events

```bash
coven sessions search "migration OR release"
coven sessions search "session stuck" --json
```

`coven sessions search` runs full-text search over recorded event payloads. Use
the JSON form when a client needs stable fields instead of terminal snippets.

## Lifecycle commands

After copying a session id:

```bash
coven attach <session-id>
coven archive <session-id>
coven summon <session-id>
coven sacrifice <session-id> --yes
```

Archive hides a non-running session from active lists while preserving its
record. Summon restores an archived session and replays it. Sacrifice is the
permanent delete path.

## Related

- [Session lifecycle](/SESSION-LIFECYCLE)
- [Attach](/reference/cli-attach)
- [Archive](/reference/cli-archive)
- [Summon](/reference/cli-summon)
- [Sacrifice](/reference/cli-sacrifice)
