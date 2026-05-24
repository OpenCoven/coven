# Chat Conversation Persistence

How `coven chat` keeps follow-up messages in the same conversation, and how to
extend the mechanism to additional harnesses.

## Status

| Harness | Resume support | Mechanism |
| --- | --- | --- |
| `claude` | ✅ | `claude --print --session-id <uuid>` on turn 1; `claude --print --resume <uuid>` on subsequent turns |
| `codex` | ✅ | Turn 1 runs plain `codex exec …`; chat captures `session id: <uuid>` from output and feeds it back as `codex exec … resume <uuid> <prompt>` on later turns |

Resume support is scoped to a single `coven chat` invocation. Exiting the chat
ends the conversation; the next invocation starts fresh. See **Future work**
below for cross-restart persistence.

The two harnesses differ in *who assigns the session id*:

- **Claude** lets us pre-assign one via `--session-id <uuid>`. The chat app
  generates a UUID upfront, sends `ConversationHint::Init { id }` on turn 1,
  and `Resume { id }` thereafter. The id is known before any output arrives.
- **Codex** assigns its own id and prints it in the run banner. The chat app
  sends *no* hint on turn 1 (so codex assigns), scans the output for
  `session id: <uuid>`, stores it, and sends `Resume { captured_id }` on
  subsequent turns. The first captured id sticks for the rest of the chat —
  later banners (e.g. from `codex exec resume`) don't override it.

`harness::harness_supports_preassigned_session_id` distinguishes the two
modes.

## How it works

Every chat turn launches a fresh daemon session in `NonInteractive` launch
mode (`claude --print …`, `codex exec …`). To preserve conversational state
across those one-shot launches, the chat app passes a `ConversationHint` along
with each launch:

- **`Init { id }`** — first turn for this harness. The harness CLI is told to
  claim a session under this UUID.
- **`Resume { id }`** — subsequent turn. The harness CLI is told to resume
  that session and append the new prompt.

The chat app keeps a `HashMap<harness_id, conversation_id>` for the lifetime
of the `App`. On the first turn for a harness, it generates a UUID, stores it,
and sends `Init`. On every later turn it sends `Resume` with the same id.
`/clear` (and Ctrl+L) drop the map so the next turn starts a brand-new
conversation.

### Data flow

```
chat App
  └─ run_harness_prompt(harness, prompt)
       └─ conversation_hint_for_harness(harness)  → Option<ConversationHint>
            └─ LaunchRequest::with_conversation(hint)
                 └─ POST /api/v1/sessions  { ..., "conversation": {"mode": "init"|"resume", "id": "<uuid>"} }
                      └─ daemon: pty_runner::build_harness_command_with_conversation
                           └─ harness::command_parts_for_harness_with_conversation
                                └─ continuity_args(spec, mode, hint)  → ["--print","--resume","<uuid>"]
```

`continuity_args` is the per-harness translation point — it's where you wire
up a new harness's resume flags. It lives in `crates/coven-cli/src/harness.rs`.

### Why not drive the harness TUI through a PTY?

An earlier approach launched the harness in `Interactive` mode (full TUI) and
piped subsequent messages as raw stdin bytes. That works for turn 1 but turn 2
silently fails: once the harness negotiates the Kitty keyboard protocol
(`CSI > 1 u`), Enter is encoded as `\x1b[13u`, not raw `\n`, so a piped
`"<text>\n"` types the characters into the harness's input box but never
submits. The output stream is also flooded with TUI rendering (spinner frames,
status bars, ANSI repaints) that has to be filtered. Resume via the harness
CLI's own session API avoids both problems.

### What does *not* resume

- **Switching agents mid-conversation** (`/agent codex` then `/agent claude`)
  preserves each harness's own conversation independently — they live in
  separate entries of `harness_conversation_ids`. There's no cross-harness
  context transfer; switching agents effectively starts (or resumes) a
  parallel thread with the new agent.
- **Restarts** of `coven chat`. The conversation ids are in-memory only.
- **`/attach`ed sessions.** Typing while attached to a session launched by
  `coven run` (not by chat) still forwards to that session's stdin — the
  resume path only applies to sessions chat itself launched.

