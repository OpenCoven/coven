# Chat Conversation Persistence

How `coven chat` keeps follow-up messages in the same conversation, and how to
extend the mechanism to additional harnesses.

## Status

| Harness | Resume support | Mechanism |
| --- | --- | --- |
| `claude` | ✅ stream-mode | Long-lived `claude --print --input-format stream-json --output-format stream-json --verbose` daemon process per chat, plus `--session-id <uuid>` on the first turn and `--resume <uuid>` for cross-restart continuation. Turn 1 spawns + sends initial user envelope; turns 2..N pipe a new user envelope into the same stdin (no cold-start). Unix kills the stream process tree with `setsid()` + `kill(-pid, SIGKILL)`; Windows uses a Job Object owned by the daemon. |
| `codex` | ✅ per-turn | Chat runs plain `codex exec …`; it captures `session id: <uuid>` from output and feeds it back as `codex exec … resume <uuid> <prompt>` on later turns. `coven run codex --stream-json` separately uses Codex's one-shot `exec --json` protocol, but Codex has no long-lived stream mode, so each chat turn cold-starts. |
| `copilot` | ✅ per-turn | Chat pre-assigns a UUID on turn 1 (`copilot --session-id <uuid> --prompt=…`) and sends the same `--session-id <uuid>` on later turns. Copilot has no long-lived stream mode, so each chat turn cold-starts. `--session-id` resumes an existing session *or* creates a fresh one under that id, so stale ids self-heal instead of erroring. |
| `grok` (experimental recipe) | ✅ per-turn | Chat pre-assigns a UUID on turn 1 (`grok … --session-id <uuid> --single=…`) and cold-starts later turns with `--resume <uuid>`. Unlike Copilot, the two flags are not interchangeable: `--session-id` refuses an id that already exists ("Session ID … is already in use") and `--resume` refuses an id that doesn't ("Session does not exist"), so stale ids are handled by the auto-recovery arm below rather than self-healing. Note: chat launches pass no `--permission` (true of every harness), which leaves Grok in its auto-cancel default — chat turns are read-and-answer only; see `docs/harnesses/grok-build.md`. |

Conversations persist across `coven chat` invocations on a per-project basis:
on startup the chat seeds its in-memory map from
`$COVEN_HOME/chat-conversations/<project-key>.json`, so the next message
sends `Resume` immediately. Different projects get different files (the key
is a deterministic FNV-1a hash of the canonical project root path);
changing project directory yields a fresh thread.

Two slash verbs reset state:

- **`/clear`** clears the visible transcript *and* drops the conversation
  ids (memory + disk). Use it when you want a complete reset.
- **`/new`** drops the conversation ids (memory + disk) but **keeps** the
  visible transcript. Use it when you want to start a fresh thread but
  still scroll up to reference the prior exchange.

The daemon's session store carries a `conversation_id` column so the
`/sessions` overlay can collapse multi-turn chat threads into a single
visible row. The chat passes the harness conversation id as
`conversationId` in every launch payload. Behavior differs between
harnesses because they have different process models:

- **Codex (per-turn)**: every chat turn cold-starts a new daemon session.
  Turn 1 lands as its own singleton row in `/sessions` because chat
  doesn't learn codex's session id until it appears in the run banner
  *after* launch — so the launch payload has no `conversationId` to
  group by. Turn 2 onward carries the captured id and groups together
  into one entry with an `Nt` turn-count badge that increments per
  turn. Net display: 1 singleton row for the cold start + 1 collapsed
  entry covering turns 2..N. Fixing the singleton would mean
  decoupling the chat's ledger id from the harness's resume id (chat
  generates its own UUID up front for grouping, separate from
  whatever codex assigns for `exec resume`).
- **Claude (stream-mode)**: only the *first* turn creates a daemon
  session row; subsequent turns are piped into the same long-lived
  process via stdin, with no fresh ledger row per turn. So the overlay
  shows one row per claude chat (no badge — singleton). To see the
  per-turn breakdown, drill into the session's events.

The `conversation_id` column also flows through to `coven sessions` for
non-TUI clients.

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

Codex chat turns launch a fresh daemon session in `NonInteractive` mode
(`codex exec …`) per turn. Claude chat turns launch a single long-lived
daemon session in `Stream` mode (`claude --print --input-format stream-json
--output-format stream-json --verbose …`) on the first turn; every
subsequent turn pipes a JSON user message into the same process's stdin
and reads JSON events back from its stdout — no cold-start. To preserve
conversational state across daemon-session boundaries (codex per-turn,
claude across `coven chat` restarts), the chat app passes a
`ConversationHint` along with each launch:

- **`Init { id }`** — first turn for this harness. The harness CLI is told to
  claim a session under this UUID.
- **`Resume { id }`** — subsequent turn. The harness CLI is told to resume
  that session and append the new prompt.

