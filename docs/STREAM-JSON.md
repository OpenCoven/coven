# Coven CLI Stream-JSON Protocol

`coven run <harness> --stream-json` emits newline-delimited JSON events on
stdout. With `--stream-json-input`, user messages are read line-by-line from
stdin as JSON when the selected harness declares native stream support.

The schema matches `OpenClaw bridge-code`; SDKs that target Coven Code work
unchanged against Coven CLI. Coven additionally emits the `output` event
type (below) for harnesses without a native stream-json protocol; consumers
should ignore event types they don't recognize.

## Event types

### `system` - emitted once at startup

```json
{"type":"system","subtype":"init","cwd":"/path","session_id":"...","tools":[],"agent_mode":null}
```

### `user` - emitted for each user message

```json
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"..."}]},"session_id":"...","parent_tool_use_id":null}
```

### `assistant` - emitted for normalized harness replies

```json
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"..."}]},"session_id":"...","stop_reason":"end_turn"}
```

### `tool_result` - emitted by stream-capable harnesses

```json
{"type":"tool_result","tool_use_id":"...","content":[{"type":"text","text":"..."}],"is_error":false,"session_id":"..."}
```

### `output` - raw harness output from non-stream harnesses

```json
{"type":"output","text":"raw PTY text…","session_id":"..."}
```

Emitted only for external harnesses without a native machine-readable
protocol. Coven runs those harnesses on a PTY and wraps every
captured chunk in one `output` frame so stdout stays JSONL-only — raw PTY
bytes never interleave with the stream. `text` is the raw PTY text: it may
contain ANSI escape sequences, carriage returns, and partial lines, and
chunk boundaries follow PTY reads rather than line breaks. Consumers that
want clean text should concatenate the `text` fields in order and strip
escapes. Never emitted on the native stream pass-through path.

### `result` - emitted once at end

```json
{"type":"result","subtype":"success","duration_ms":1234,"is_error":false,"num_turns":1,"session_id":"...","harness_session_id":null,"error":null}
```

`subtype` is `"success"` on a clean exit and `"error_during_execution"`
otherwise. When `is_error` is true, `error` may contain a human-readable
failure string. `harness_session_id` is optional/additive. When present, it
is the harness's own resumable conversation id (for example Codex's
`thread_id`), distinct from Coven's stable ledger `session_id`.

## Stdin protocol (`--stream-json-input`)

Each line is one JSON object with the same shape as the `user` event's
`message` field:

```json
{"role":"user","content":[{"type":"text","text":"hello"}]}
```

`image` content blocks use `source.type = "path"` (local file) or
`source.type = "base64"` (inline). `--stream-json-input` requires
`--stream-json` and is only forwarded when the selected harness declares
`capabilities.stream`; for non-stream harnesses the flag is accepted but no
stdin forwarding occurs.

## Harness behavior

- **Stream-capable harnesses**: Coven launches the harness with its declared
  `stream_args` and passes through the harness's native JSONL events, framed by
  Coven's `system` and `result`. Coven does *not* synthesize a `user` event on
  this path; the harness emits its own when it processes the prompt.

- **codex**: Coven launches `codex exec --json` as a one-shot, ordinary-pipe
  process and translates completed Codex agent messages into Coven
  `assistant` events. `thread.started.thread_id` is returned additively as
  `result.harness_session_id` and stored internally for a later `--continue`.
  This is deliberately not a PTY path: on Windows an npm-installed
  `codex.cmd` reads its prompt through stdin (`codex exec -`), avoiding the
  ConPTY/OpenConsole handoff while keeping multiline familiar prompts out of
  the batch command line. A lack of valid Codex JSON activity is bounded by a
  five-minute idle timeout (including a blocked prompt write) and ends with an
  error `result`, rather than leaving a request running forever. Timeout,
  protocol/wrapper failure, and Unix `SIGINT`/`SIGTERM`/`SIGHUP` cancellation
  terminate the owned pipe process tree: a process group on Unix and a Windows
  Job Object when assignment succeeds. On Windows where Job assignment is
  unavailable, Coven uses `taskkill /T /F` for its own timeout/error cleanup;
  an external forced process kill cannot run that fallback.

- **other non-stream harnesses**: Coven synthesizes `system.init` + `user` +
  `result`, and wraps raw PTY output in `output` frames between them. stdout
  carries only JSONL frames; raw PTY bytes never leak onto the stream.

## Stability

This is a stable interface as of 2026-05. Additions are additive (new event
types, new optional fields). Removals or breaking changes to existing event
shapes require a major version bump of the CLI.

The `output` event type was added 2026-07 as an additive change under this
policy; consumers must ignore event types they don't recognize.
