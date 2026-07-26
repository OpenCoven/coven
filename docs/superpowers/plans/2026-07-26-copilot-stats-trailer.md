# Copilot Stats Trailer Formatting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep genuine terminal Copilot usage trailers out of `coven chat` while ensuring stats-shaped assistant prose, incomplete trailers, and every later reply remain visible.

**Architecture:** Replace the current per-line, mode-mutating Copilot filter with a small bounded candidate state inside each `PtyLineBuffer`. Only an exact ordered `Changes → Requests → Tokens → Resume` candidate from a known Copilot session may be discarded, and only when a normal `exit` confirms that it is the terminal suffix. All mismatches fail open by replaying the candidate verbatim through the existing live/batched output sink.

**Tech Stack:** Rust, the existing `coven-cli` TUI state machine, `cargo test`, and the repository CI commands.

---

## Task 1: Encode the fail-open contract as regressions

**Files:**

- Modify: `crates/coven-cli/src/tui/chat/app.rs` (test module near the existing Copilot and PTY tests)

- [ ] Add one test constant and one terminal-event helper beside the existing `output_event`/`agent_text` test helpers:

```rust
const COPILOT_STATS_TRAILER: &str = concat!(
    "Changes    +1 -1\n",
    "Requests   1 Premium (8s)\n",
    "Tokens     ↑ 28.0k (20.4k cached) • ↓ 32\n",
    "Resume     copilot --resume=cb845dd4-234f-46a0-8e6a-7f15ce8170be\n",
);

fn terminal_event(seq: i64, session_id: &str, kind: &str) -> EventRecord {
    EventRecord {
        seq,
        id: format!("event-{seq}"),
        session_id: session_id.to_string(),
        kind: kind.to_string(),
        payload_json: serde_json::json!({
            "status": if kind == "exit" { "completed" } else { "killed" }
        })
        .to_string(),
        created_at: "2026-07-26T00:00:00Z".to_string(),
    }
}
```

- [ ] Replace the old line-oriented expectations in
  `copilot_stats_lines_hide_from_chat_transcript`,
  `copilot_transcript_keeps_prose_and_drops_stats_block`, and
  `plain_output_keeps_marker_like_prose_and_still_drops_stats_block` with
  end-to-end tests for the approved trailer contract:

