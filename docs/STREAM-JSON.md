# Coven CLI Stream-JSON Protocol

`coven run <harness> --stream-json` emits newline-delimited JSON events on
stdout. With `--stream-json-input`, user messages are read line-by-line from
stdin as JSON (claude harness only).

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

### `assistant` - emitted by stream-capable harnesses (claude)

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

Emitted only for harnesses without a native stream-json protocol (codex,
external adapters). Coven runs those harnesses on a PTY and wraps every
captured chunk in one `output` frame so stdout stays JSONL-only — raw PTY
bytes never interleave with the stream. `text` is the raw PTY text: it may
contain ANSI escape sequences, carriage returns, and partial lines, and
chunk boundaries follow PTY reads rather than line breaks. Consumers that
want clean text should concatenate the `text` fields in order and strip
escapes. Never emitted on the claude pass-through path.

### `result` - emitted once at end

```json
{"type":"result","subtype":"success","duration_ms":1234,"is_error":false,"num_turns":1,"session_id":"...","error":null}
```

`subtype` is `"success"` on a clean exit and `"error_during_execution"`
otherwise. When `is_error` is true, `error` may contain a human-readable
failure string.

## Stdin protocol (`--stream-json-input`)

Each line is one JSON object with the same shape as the `user` event's
`message` field:

```json
{"role":"user","content":[{"type":"text","text":"hello"}]}
```

`image` content blocks use `source.type = "path"` (local file) or
`source.type = "base64"` (inline). `--stream-json-input` requires
`--stream-json` and is only consumed when the harness is `claude`; for
non-stream harnesses the flag is accepted but no stdin forwarding occurs.

## Harness behavior

- **claude**: passes through claude's native stream-json events, framed by
  Coven's `system` and `result`. Coven does *not* synthesize a `user` event
  on this path; claude emits its own when it processes the prompt.

- **codex** and other non-stream harnesses: Coven synthesizes
  `system.init` + `user` + `result`, and wraps the harness's raw PTY
  output in `output` frames between them. The harness always launches in
  its one-shot/non-interactive form on this path (stream-json is a machine
  protocol; a TUI would wait on keystrokes for a screen that is never
  rendered). The output is not parsed into `assistant` events — `output`
  carries the raw text, escapes and all.

  stdout carries only JSONL frames regardless of harness; anything the
  harness writes to its PTY rides inside `output` events instead of
  leaking onto the stream.

## Stability

This is a stable interface as of 2026-05. Additions are additive (new event
types, new optional fields). Removals or breaking changes to existing event
shapes require a major version bump of the CLI.

The `output` event type was added 2026-07 as an additive change under this
policy; consumers must ignore event types they don't recognize.