## Adding support for a new harness

1. **Map the harness CLI's resume flags.** Read the CLI's docs to find:
   - Whether the CLI lets you pre-assign a session id at launch, or whether
     it auto-generates one (and prints it somewhere parseable).
   - How to resume a session by id in non-interactive mode.

   Claude: pre-assign via `--session-id <uuid>`, resume via `--resume <uuid>`
   — both work with `--print`. Codex: auto-assigns and prints
   `session id: <uuid>` in the run header; resume via `codex exec … resume
   <uuid> <prompt>`.

2. **Extend `continuity_args` in `crates/coven-cli/src/harness.rs`.** Add a
   new arm to the `match spec.id` block translating `Init` and `Resume` into
   the harness's actual CLI args. Both existing arms are good templates:
   `"claude"` for pre-assigned ids, `"codex"` for the auto-assign +
   capture-from-output flow (`Init` returns `None` so the default args run,
   `Resume` injects `resume <id>` after the prefix args).

3. **Tell the chat app the new harness supports resume.** Add the id to
   `harness_supports_chat_resume` in
   `crates/coven-cli/src/tui/chat/app.rs`. If the harness pre-assigns ids
   (claude-style), also add it to
   `harness::harness_supports_preassigned_session_id` so the chat generates a
   UUID upfront. Auto-assigning harnesses (codex-style) need *no* entry
   there.

4. **For auto-assigning harnesses, wire output capture.** Codex uses
   `extract_codex_session_id` (scans for `session id: <uuid>` lines) called
   from `maybe_capture_codex_session_id` in the chat app's output event
   handler. For a new harness with a different banner format, add a sibling
   extractor and call it from `maybe_capture_codex_session_id` (or refactor
   into a dispatcher keyed on `active_session_harness`).

5. **Add tests** in `harness::tests` covering Init + Resume → expected args,
   matching `claude_init_hint_attaches_session_id_flag_in_print_mode` /
   `codex_resume_hint_uses_exec_resume_subcommand_with_id`.

6. **Add app-level tests** in `tui::chat::app::tests` similar to
   `second_claude_chat_turn_reuses_init_id_as_resume` (pre-assigned) or
   `second_codex_chat_turn_resumes_using_id_captured_from_first_turn_output`
   (capture-from-output), asserting the second turn carries `Resume` with
   the right id.

## Future work

### Cross-restart persistence

Right now closing `coven chat` loses the conversation. To persist:

1. Whenever an id is added to `harness_conversation_ids` (claude's `Init` or
   codex's capture from output), write the `(harness, id)` pair to a
   per-project file under `$COVEN_HOME/chat-conversations/<project-hash>.json`
   (or extend `chat-settings.json`).
2. On `coven chat` startup, read it back and seed `harness_conversation_ids`
   with the stored ids. The next message will send `Resume` directly without
   needing turn 1 to capture anything.
3. Add a `/new` slash command to clear stored ids and start fresh (mirroring
   what `/clear` does in-memory today).
4. Decide what to do when the stored id no longer exists on the harness side
   (claude's `--resume` will error; codex's `exec resume <id>` will error).
   Either surface the error and fall back to no-hint, or detect the
   missing-session error pattern and silently regenerate.

### One ledger row per conversation

Today each chat turn shows up as a separate session in `/sessions`. That's
ledger noise. Options:

- Daemon API change: add a `conversation_id` column to the session store and
  group by it in the `/sessions` overlay.
- Chat-side aggregation: keep displaying one row per launch, but tag each
  with its conversation id and let the overlay collapse them.

### True streaming follow-ups

Each follow-up turn is a fresh process; latency includes the harness CLI's
cold start (~1-3 s for claude). For lower-latency chat, options are:

- Use `claude --input-format stream-json --output-format stream-json` to keep
  one harness process alive across turns, feeding new prompts as JSON
  messages on stdin. Avoids cold-start per turn but requires a
  daemon-side change to keep a long-lived process per chat and route
  per-turn JSON messages to it.
- A first-party Coven gateway that holds the model connection directly, with
  the harness CLI being just one of several backends.