```rust
#[test]
fn copilot_resume_shaped_prose_and_following_reply_stay_visible() {
    let client = RecordingChatClient::with_session(test_session(
        "session-1",
        "copilot",
        "Existing",
        "running",
    ));
    client.events.borrow_mut().push(output_event(
        1,
        "session-1",
        concat!(
            "Use the saved command below.\n",
            "Resume     copilot --resume=example\n",
            "Then verify the result.\n",
        ),
    ));
    let (mut app, _) = app_with_client(client);

    app.handle_slash_command("/attach session-1");

    assert_eq!(
        agent_text(&app),
        concat!(
            "Use the saved command below.\n",
            "Resume     copilot --resume=example\n",
            "Then verify the result.\n",
        )
    );
    assert_eq!(app.agent_output_mode, AgentOutputMode::Unknown);
}

#[test]
fn copilot_complete_stats_shape_followed_by_prose_is_visible_verbatim() {
    let client = RecordingChatClient::default();
    let (mut app, _) = app_with_client(client);
    app.active_session_id = Some("session-1".to_string());
    app.active_session_harness = Some("copilot".to_string());

    app.push_event_message(&output_event(
        1,
        "session-1",
        &format!("{COPILOT_STATS_TRAILER}Explanation continues.\n"),
    ));

    assert_eq!(
        agent_text(&app),
        format!("{COPILOT_STATS_TRAILER}Explanation continues.\n")
    );
}

#[test]
fn copilot_false_positive_split_across_chunks_stays_visible() {
    let client = RecordingChatClient::default();
    let (mut app, _) = app_with_client(client);
    app.active_session_id = Some("session-1".to_string());
    app.active_session_harness = Some("copilot".to_string());

    app.push_event_message(&output_event(1, "session-1", "Res"));
    app.push_event_message(&output_event(
        2,
        "session-1",
        "ume     copilot --resume=example\nLater prose.\n",
    ));

    assert_eq!(
        agent_text(&app),
        "Resume     copilot --resume=example\nLater prose.\n"
    );
}

#[test]
fn copilot_out_of_order_candidate_flushes_verbatim() {
    let client = RecordingChatClient::default();
    let (mut app, _) = app_with_client(client);
    app.active_session_id = Some("session-1".to_string());
    app.active_session_harness = Some("copilot".to_string());
    let out_of_order = concat!(
        "Changes    +1 -1\n",
        "Tokens     ↑ 28.0k (20.4k cached) • ↓ 32\n",
    );

    app.push_event_message(&output_event(1, "session-1", out_of_order));

    assert_eq!(agent_text(&app), out_of_order);
}

#[test]
fn copilot_partial_stats_candidate_flushes_on_exit() {
    let client = RecordingChatClient::default();
    let (mut app, _) = app_with_client(client);
    app.active_session_id = Some("session-1".to_string());
    app.active_session_harness = Some("copilot".to_string());

    let partial = concat!(
        "Changes    +1 -1\n",
        "Requests   1 Premium (8s)\n",
    );
    app.push_event_message(&output_event(1, "session-1", partial));
    assert!(agent_text(&app).is_empty());

    app.push_event_message(&terminal_event(2, "session-1", "exit"));

    assert_eq!(agent_text(&app), partial);
}

#[test]
fn copilot_complete_terminal_stats_trailer_is_hidden_on_exit() {
    let client = RecordingChatClient::default();
    let (mut app, _) = app_with_client(client);
    app.active_session_id = Some("session-1".to_string());
    app.active_session_harness = Some("copilot".to_string());

    app.push_event_message(&output_event(
        1,
        "session-1",
        &format!("Answer.\n{COPILOT_STATS_TRAILER}"),
    ));
    assert_eq!(agent_text(&app), "Answer.\n");
    assert!(app.pty_line_buffers.contains_key("session-1"));

    app.push_event_message(&terminal_event(2, "session-1", "exit"));

    assert_eq!(agent_text(&app), "Answer.\n");
    assert!(!app.pty_line_buffers.contains_key("session-1"));
}

#[test]
fn stats_shaped_prose_is_visible_for_non_copilot_harnesses() {
    for harness in ["codex", "claude", "grok", "custom"] {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.active_session_id = Some(format!("{harness}-session"));
        app.active_session_harness = Some(harness.to_string());
        let session_id = app.active_session_id.clone().expect("active session");
        let transcript = format!(
            "assistant\n{COPILOT_STATS_TRAILER}This belongs to {harness}.\n"
        );

        app.push_event_message(&output_event(1, &session_id, &transcript));
        app.push_event_message(&terminal_event(2, &session_id, "exit"));

        let visible = agent_text(&app);
        assert!(
            visible.contains("Resume     copilot --resume="),
            "{harness} must not run the Copilot trailer recognizer: {visible:?}"
        );
        assert!(
            visible.contains(&format!("This belongs to {harness}.")),
            "{harness} prose after stats-shaped text must stay visible: {visible:?}"
        );
    }
}
```

- [ ] Run the regressions against the old implementation and confirm RED:

```sh
cargo test -p coven-cli tui::chat::app::tests::copilot_resume_shaped_prose_and_following_reply_stay_visible -- --exact --nocapture
cargo test -p coven-cli tui::chat::app::tests::copilot_false_positive_split_across_chunks_stays_visible -- --exact --nocapture
cargo test -p coven-cli tui::chat::app::tests::copilot_partial_stats_candidate_flushes_on_exit -- --exact --nocapture
cargo test -p coven-cli tui::chat::app::tests::stats_shaped_prose_is_visible_for_non_copilot_harnesses -- --exact --nocapture
```

Expected: assertion failures showing the `Resume` line and later prose are absent, the partial candidate is lost at exit, and non-Copilot output is filtered. Do not accept a compile error as the RED result.

## Task 2: Implement the bounded Copilot trailer candidate

**Files:**

- Modify: `crates/coven-cli/src/tui/chat/app.rs:35-60`
- Modify: `crates/coven-cli/src/tui/chat/app.rs` around `active_session_emits_codex_markers`
- Modify: `crates/coven-cli/src/tui/chat/app.rs` around `dispatch_pty_output` and `flush_pty_line_buffer`
- Modify: `crates/coven-cli/src/tui/chat/app.rs` around the output-classification helpers

