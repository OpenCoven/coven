# Coven CLI Stream-JSON Protocol

`coven run <harness> --stream-json` emits newline-delimited JSON events on
stdout. With `--stream-json-input`, user messages are read line-by-line from
stdin as JSON (claude harness only).

The schema matches `OpenClaw bridge-code` exactly; SDKs that target Coven
Code work unchanged against Coven CLI.

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
  `system.init` + `user` + `result`. The harness's text output is not
  exposed as `assistant` events in this mode (use `coven attach <id>` for
  streamed text). In practice `--stream-json` for codex is most useful with
  `--detach`, which records the session without launching the harness and
  emits init+user+result immediately.

## Stability

This is a stable interface as of 2026-05. Additions are additive (new event
types, new optional fields). Removals or breaking changes to existing event
shapes require a major version bump of the CLI.
