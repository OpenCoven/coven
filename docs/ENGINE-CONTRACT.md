---
title: "Coven ↔ Engine Compatibility Contract (v1)"
summary: "The invocation surfaces, environment contract, and stream-json protocol that coven uses when driving a coven-code engine. Covers CLI surfaces, auth, streaming, and exit codes."
read_when:
  - Adding or changing a CLI surface that coven invokes
  - Implementing engine resolver or MIN_ENGINE_VERSION enforcement
  - Writing contract tests against golden stream fixtures
description: "Versioned compatibility boundary between coven and a coven-code engine binary: invocation surfaces, environment variables, stream-json event types, and exit codes."
---

# Coven ↔ Engine Compatibility Contract (v1)

Coven invokes the engine (coven-code) ONLY through these surfaces. Any breaking
change to them requires bumping `contract_version` here and in coven's
`MIN_ENGINE_VERSION` gate. The engine CI runs coven's contract tests (Phase 2).

## Version

`contract_version: 1`. Enforcement lives in coven's engine resolver
(`crates/coven-cli/src/engine.rs`, `MIN_ENGINE_VERSION` — forthcoming in
Phase 1), which refuses to launch engines older than the minimum compatible
version.

## Invocation surfaces

1. `coven-code` (no args) — interactive TUI, exits 0 on quit
2. `coven-code --version` — stdout: `coven-code <semver>` (single line, no trailing
   text); example: `coven-code 0.6.1`
3. `coven-code --print <prompt>` — headless; `<prompt>` is the positional `[PROMPT]`
   arg (not an option value); result to stdout; exit 0
4. `coven-code --print --input-format stream-json --output-format stream-json` —
   long-lived stream loop; one JSON frame per line on stdin; exits on stdin EOF
5. `coven-code --resume <id>` — resume a previous session by ID (omit ID to resume
   most recent)
6. `coven-code --session-id <tag>` — attach a tracking tag to a headless run (for
   logs/hooks); NOT the same as --resume — does not pin or restore a session
7. `coven-code --model <id>` / `--append-system-prompt <text>` / `--cwd <dir>` —
   accepted and honored; coven passes values through unvalidated
8. `coven-code --permission-mode {default|accept-edits|bypass-permissions|plan}` —
   accepted and honored; coven passes the value through unvalidated
9. `coven-code auth status --json` — machine-readable auth state; coven reads only
   the `loggedIn` boolean; additional fields may be present and are ignored;
   exit 0 = logged in, 1 = not

   Minimal example:
   ```json
   {"loggedIn": false}
   ```

10. `coven-code acp` — Agent Client Protocol server on stdio; newline-delimited
    JSON-RPC 2.0 (verified via source: `crates/acp/src/connection.rs`); subcommand
    accepts no flags and produces no --help output — it is a fast-path in the CLI
    dispatcher
11. `--effort {low|medium|high|max}` → extended-thinking budget; coven maps the
    harness "think" capability to `--effort high`

## Environment

- `COVEN_PARENT=coven`        set by coven on every delegated invocation
- `COVEN_HOME`                coven state root, actively forwarded when set
- `COVEN_DAEMON_SOCKET`       daemon UDS path; inherited through the environment
                              (coven does not clear env), reserved for the
                              Phase 3 daemon-session notifier
- `COVEN_CODE_*`              engine-owned namespace; coven never overrides

## Stream-json events (subset coven parses)

Coven parses the following event types from the engine's stdout stream (surface 4).
Type names are verbatim from the engine protocol:

- `system` (subtype `init`) — emitted once at stream startup; carries `cwd`,
  `session_id`, `tools`, and `model`
- `user` — echoed user message frame; carries `message.role`, `message.content`,
  and `session_id`
- `assistant` — model response; carries `message.role`, `message.content` (text
  blocks or tool-use blocks), `session_id`, and `stop_reason`
- `tool_result` — outcome of a tool execution; carries `tool_use_id`, `content`,
  `is_error`, and `session_id`
- `result` — terminal frame closing each turn; carries `subtype`
  (`success` or `error_during_execution`), `duration_ms`, `is_error`, `num_turns`,
  `session_id`, and `error`

Event schemas: see [docs/STREAM-JSON.md](STREAM-JSON.md).

Note: STREAM-JSON.md documents the output (engine → coven) side of the protocol.

**Bidirectional mode observed kinds (surface 4):** in `--print --input-format stream-json --output-format stream-json` (the coven-code harness mode), the engine emits exactly `system`(init), `assistant`, and `result`(success) — all handled by coven's existing `Event` enum without extension. Verified against the golden fixture at `crates/coven-cli/tests/fixtures/engine/basic.stream.jsonl`.
For input frames (coven → engine on stdin), see the Input frames section below.

### Input frames (stdin, surface 4)

Two shapes are accepted per `stream_mode.rs`:

- Primary (Claude/Coven) shape: `{"type":"user","message":{"role":"user","content":<string or text-block array>}}`
  triggers a turn.
- Legacy shape: `{"role":"user"|"assistant","content":"..."}` — `assistant` frames
  append as prefill without running a turn.

Unknown `type` values are silently ignored. Formal schema forthcoming with the
Phase 2 golden fixtures (`coven/tests/fixtures/engine/` —
forthcoming — added in Phase 2 with the contract test suite).

## Daemon endpoints the engine calls

This section documents the reverse direction: daemon API surfaces that the engine (coven-code) relies on, not surfaces that coven invokes on the engine. The integration is opt-in (`daemonLedger` setting in the engine) and best-effort — failures are logged but do not abort the session.

Full request/response shapes, field validation rules, and error codes are defined in [`docs/API-CONTRACT.md`](API-CONTRACT.md).

### `POST /api/v1/sessions/external`

Called by the engine at session start to register the running session in the daemon ledger. The daemon does not launch or manage the process; it only holds the record.

Key request fields (camelCase JSON body):

| Field            | Required | Notes                                              |
|------------------|----------|----------------------------------------------------|
| `id`             | Yes      | Session id chosen by the engine (e.g. a UUID).     |
| `projectRoot`    | Yes      | Absolute path to the project root.                 |
| `harness`        | Yes      | Harness id, e.g. `"coven-code"`.                   |
| `title`          | No       | Display title; defaults to `"External session"`.   |
| `transcriptPath` | No       | Absolute path to the engine's transcript file.     |

Returns `201` on first registration, `200` on idempotent re-registration with the same id, or `409 session_id_conflict` if a daemon-managed session already holds that id.

### `POST /api/v1/sessions/<id>/complete`

Called by the engine when the session ends to mark it finished in the daemon ledger.

Key request fields (camelCase JSON body):

| Field      | Required | Notes                                                                             |
|------------|----------|-----------------------------------------------------------------------------------|
| `exitCode` | No       | Integer exit code. Absent, `null`, or `0` → `"completed"`. Nonzero → `"failed"`. |

Returns `200` with the updated session record on success, `404 session_not_found` if the id is unknown, or `422 not_external_session` if the session was not registered as external.

## Exit codes (headless)

0 = completed; 1 = errored / budget exceeded; others reserved