- [ ] Add typed, bounded candidate state before `PtyLineBuffer`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopilotStatsLine {
    Changes,
    Requests,
    Tokens,
    Resume,
}

#[derive(Debug, Default)]
struct CopilotStatsCandidate {
    /// Exact cleaned lines, including their original newlines. Logic below
    /// bounds this to the four recognized trailer rows.
    lines: Vec<String>,
    /// Copilot may print one cosmetic blank after its terminal table.
    trailing_blank: bool,
}

impl CopilotStatsCandidate {
    fn expected(&self) -> Option<CopilotStatsLine> {
        match self.lines.len() {
            0 => Some(CopilotStatsLine::Changes),
            1 => Some(CopilotStatsLine::Requests),
            2 => Some(CopilotStatsLine::Tokens),
            3 => Some(CopilotStatsLine::Resume),
            _ => None,
        }
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    fn is_complete(&self) -> bool {
        self.lines.len() == 4
    }

    fn clear(&mut self) {
        self.lines.clear();
        self.trailing_blank = false;
    }

    fn take_visible(&mut self) -> String {
        let mut visible = std::mem::take(&mut self.lines).concat();
        if std::mem::take(&mut self.trailing_blank) {
            visible.push('\n');
        }
        visible
    }

    fn push_line(&mut self, raw_line: &str, visible: &mut String) {
        let marker = raw_line.trim_end_matches('\n').trim();
        if self.is_complete() && marker.is_empty() {
            self.trailing_blank = true;
            return;
        }

        let kind = copilot_stats_line_kind(marker);
        if kind == self.expected() {
            self.lines.push(raw_line.to_string());
            debug_assert!(self.lines.len() <= 4);
            return;
        }

        if !self.is_empty() {
            visible.push_str(&self.take_visible());
        }
        if kind == Some(CopilotStatsLine::Changes) {
            self.lines.push(raw_line.to_string());
        } else {
            visible.push_str(raw_line);
        }
    }

    /// Normal EOF confirms a complete candidate as the terminal trailer.
    /// Every incomplete candidate fails open.
    fn finish_at_exit(&mut self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        if self.is_complete() {
            self.clear();
            return None;
        }
        Some(self.take_visible())
    }
}
```

- [ ] Extend `PtyLineBuffer` and centralize its retention predicate:

```rust
#[derive(Debug, Default)]
struct PtyLineBuffer {
    tail: String,
    emitted_len: usize,
    copilot_stats: CopilotStatsCandidate,
}

impl PtyLineBuffer {
    fn has_pending(&self) -> bool {
        !self.tail.is_empty() || self.emitted_len > 0 || !self.copilot_stats.is_empty()
    }
}
```

- [ ] Replace `is_copilot_stats_line` with a typed classifier. Keep the existing strict column/value recognition:

```rust
fn copilot_stats_line_kind(line: &str) -> Option<CopilotStatsLine> {
    fn column<'a>(line: &'a str, label: &str) -> Option<&'a str> {
        line.strip_prefix(label)?
            .strip_prefix("   ")
            .map(str::trim_start)
    }

    if column(line, "Changes").is_some_and(|value| value.starts_with('+')) {
        return Some(CopilotStatsLine::Changes);
    }
    if column(line, "Requests").is_some_and(|value| {
        value.chars().next().is_some_and(|ch| ch.is_ascii_digit())
    }) {
        return Some(CopilotStatsLine::Requests);
    }
    if column(line, "Tokens").is_some_and(|value| value.starts_with('↑')) {
        return Some(CopilotStatsLine::Tokens);
    }
    if column(line, "Resume").is_some_and(|value| value.starts_with("copilot --resume=")) {
        return Some(CopilotStatsLine::Resume);
    }
    None
}
```

- [ ] Remove the Copilot predicate from `human_facing_agent_output`; stats text must never mutate `AgentOutputMode`.

- [ ] Replace `human_facing_plain_output` with harness-neutral passthrough and add the candidate-aware dispatcher:

```rust
fn human_facing_plain_output(data: &str) -> Option<String> {
    clean_terminal_output(data)
}

