---
summary: "A short hands-on loop for people who want to inspect and extend Coven after first setup."
read_when:
  - You finished Getting started and want to understand the runtime surface
title: "Tinkerer's next 30 minutes"
description: "A practical Coven tinkerer path: inspect health, list sessions as JSON, read events, try a fake harness, and map the files to edit next."
---

You have already run `coven doctor`, started the daemon, and launched a first session. This page gives you one focused half-hour to see what Coven owns and where to change it.

## 1. Confirm the daemon contract

```bash
coven daemon status
```

Then inspect the versioned local API. On Unix-like hosts, the socket defaults
to `~/.coven/coven.sock`:

```bash
curl --unix-socket ~/.coven/coven.sock http://coven/api/v1/health
curl --unix-socket ~/.coven/coven.sock http://coven/api/v1/capabilities
```

On Windows, use a named-pipe-capable local IPC client and the endpoint reported
by `coven daemon status`; do not construct a `.sock` path.

You should see `apiVersion: "coven.daemon.v1"` before building client assumptions.

## 2. Read sessions as data

Launch a small session from a project directory:

```bash
coven run codex "summarize this repo in five bullets" --title "Tinkerer smoke"
```

Then compare human and machine output:

```bash
coven sessions
coven sessions --plain
coven sessions --json
```

Use `--json` for clients and scripts. Use the TUI/browser for human review.

## 3. Inspect the event log

Copy a session id from `coven sessions --json`, then read its events on a
Unix-like host:

```bash
curl --unix-socket ~/.coven/coven.sock "http://coven/api/v1/events?sessionId=<session-id>"
```

Events are append-only records. They are why completed work can be replayed after the process exits or the daemon restarts.

## 4. Try the fake harness smoke path

If you are working from source, run the smoke test that boots a temporary daemon and fake Codex binary:

```bash
cargo test -p coven-cli --test smoke -- --nocapture
```

Use this before changing daemon/session behavior. It exercises launch, replay, kill, archive, summon, and sacrifice without private provider credentials.

## 5. Know what to edit

| Change | Start here |
|---|---|
| CLI command behavior | `crates/coven-cli/src/main.rs` |
| Daemon lifecycle and socket health | `crates/coven-cli/src/daemon.rs` |
| API response shape | `crates/coven-cli/src/api.rs` and `docs/API-CONTRACT.md` |
| Session persistence | `crates/coven-cli/src/store.rs` |
| Harness launch behavior | `crates/coven-cli/src/harness.rs` and `crates/coven-cli/src/pty_runner.rs` |
| CLI npm wrapper | `npm/coven` and `packages/cli` |
| OpenClaw bridge | `packages/openclaw-coven` |
| Source docs | `docs` |
| Published docs site | `coven-docs/content/docs` |

When a change touches `/api/v1`, update docs and compatibility tests in the same patch.
