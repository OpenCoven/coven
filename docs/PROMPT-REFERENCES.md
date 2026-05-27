# Prompt References

Coven expands four kinds of references in `coven run` prompts before sending them to a harness. Expansion happens inside the daemon's `run_session` path; the inlined content is prepended to the original prompt with delimited blocks so the harness sees full context and the user keeps their typed prompt intact.

The original prompt is also what becomes the session title and what is echoed to stdout — only the harness invocation receives the expanded form.

## `@path/to/file`

Inlines a single text file resolved relative to the invocation cwd.

- Up to `MAX_TEXT_LINES` lines (default 500). Excess is replaced with `[…truncated at 500 lines]`.
- Each line is capped at `MAX_LINE_CHARS` characters (default 2048).
- A missing path becomes `[missing @path]` so the prompt still flows.
- Image extensions (`.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`) become a placeholder of the form `[image @ /abs/path: image/png, 12345 bytes]`. In a future `--stream-json` mode these become real `image` content blocks.

Example:

```
coven run claude "explain @README.md briefly"
```

## `@glob/*.ext`

Expands a glob relative to the invocation cwd. Up to 20 matching files are inlined; once the cap is hit, the remaining matches are summarized as `[…glob match cap reached at 20 files]`. Each match is rendered with the same per-file rules as `@path`.

A glob that resolves to zero files produces `[no matches for @pattern]`.

Example:

```
coven run codex "summarise @docs/*.md and note duplicates"
```

## `@T-<session-id>`

Inlines up to 200 redacted event payloads from a prior session, in chronological order. Each event is rendered as `[<created_at>] <kind>: <payload_json>`.

- Lookup is by exact session id; the `T-` prefix is conventional and preserved.
- A missing or empty session becomes `[no events for @T-<id>]`.

Example:

```
coven run claude "continue @T-019e5c86-8f1f-7291-a303-69c37aed291d with new ideas"
```

## `@@search words`

Runs a SQLite FTS5 query over local session event payloads. The query body runs to end-of-line, so the entire line after `@@` becomes the search expression.

- Up to 5 hits are inlined, each as `[<created_at>] <session_id> <kind> <snippet>`.
- Zero hits produces `[no search hits for @@<query>]`.

Example:

```
coven run codex "context:
@@privacy redaction
now finish the helper"
```

## Combining refs

A single prompt can mix all four ref kinds. Refs are resolved in the order they appear; resolved blocks are concatenated and prepended to the original prompt. Order within the prefix matches the order of refs in the source prompt.

```
coven run claude "read @intro.md and @docs/*.md
context: @@phoenix rising
continue @T-019e5c86-8f1f-7291-a303-69c37aed291d"
```

## What does NOT trigger expansion

- A bare `@` followed by whitespace (e.g., `email me at @ work`) — no ref is produced.
- A `@@` with an empty body — ignored.
- A `@` inside a code fence — currently still triggers expansion; quote the literal `@` with a space if you need to mention one.

## Limits and defaults

| Setting | Default | Source |
|---|---|---|
| Lines per file | 500 | `prompt_refs::MAX_TEXT_LINES` |
| Chars per line | 2048 | `prompt_refs::MAX_LINE_CHARS` |
| Glob match cap | 20 files | `expand_glob` in `prompt_refs.rs` |
| Thread event cap | 200 events | `expand_thread` in `prompt_refs.rs` |
| Search hit cap | 5 hits | `expand_search` in `prompt_refs.rs` |

These match the Coven Code (Node) reference implementation's `FILE_MENTION_MAX_*` defaults. They are not currently user-configurable; making them settings-driven is a follow-up under the unified JSONC settings work.

## Where this lives in the code

- Parser + expanders: `crates/coven-cli/src/prompt_refs.rs`
- Wired into `coven run`: `crates/coven-cli/src/main.rs::run_session`
- FTS5 backing for `@@search`: `crates/coven-cli/src/store.rs::search_events`