fn human_facing_copilot_output(
    data: &str,
    candidate: &mut CopilotStatsCandidate,
) -> Option<String> {
    let cleaned = clean_terminal_output(data)?;
    let mut visible = String::new();
    for raw_line in cleaned.split_inclusive('\n') {
        candidate.push_line(raw_line, &mut visible);
    }
    let has_structure = visible.chars().any(|ch| ch == '\n' || !ch.is_whitespace());
    has_structure.then_some(visible)
}

fn human_facing_pty_output(
    data: &str,
    mode: &mut AgentOutputMode,
    codex_markers: bool,
    copilot_stats: bool,
    candidate: &mut CopilotStatsCandidate,
) -> Option<String> {
    if codex_markers {
        human_facing_agent_output(data, mode)
    } else if copilot_stats {
        human_facing_copilot_output(data, candidate)
    } else {
        human_facing_plain_output(data)
    }
}
```

- [ ] Add a separate exact harness gate beside `active_session_emits_codex_markers`:

```rust
fn active_session_emits_copilot_stats(&self) -> bool {
    self.active_session_harness.as_deref() == Some("copilot")
}
```

- [ ] Change `partial_line_may_become_marker` to accept
  `(fragment, codex_markers, copilot_stats)`. Run the stats-prefix logic only
  when `copilot_stats` is true; preserve the Codex marker logic exactly.
  Update every existing test call to pass both booleans.

- [ ] In `dispatch_pty_output`, capture both harness gates, route complete
  lines through `human_facing_pty_output`, and retain state when
  `state.has_pending()`:

```rust
let codex_markers = self.active_session_emits_codex_markers();
let copilot_stats = self.active_session_emits_copilot_stats();
// ...
let classified = human_facing_pty_output(
    &complete,
    &mut self.agent_output_mode,
    codex_markers,
    copilot_stats,
    &mut state.copilot_stats,
);
// ...
if state.has_pending() {
    self.pty_line_buffers.insert(session_id.to_string(), state);
}
```

- [ ] Tighten the trailing-fragment pre-emission condition so prose after a
  candidate cannot leapfrog the held candidate:

```rust
if !state.tail.is_empty()
    && self.agent_output_mode != AgentOutputMode::Hidden
    && state.emitted_len == 0
    && state.copilot_stats.is_empty()
{
    let displayable = clean_terminal_output(&state.tail).filter(|text| {
        !partial_line_may_become_marker(text.trim(), codex_markers, copilot_stats)
    });
    // existing emission body
}
```

- [ ] Rewrite `flush_pty_line_buffer` so it handles an empty raw tail with a
  pending candidate, classifies an un-emitted EOF tail through the same
  harness-aware helper, and finally calls `finish_at_exit`:

```rust
fn flush_pty_line_buffer(&mut self, session_id: &str) {
    let Some(mut state) = self.pty_line_buffers.remove(session_id) else {
        return;
    };
    let codex_markers = self.active_session_emits_codex_markers();
    let copilot_stats = self.active_session_emits_copilot_stats();
    let mut visible = String::new();

    if !state.tail.is_empty() && state.emitted_len == 0 {
        if let Some(text) = human_facing_pty_output(
            &state.tail,
            &mut self.agent_output_mode,
            codex_markers,
            copilot_stats,
            &mut state.copilot_stats,
        ) {
            visible.push_str(&text);
        }
    }
    if let Some(text) = state.copilot_stats.finish_at_exit() {
        visible.push_str(&text);
    }
    self.flush_pty_visible(&mut visible, false);
}
```

- [ ] Run the new contract tests and the complete chat-app test module:

```sh
cargo test -p coven-cli tui::chat::app::tests::copilot_ -- --nocapture
cargo test -p coven-cli tui::chat::app::tests -- --nocapture
```

Expected: all pass. Specifically confirm that existing Codex marker,
CR/backspace/ANSI, whitespace, attach/replay, and batched-output regressions
remain green.

- [ ] Format, inspect, and commit the core fix:

```sh
cargo fmt --check
git diff --check
git diff -- crates/coven-cli/src/tui/chat/app.rs
git status --short
git add crates/coven-cli/src/tui/chat/app.rs
git commit -s -m "fix: make Copilot stats filtering fail open"
```

## Task 3: Cover teardown and state-lifecycle semantics

**Files:**

- Modify: `crates/coven-cli/src/tui/chat/app.rs` (kill handling and tests)

- [ ] Add the kill-path regression first:

```rust
#[test]
fn kill_flushes_copilot_candidate_but_drops_the_unfinished_tail() {
    let client = RecordingChatClient::default();
    let (mut app, _) = app_with_client(client);
    app.active_session_id = Some("session-1".to_string());
    app.active_session_harness = Some("copilot".to_string());
    let candidate = concat!(
        "Changes    +1 -1\n",
        "Requests   1 Premium (8s)\n",
    );

    app.push_event_message(&output_event(
        1,
        "session-1",
        &format!("{candidate}unfinished"),
    ));
    assert!(agent_text(&app).is_empty());

    app.push_event_message(&terminal_event(2, "session-1", "kill"));

    assert_eq!(agent_text(&app), candidate);
    assert!(!agent_text(&app).contains("unfinished"));
}
```

- [ ] Run it and confirm RED:

```sh
cargo test -p coven-cli tui::chat::app::tests::kill_flushes_copilot_candidate_but_drops_the_unfinished_tail -- --exact --nocapture
```

Expected: the old kill branch drops the candidate, leaving an empty agent transcript.

- [ ] Add a narrowly scoped kill helper and call it before flushing the pending batched sink:

```rust
fn flush_pty_candidate_on_kill(&mut self, session_id: &str) {
    let Some(mut state) = self.pty_line_buffers.remove(session_id) else {
        return;
    };
    if !state.copilot_stats.is_empty() {
        self.emit_agent_text(&state.copilot_stats.take_visible());
    }
}
```

Replace the kill branch's direct `pty_line_buffers.remove` with:

```rust
self.flush_pty_candidate_on_kill(&event.session_id);
self.flush_pending_agent_buffer();
```

- [ ] Extend the existing `/clear`, `/new`, and suppressed-session tests so
  they seed a real `Changes` candidate, not only a partial `Resume` line:

```rust
app.push_event_message(&output_event(
    1,
    "session-1",
    "Changes    +1 -1\n",
));
```

Assert:

- `/clear` removes the candidate and it never resurfaces.
- `/new` preserves the candidate because it preserves the visible transcript
  and live PTY session.
- a suppressed terminal event removes the candidate without displaying it.
- a complete candidate plus repeated blank output never stores more than four
  candidate lines (`lines.len() == 4`); the blank-state flag remains boolean.

- [ ] Run the lifecycle tests in both live and batched mode:

```sh
cargo test -p coven-cli tui::chat::app::tests::kill_flushes_copilot_candidate_but_drops_the_unfinished_tail -- --exact --nocapture
cargo test -p coven-cli tui::chat::app::tests::clear_transcript_drops_held_pty_line_fragments -- --exact --nocapture
cargo test -p coven-cli tui::chat::app::tests::start_new_conversation_keeps_held_pty_line_fragments -- --exact --nocapture
cargo test -p coven-cli tui::chat::app::tests::batched_ -- --nocapture
```

Expected: all pass, and the recovered kill candidate reaches the same sink in
both streaming modes.

- [ ] Format, inspect, and commit:

```sh
cargo fmt --check
git diff --check
git diff -- crates/coven-cli/src/tui/chat/app.rs
git add crates/coven-cli/src/tui/chat/app.rs
git commit -s -m "fix: preserve Copilot candidates on cancellation"
```

## Task 4: Verify chunk boundaries and output-mode parity

**Files:**

- Modify: `crates/coven-cli/src/tui/chat/app.rs` (tests only unless a regression exposes a defect)

- [ ] Replace `copilot_stats_line_split_across_chunks_is_still_hidden` with a
  full ordered trailer split at hostile boundaries, then terminate normally:

```rust
#[test]
fn copilot_terminal_stats_trailer_is_hidden_across_pty_chunks() {
    let client = RecordingChatClient::default();
    let (mut app, _) = app_with_client(client);
    app.active_session_id = Some("session-1".to_string());
    app.active_session_harness = Some("copilot".to_string());

    for (seq, chunk) in [
        "Answer.\nCha",
        "nges    +1 -1\nRequests ",
        "  1 Premium (8s)\nTokens     ↑ 28.0k",
        " (20.4k cached) • ↓ 32\nRes",
        "ume     copilot --resume=cb845dd4\n",
    ]
    .into_iter()
    .enumerate()
    {
        app.push_event_message(&output_event(seq as i64 + 1, "session-1", chunk));
    }
    app.push_event_message(&terminal_event(6, "session-1", "exit"));

    assert_eq!(agent_text(&app), "Answer.\n");
}
```

- [ ] Add a batched-mode mirror of the issue #493 false-positive regression:

```rust
#[test]
fn batched_copilot_stats_shaped_prose_fails_open() {
    let client = RecordingChatClient::default();
    let (mut app, _) = app_with_client(client);
    app.handle_slash_command("/stream off");
    app.active_session_id = Some("session-1".to_string());
    app.active_session_harness = Some("copilot".to_string());
    app.is_responding = true;
    let reply = concat!(
        "Changes    +1 -1\n",
        "Requests   1 Premium (8s)\n",
        "This table is part of the answer.\n",
    );

    app.push_event_message(&output_event(1, "session-1", reply));
    app.push_event_message(&terminal_event(2, "session-1", "exit"));

    assert_eq!(agent_text(&app), reply);
}
```

- [ ] Run the entire affected test module again:

```sh
cargo test -p coven-cli tui::chat::app::tests -- --nocapture
```

Expected: all tests pass. If any CR/backspace/ANSI fidelity test fails, fix the
shared PTY buffering logic rather than weakening that test.

- [ ] Commit the regression coverage:

```sh
cargo fmt --check
git diff --check
git add crates/coven-cli/src/tui/chat/app.rs
git commit -s -m "test: cover Copilot trailer formatting boundaries"
```

## Task 5: Run repository gates and prepare the issue-scoped PR

**Files:**

- Verify: `docs/superpowers/specs/2026-07-26-copilot-stats-trailer-design.md`
- Verify: `docs/superpowers/plans/2026-07-26-copilot-stats-trailer.md`
- Verify: `crates/coven-cli/src/tui/chat/app.rs`

- [ ] Refresh the shared claim before the slower gates:

```sh
coven claim heartbeat issue-493
```

- [ ] Run every required Rust and repository gate from the worktree root:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
git diff --check origin/main...HEAD
```