The chat app keeps a `HashMap<harness_id, conversation_id>` seeded from the
persistence file on startup. On the first turn for a harness that doesn't
have a stored id yet, it generates a UUID (claude, copilot) or waits to
capture one from output (codex), stores it, and sends `Init` (claude,
copilot) or no hint (codex). On every later turn it sends `Resume` with the
stored id. `/clear`
(and Ctrl+L) drop the map *and* the visible transcript; `/new` drops just
the map.

### Data flow

```
chat App startup
  └─ persistence::load_for_project(coven_home, project_root)  → HashMap<harness, id>
       └─ seeds harness_conversation_ids

chat App on user message
  └─ run_harness_prompt(harness, prompt)
       └─ conversation_hint_for_harness(harness)  → Option<ConversationHint>
            └─ (claude pre-assign path) persistence::save_for_project(...)
            └─ LaunchRequest::with_conversation(hint)
                 └─ POST /api/v1/sessions  { ..., "conversation": {"mode": "init"|"resume", "id": "<uuid>"} }
                      └─ daemon: pty_runner::build_harness_command_with_conversation
                           └─ harness::command_parts_for_harness_with_conversation
                                └─ continuity_args(spec, mode, hint)  → ["--print","--resume","<uuid>"]

chat App on output (codex path)
  └─ maybe_capture_codex_session_id(data)
       └─ on hit: insert into map + persistence::save_for_project(...)

chat App on /clear
  └─ harness_conversation_ids.clear()
       └─ persistence::clear_for_project(...)  // deletes the file
```

`continuity_args` is the per-harness translation point — it's where you wire
up a new harness's resume flags. It lives in `crates/coven-cli/src/harness.rs`.
The persistence layer lives in
`crates/coven-cli/src/tui/chat/persistence.rs`.

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
- **Stale ids** — auto-recovered with auto-retry, raw error hidden. If the
  harness CLI rejects our `Resume` because the prior session no longer
  exists (claude: `No conversation found with session ID:`; codex: `no
  rollout found for thread id` / `thread/resume failed`; grok: the full
  printed line `Error: Session does not exist`), the chat detects
  the message in the output stream, drops the id from both memory and disk,
  re-sends the user's original prompt with no resume hint, **and**
  suppresses every remaining event from the failed daemon session (the
  stale-error chunk itself, any trailing teardown output, and the orphaned
  exit event). Copilot never enters this path: its `--session-id` resumes
  re-create a missing session under the same id instead of erroring. The
  transcript reads: "Prior <harness> conversation no
  longer exists. Starting a new one and re-sending your message." → reply
  from the fresh conversation, with no scary raw error in between.
  Bounded to one auto-retry per user turn — a second stale event in the
  same turn falls back to "Send your message again to start a fresh one."
  so a degenerate loop can't pile up launches. Detection uses output-text
  matching because claude and codex exit 0 on the stale-id error (grok
  exits non-zero, but its stderr shares the PTY, so the same matching
  covers it).
- **`/attach`ed sessions.** Typing while attached to a session launched by
  `coven run` (not by chat) still forwards to that session's stdin — the
  resume path only applies to sessions chat itself launched.
- **Concurrent `coven chat` invocations in the same project** race on the
  persistence file (last write wins). For single-user terminal use this is
  fine; multi-terminal workflows should expect the second invocation to
  silently overwrite the first when its turn completes.

## Adding support for a new harness

1. **Map the harness CLI's resume flags.** Read the CLI's docs to find:
   - Whether the CLI lets you pre-assign a session id at launch, or whether
     it auto-generates one (and prints it somewhere parseable).
   - How to resume a session by id in non-interactive mode.

   Claude: pre-assign via `--session-id <uuid>`, resume via `--resume <uuid>`
   — both work with `--print`. Codex: auto-assigns and prints
   `session id: <uuid>` in the run header; resume via `codex exec … resume
   <uuid> <prompt>`. Copilot: `--session-id <uuid>` serves both directions —
   it pre-assigns on a fresh launch and resumes an existing session
   (`--resume` only binds its value as `--resume=<id>`, which the token-pair
   continuity form can't emit).

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

### Stream-mode for codex

Codex doesn't have a long-lived stream-json mode (only `--json` for a
single result), so codex chat turns still cold-start. If Codex ships
something equivalent, the wiring is mostly already there: add `"codex"`
to `harness_supports_stream_mode`, fill in `stream_args` for codex, and
update `daemon::write_stream_message` if codex's user-message envelope
differs from claude's.

### First-party Coven gateway

The longer-term plan: a first-party Coven gateway that holds the model
connection directly. Harness CLIs become one of several backends rather
than the only option. Would let Coven offer chat that doesn't depend on
having claude or codex installed locally, and would unlock features
neither CLI exposes (cross-harness conversation handoff, server-side
multi-user state, …).