Expected:

- formatting clean;
- clippy exits zero with no warnings;
- workspace tests report zero failures;
- secret guard reports no current-tree or history findings;
- diff check prints nothing.

- [ ] Audit the final scope and history:

```sh
git status --short --branch
git diff --stat origin/main...HEAD
git diff origin/main...HEAD -- crates/coven-cli/src/tui/chat/app.rs
git log --oneline --decorate origin/main..HEAD
gh pr list --state open --search "493 in:body,head"
```

Expected: only the approved design, this implementation plan, and the
issue-493 `app.rs` change are present; no duplicate PR exists.

- [ ] Push the issue branch and open one PR:

```sh
git push -u origin fix/493-copilot-stats-trailer
gh pr create \
  --title "fix: make Copilot stats trailer filtering fail open" \
  --body $'Closes #493\n\n## Summary\n- treat Copilot usage stats as a bounded, ordered terminal trailer instead of mode-changing lines\n- fail open for partial, out-of-order, followed-by-prose, cancelled, and non-Copilot output\n- preserve PTY chunk fidelity and live/batched output parity\n\n## Verification\n- cargo fmt --check\n- cargo clippy --workspace --all-targets -- -D warnings\n- cargo test --workspace --locked\n- python scripts/check-secrets.py\n- git diff --check origin/main...HEAD'
```

- [ ] Inspect the created PR and its initial checks:

```sh
gh pr view --json number,url,title,body,headRefName,baseRefName
gh pr checks
gh pr checks --watch --interval 10
```

Expected: all required checks finish green. Do not merge. Keep `issue-493`
claimed until the PR is merged or the work is explicitly stopped.
