//! Chat application state, behavior, and helpers. Owns `App` and all of its
//! methods; provides `discover_agents` and the spinner-frame data.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::{
    harness, project, store,
    tui::cast::{
        self, build_plan, parse_spell,
        plan::{CastHarnessSource, CastPlan},
        safety::{CastRisk, SafetyDecision},
        CastHarness, CastIntent,
    },
};

use super::client::{
    coven_home_dir, ChatClient, ChatDaemonStatus, ChatEventQuery, DaemonChatClient, LaunchRequest,
};
use super::persistence;
use super::settings::{self, ChatSettings, StreamingMode};

// ── Data types ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum MessageRole {
    User,
    Agent,
    System,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AgentOutputMode {
    #[default]
    Unknown,
    Assistant,
    Hidden,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub sender: String,
    pub content: String,
    pub timestamp: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentInfo {
    pub id: String,
    pub label: String,
    pub harness: String,
    pub available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum InputMode {
    Normal,
    AgentSelect,
}

#[derive(Clone, Debug)]
pub(super) enum SlashCommandResult {
    Handled,
    Quit,
    #[allow(dead_code)]
    Unknown(String),
}

// ── App state ──────────────────────────────────────────────────────────────

pub(super) struct App {
    pub(super) messages: Vec<ChatMessage>,
    pub(super) input: String,
    pub(super) cursor_pos: usize,
    pub(super) scroll_offset: usize,
    pub(super) agents: Vec<AgentInfo>,
    pub(super) active_agent: Option<usize>,
    project_label: String,
    pub(super) input_mode: InputMode,
    pub(super) agent_select_index: usize,
    pub(super) show_help: bool,
    pub(super) spinner_frame: usize,
    pub(super) is_responding: bool,
    pub(super) last_tick: Instant,
    pub(super) active_session_id: Option<String>,
    pub(super) last_event_seq: Option<i64>,
    event_poll_backoff_until: Option<Instant>,
    event_poll_failure_streak: u32,
    last_event_poll_error: Option<String>,
    event_poll_paused_for_api_mismatch: bool,
    daemon_status: ChatDaemonStatus,
    pub(super) sessions: Vec<store::SessionRecord>,
    pub(super) show_session_overlay: bool,
    pub(super) input_history: Vec<String>,
    pub(super) history_index: Option<usize>,
    pending_cast_confirmation: Option<CastPlan>,
    streaming_mode: StreamingMode,
    pending_agent_buffer: Option<(String, String)>,
    agent_output_mode: AgentOutputMode,
    coven_home: Option<PathBuf>,
    pub(super) slash_suggestion_index: usize,
    pub(super) slash_popup_dismissed: bool,
    /// Timestamp of the most recent Ctrl+C press, used to require a double
    /// tap before exiting so a stray ^C doesn't kill the session.
    last_interrupt_at: Option<Instant>,
    /// Per-harness conversation ids so chat turns reuse the harness CLI's
    /// own session-resume mechanism. Persisted per-project so a fresh
    /// `coven chat` invocation resumes the prior conversation. Reset on
    /// `/clear`. See `docs/chat-persistence.md`.
    harness_conversation_ids: HashMap<String, String>,
    /// Canonical project root used to scope persisted conversation ids. If
    /// missing (e.g. tests, broken cwd), the chat runs without cross-restart
    /// persistence.
    project_root: Option<PathBuf>,
    /// True when `active_session_id` points at a session this chat launched
    /// as a turn (so the next message should be a fresh launch + resume).
    /// False when the user `/attach`ed an externally-spawned session, in
    /// which case typing is forwarded as stdin to that PTY.
    chat_owns_active_session: bool,
    /// Harness id of `active_session_id`. Used to decide whether output from
    /// the active session is worth scanning for a codex session-id banner.
    active_session_harness: Option<String>,
    /// Most recent user prompt the chat sent through `run_harness_prompt`,
    /// stashed so stale-id recovery can auto-resend it with no hint instead
    /// of asking the user to retype.
    last_chat_prompt: Option<String>,
    /// True if we've already auto-retried once during the current user turn.
    /// Reset by `handle_input` so a fresh user message gets a fresh retry
    /// budget; prevents an infinite loop if the retry itself somehow hits
    /// stale-detection too.
    auto_retry_consumed: bool,
    /// Session ids whose events should be hidden from the visible
    /// transcript. Populated when stale-recovery fires so the raw harness
    /// error chunk, any trailing teardown output, and the orphaned exit
    /// event don't clutter the chat after we've already kicked off a
    /// retry. Entries are cleared once their exit (or kill) event arrives.
    suppressed_session_ids: HashSet<String>,
    /// Per-harness daemon session ids for long-lived stream-mode processes.
    /// On the first chat turn for a stream-capable harness we launch with
    /// `HarnessLaunchMode::Stream` and store the daemon session id here;
    /// subsequent turns reuse it via `send_input` (no fresh launch, no
    /// cold-start). Cleared on session exit/kill/`/clear`/`/new`. Today
    /// only claude is stream-capable. See `docs/chat-persistence.md`.
    harness_stream_session_ids: HashMap<String, String>,
    /// Per-stream-session accumulator for partial JSON lines. Daemon
    /// output events come from 8KiB reads of the child's stdout; a single
    /// JSON line can be split across two events. We buffer the trailing
    /// partial line here and prepend it to the next event so
    /// `dispatch_stream_json_output` only ever tries to parse complete
    /// newline-terminated lines.
    stream_json_buffers: HashMap<String, String>,
    client: Box<dyn ChatClient>,
    /// Optional familiar id for the session owner (e.g. `"sage"`). When set,
    /// delegation events are emitted to `cave-coven-calls.json` whenever
    /// this chat dispatches a harness task to another familiar.
    familiar_id: Option<String>,
    /// Coven call id of the currently-running delegation, if one was emitted.
    /// Cleared when the associated session reaches a terminal event.
    active_call_id: Option<String>,
}

/// Outcome of a Ctrl+C press routed through [`App::handle_interrupt`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InterruptOutcome {
    /// First press (or a press after the arming window expired): the app
    /// stayed alive but cleared composer/session state.
    Cancelled,
    /// Second press within the arming window: the caller should exit.
    Quit,
}

const INTERRUPT_REARM_WINDOW: Duration = Duration::from_secs(2);

/// One row in the slash-command autocomplete popup. `name` is what the popup
/// matches against (including the leading slash) and `summary` is the one-line
/// description rendered next to it.
#[derive(Clone, Copy, Debug)]
pub(super) struct SlashCommand {
    pub(super) name: &'static str,
    pub(super) summary: &'static str,
}

/// Canonical chat slash commands. Ordering controls display ordering when no
/// further filtering applies. Aliases share the same entry; the popup matches
/// by case-insensitive prefix on `name`, so typing `/h` surfaces `/help` (and
/// any other `/h*` command) without us having to enumerate every alias.
pub(super) const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "/help",
        summary: "Toggle the command palette",
    },
    SlashCommand {
        name: "/clear",
        summary: "Clear the transcript and start a fresh thread",
    },
    SlashCommand {
        name: "/new",
        summary: "Start a fresh thread; keep the transcript visible",
    },
    SlashCommand {
        name: "/agent",
        summary: "Switch agent (no arg = picker)",
    },
    SlashCommand {
        name: "/doctor",
        summary: "Show setup checks inline",
    },
    SlashCommand {
        name: "/daemon",
        summary: "Show daemon status inline",
    },
    SlashCommand {
        name: "/sessions",
        summary: "Open the daemon session overlay",
    },
    SlashCommand {
        name: "/attach",
        summary: "Attach to a daemon session",
    },
    SlashCommand {
        name: "/run",
        summary: "Launch <harness> <prompt> via daemon",
    },
    SlashCommand {
        name: "/kill",
        summary: "Stop the active (or named) session",
    },
    SlashCommand {
        name: "/stream",
        summary: "Toggle live agent streaming",
    },
    SlashCommand {
        name: "/export",
        summary: "Save the transcript to ~/.coven/exports/",
    },
    SlashCommand {
        name: "/exit",
        summary: "Quit Coven chat",
    },
];

/// Braille dots animate left-to-right; each frame is width-1 so the status-bar
/// budget stays predictable across NoColor / piped terminals.
pub(super) const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl App {
    pub(super) fn new() -> Self {
        let agents = discover_agents();
        let active_agent = agents.iter().position(|a| a.available);
        Self::new_with_state(
            agents,
            active_agent,
            Box::<DaemonChatClient>::default(),
            Some(coven_home_dir()),
        )
    }

    pub(super) fn new_with_state(
        agents: Vec<AgentInfo>,
        active_agent: Option<usize>,
        client: Box<dyn ChatClient>,
        coven_home: Option<PathBuf>,
    ) -> Self {
        Self::new_with_state_and_project_root(
            agents,
            active_agent,
            client,
            coven_home,
            std::env::current_dir().ok(),
        )
    }

    pub(super) fn new_with_state_and_project_root(
        agents: Vec<AgentInfo>,
        active_agent: Option<usize>,
        mut client: Box<dyn ChatClient>,
        coven_home: Option<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> Self {
        let streaming_mode = coven_home
            .as_deref()
            .map(|home| settings::load_from(home).streaming)
            .unwrap_or_default();
        let daemon_status =
            client
                .daemon_status()
                .unwrap_or_else(|error| ChatDaemonStatus::Unavailable {
                    message: error.to_string(),
                });
        let harness_conversation_ids = match (coven_home.as_deref(), project_root.as_deref()) {
            (Some(home), Some(root)) => persistence::load_for_project(home, root),
            _ => HashMap::new(),
        };
        let mut app = App {
            messages: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            scroll_offset: 0,
            agents,
            active_agent,
            project_label: current_project_label(),
            input_mode: InputMode::Normal,
            agent_select_index: 0,
            show_help: false,
            spinner_frame: 0,
            is_responding: false,
            last_tick: Instant::now(),
            active_session_id: None,
            last_event_seq: None,
            event_poll_backoff_until: None,
            event_poll_failure_streak: 0,
            last_event_poll_error: None,
            event_poll_paused_for_api_mismatch: false,
            daemon_status,
            sessions: Vec::new(),
            show_session_overlay: false,
            input_history: Vec::new(),
            history_index: None,
            pending_cast_confirmation: None,
            streaming_mode,
            pending_agent_buffer: None,
            agent_output_mode: AgentOutputMode::Unknown,
            coven_home,
            slash_suggestion_index: 0,
            slash_popup_dismissed: false,
            last_interrupt_at: None,
            harness_conversation_ids,
            project_root,
            chat_owns_active_session: false,
            active_session_harness: None,
            last_chat_prompt: None,
            auto_retry_consumed: false,
            suppressed_session_ids: HashSet::new(),
            harness_stream_session_ids: HashMap::new(),
            stream_json_buffers: HashMap::new(),
            familiar_id: None,
            active_call_id: None,
            client,
        };

        app.push_system_message("Ready. Type a task or /help.");

        if app.active_agent.is_none() {
            app.push_system_message("No agents available. Run `coven doctor` to check your setup.");
        }

        app
    }

    #[cfg(test)]
    pub(super) fn new_with_client(client: Box<dyn ChatClient>) -> Self {
        let agents = discover_agents();
        let active_agent = agents.iter().position(|a| a.available);
        Self::new_with_state(agents, active_agent, client, None)
    }

    pub(super) fn push_system_message(&mut self, content: &str) {
        self.messages.push(ChatMessage {
            role: MessageRole::System,
            sender: "coven".into(),
            content: content.to_string(),
            timestamp: timestamp_now(),
        });
    }

    fn push_user_message(&mut self, content: &str) {
        self.messages.push(ChatMessage {
            role: MessageRole::User,
            sender: "You".into(),
            content: content.to_string(),
            timestamp: timestamp_now(),
        });
    }

    fn push_agent_message(&mut self, agent_name: &str, content: &str) {
        self.messages.push(ChatMessage {
            role: MessageRole::Agent,
            sender: agent_name.to_string(),
            content: content.to_string(),
            timestamp: timestamp_now(),
        });
    }

    fn push_or_append_agent_message(&mut self, agent_name: &str, content: &str) {
        if let Some(last) = self.messages.last_mut() {
            if matches!(last.role, MessageRole::Agent) && last.sender == agent_name {
                last.content.push_str(content);
                return;
            }
        }
        self.push_agent_message(agent_name, content);
    }

    /// Stash agent output until the session completes (batched mode). Keyed by
    /// sender so a mid-stream agent switch doesn't merge two voices into one
    /// bubble.
    fn buffer_pending_agent_output(&mut self, agent_name: &str, content: &str) {
        match self.pending_agent_buffer.as_mut() {
            Some((sender, buffer)) if sender == agent_name => buffer.push_str(content),
            Some(_) => {
                self.flush_pending_agent_buffer();
                self.pending_agent_buffer = Some((agent_name.to_string(), content.to_string()));
            }
            None => {
                self.pending_agent_buffer = Some((agent_name.to_string(), content.to_string()));
            }
        }
    }

    /// Drain the batched-mode buffer into a single agent message. Called on
    /// session end (exit/kill) and when the user flips streaming back on.
    fn flush_pending_agent_buffer(&mut self) {
        let Some((sender, buffer)) = self.pending_agent_buffer.take() else {
            return;
        };
        if buffer.trim().is_empty() {
            return;
        }
        self.push_agent_message(&sender, &buffer);
    }

    pub(super) fn streaming_mode(&self) -> StreamingMode {
        self.streaming_mode
    }

    pub(super) fn has_pending_batched_output(&self) -> bool {
        self.pending_agent_buffer
            .as_ref()
            .is_some_and(|(_, buffer)| !buffer.is_empty())
    }

    fn set_streaming_mode(&mut self, mode: StreamingMode) {
        if self.streaming_mode == mode {
            let already = match mode {
                StreamingMode::Live => "Streaming is already on.",
                StreamingMode::Batched => "Streaming is already off.",
            };
            self.push_system_message(already);
            return;
        }
        self.streaming_mode = mode;
        // Flipping back to live should not strand any held-back output.
        if mode.is_live() {
            self.flush_pending_agent_buffer();
        }
        if let Some(home) = self.coven_home.clone() {
            let settings = ChatSettings { streaming: mode };
            if let Err(error) = settings::save_to(&home, &settings) {
                self.push_system_message(&format!(
                    "Streaming preference not persisted: {error}. Setting still active for this session."
                ));
            }
        }
        let note = match mode {
            StreamingMode::Live => "Streaming on. Agent output will appear as it arrives.",
            StreamingMode::Batched => {
                "Streaming off. Agent output will appear once the response completes."
            }
        };
        self.push_system_message(note);
    }

    pub(super) fn active_agent_label(&self) -> &str {
        self.active_agent
            .and_then(|idx| self.agents.get(idx))
            .map(|a| a.label.as_str())
            .unwrap_or("none")
    }

    pub(super) fn active_agent_harness(&self) -> &str {
        self.active_agent
            .and_then(|idx| self.agents.get(idx))
            .map(|a| a.harness.as_str())
            .unwrap_or("—")
    }

    pub(super) fn project_label(&self) -> &str {
        &self.project_label
    }

    pub(super) fn active_session_id(&self) -> Option<&str> {
        self.active_session_id.as_deref()
    }

    pub(super) fn daemon_status_label(&self) -> &'static str {
        match self.daemon_status {
            ChatDaemonStatus::Running { .. } => "running",
            ChatDaemonStatus::Stale { .. } => "stale",
            ChatDaemonStatus::Stopped => "stopped",
            ChatDaemonStatus::ApiMismatch { .. } => "mismatch",
            ChatDaemonStatus::Unavailable { .. } => "unavailable",
        }
    }

    pub(super) fn active_session_label(&self) -> String {
        self.active_session_id
            .as_deref()
            .map(short_session_id)
            .unwrap_or_else(|| "none".to_string())
    }

    fn refresh_daemon_status(&mut self) -> ChatDaemonStatus {
        self.daemon_status =
            self.client
                .daemon_status()
                .unwrap_or_else(|error| ChatDaemonStatus::Unavailable {
                    message: error.to_string(),
                });
        self.daemon_status.clone()
    }

    pub(super) fn handle_input(&mut self) -> Option<SlashCommandResult> {
        let raw = self.input.trim().to_string();
        self.input.clear();
        self.cursor_pos = 0;

        if raw.is_empty() {
            return Some(SlashCommandResult::Handled);
        }

        self.event_poll_paused_for_api_mismatch = false;
        // Each user message gets a fresh auto-retry budget.
        self.auto_retry_consumed = false;

        if self.pending_cast_confirmation.is_some() {
            let result = self.resolve_pending_cast_confirmation(&raw);
            self.scroll_to_bottom();
            return Some(result);
        }

        let raw = self.cast_slash_with_context(&raw);

        if raw.starts_with('/') && is_chat_local_slash(&raw) {
            return Some(self.handle_slash_command(&raw));
        }

        self.record_history(&raw);
        self.push_user_message(&raw);
        if raw.starts_with('/') {
            let result = self.launch_chat_session(&raw);
            self.scroll_to_bottom();
            return Some(result);
        }

        // If the user is talking to an externally-spawned session they
        // `/attach`ed to, keep the legacy "type forwards as stdin" flow —
        // it's how you drive a long-running `coven run` task. Chat-owned
        // sessions take the resume path instead.
        if let Some(session_id) = self
            .active_session_id
            .clone()
            .filter(|_| !self.chat_owns_active_session)
        {
            self.forward_input_to_session(&session_id, &raw);
        } else if self.is_responding {
            self.push_system_message(
                "Previous reply is still streaming. Wait for it to finish or press Ctrl+C to interrupt.",
            );
        } else {
            // Route into the chat-launch path. Non-stream harnesses (codex
            // today) cold-start a fresh daemon session per turn, carrying
            // conversation state through the harness CLI's own resume
            // mechanism (--session-id/--resume for claude when not in
            // stream mode, `exec resume <id>` for codex). Stream-mode
            // harnesses (claude on unix) take a fast-path inside
            // `run_harness_prompt` that reuses the existing long-lived
            // daemon session via `forward_input_to_session` instead.
            // See docs/chat-persistence.md.
            self.launch_chat_session(&raw);
        }
        self.scroll_to_bottom();
        Some(SlashCommandResult::Handled)
    }

    /// Clear the visible transcript and reset scroll, matching what `/clear`
    /// does. Used by Ctrl+L so the keybind doesn't have to fake a slash
    /// command through the parser. Also drops the harness conversation ids
    /// (both in-memory and on disk) so the next turn starts a fresh thread
    /// rather than silently resuming after a restart, and tears down any
    /// long-lived stream sessions so the next claude turn cold-starts a
    /// fresh process.
    pub(super) fn clear_transcript(&mut self) {
        self.messages.clear();
        self.scroll_offset = 0;
        self.harness_conversation_ids.clear();
        self.kill_all_stream_sessions();
        self.clear_persisted_conversations();
        self.push_system_message("Chat cleared.");
    }

    /// Drop the in-memory + persisted harness conversation ids without
    /// touching the visible transcript. Useful when a user wants to start
    /// a fresh thread (next message will create a new harness session) but
    /// keep the prior exchange visible for their own reference. Tears down
    /// any long-lived stream sessions for the same reason as `/clear`.
    pub(super) fn start_new_conversation(&mut self) {
        self.harness_conversation_ids.clear();
        self.kill_all_stream_sessions();
        self.clear_persisted_conversations();
        self.push_system_message(
            "Started a new conversation. Your next message creates a fresh thread; the transcript above stays for reference.",
        );
    }

    /// Best-effort shutdown for the chat App: tears down any long-lived
    /// stream-mode daemon sessions so they don't outlive `coven chat`.
    /// Called by `run_chat` on every exit path (slash `/exit`, double
    /// Ctrl+C, Ctrl+D, panic-free unwind of the event loop). Safe to
    /// call multiple times — `kill_all_stream_sessions` is idempotent on
    /// an empty map.
    pub(super) fn shutdown(&mut self) {
        self.kill_all_stream_sessions();
    }

    /// Kill every tracked stream-mode daemon session and clear our local
    /// map (including the per-session JSON buffers — leaving those behind
    /// would leak across a long chat). Best-effort: kill failures are
    /// logged but don't block the caller. Used by `/clear`, `/new`, and
    /// `shutdown` to ensure the next message cold-starts a fresh stream
    /// process (or no process at all, on exit).
    ///
    /// Also clears `active_session_id` if it points at one of the killed
    /// sessions and adds each killed id to `suppressed_session_ids`, so
    /// the user's "Chat cleared."/"Started a new conversation." line
    /// isn't followed by an orphan "Session kill recorded." once the
    /// daemon's kill event eventually polls in.
    fn kill_all_stream_sessions(&mut self) {
        let ids: Vec<String> = self.harness_stream_session_ids.values().cloned().collect();
        for id in &ids {
            if let Err(error) = self.client.kill_session(id) {
                self.push_system_message(&format!(
                    "Stream session {id} kill failed: {error}. Daemon may still hold it."
                ));
            }
            self.stream_json_buffers.remove(id);
            // Suppress the impending kill/exit events for this session so
            // they don't leak back into the transcript after the user
            // reset state.
            self.suppressed_session_ids.insert(id.clone());
            // If the active session is one we're tearing down, clear the
            // active-session fields now so the event poller stops
            // chasing it and the next user input is treated as a fresh
            // turn rather than a "reply still streaming" rejection.
            if self.active_session_id.as_deref() == Some(id.as_str()) {
                self.active_session_id = None;
                self.active_session_harness = None;
                self.chat_owns_active_session = false;
                self.is_responding = false;
            }
        }
        self.harness_stream_session_ids.clear();
    }

    pub(super) fn handle_slash_command(&mut self, input: &str) -> SlashCommandResult {
        let parts: Vec<&str> = input.splitn(2, char::is_whitespace).collect();
        let cmd = parts[0].to_lowercase();
        let arg = parts.get(1).map(|s| s.trim()).unwrap_or("");

        match cmd.as_str() {
            "/help" | "/h" => {
                self.show_help = !self.show_help;
                SlashCommandResult::Handled
            }
            "/clear" | "/cls" => {
                self.clear_transcript();
                SlashCommandResult::Handled
            }
            "/new" => {
                self.start_new_conversation();
                SlashCommandResult::Handled
            }
            "/agent" | "/a" => {
                if arg.is_empty() {
                    self.input_mode = InputMode::AgentSelect;
                    self.agent_select_index = self.active_agent.unwrap_or(0);
                } else {
                    self.switch_agent_by_name(arg);
                }
                SlashCommandResult::Handled
            }
            "/exit" | "/quit" | "/q" => SlashCommandResult::Quit,
            "/session" | "/sessions" => {
                self.refresh_sessions();
                self.show_session_overlay = !self.show_session_overlay;
                SlashCommandResult::Handled
            }
            "/attach" => {
                if arg.is_empty() {
                    self.push_system_message("Usage: /attach <session-id>");
                } else {
                    self.attach_session(arg);
                }
                SlashCommandResult::Handled
            }
            "/export" => {
                self.export_chat();
                SlashCommandResult::Handled
            }
            "/run" => {
                let Some((harness, prompt)) = split_first_arg(arg) else {
                    self.push_system_message("Usage: /run <harness> <prompt>");
                    return SlashCommandResult::Handled;
                };
                let _ = self.run_harness_prompt(harness, prompt);
                SlashCommandResult::Handled
            }
            "/kill" => {
                let session_id = if arg.is_empty() {
                    self.active_session_id.clone()
                } else {
                    Some(arg.to_string())
                };
                match session_id {
                    Some(session_id) => self.kill_session(&session_id),
                    None => {
                        self.push_system_message("No active session. Usage: /kill <session-id>")
                    }
                }
                SlashCommandResult::Handled
            }
            "/palette" | "/commands" => {
                self.show_help = !self.show_help;
                SlashCommandResult::Handled
            }
            "/stream" | "/streaming" => {
                let new_mode = match arg.to_ascii_lowercase().as_str() {
                    "" | "toggle" => self.streaming_mode.toggled(),
                    "on" | "live" => StreamingMode::Live,
                    "off" | "batched" | "complete" => StreamingMode::Batched,
                    "status" => {
                        self.push_system_message(&format!(
                            "Streaming is {}.",
                            self.streaming_mode.status_label()
                        ));
                        return SlashCommandResult::Handled;
                    }
                    other => {
                        self.push_system_message(&format!(
                            "Unknown /stream argument \"{other}\". Usage: /stream [on|off|toggle|status]."
                        ));
                        return SlashCommandResult::Handled;
                    }
                };
                self.set_streaming_mode(new_mode);
                SlashCommandResult::Handled
            }
            _ => SlashCommandResult::Unknown(cmd),
        }
    }

    fn launch_chat_session(&mut self, prompt: &str) -> SlashCommandResult {
        let plan = match parse_spell(prompt)
            .and_then(|intent| build_plan(intent, || self.default_cast_harness()))
            .map(|plan| plan.with_raw_spell(prompt))
        {
            Ok(plan) => plan,
            Err(error) => {
                self.push_system_message(&format!("{error}"));
                return SlashCommandResult::Handled;
            }
        };
        self.dispatch_cast_plan(plan)
    }

    fn dispatch_cast_plan(&mut self, plan: CastPlan) -> SlashCommandResult {
        if should_keep_launch_inline(&plan) {
            self.push_system_message(&format_cast_plan_for_chat(&plan));
        } else if let Some(plan_harness) = plan.harness {
            self.push_system_message(&format!("Starting {}...", plan_harness.harness.label()));
        }

        match &plan.decision {
            SafetyDecision::Proceed => self.execute_cast_plan(plan),
            SafetyDecision::Confirm { suggestion, .. } => {
                self.push_system_message(&format!(
                    "Confirmation required before launch. Type accept to proceed or reject to cancel. {suggestion}"
                ));
                self.pending_cast_confirmation = Some(plan);
                SlashCommandResult::Handled
            }
            SafetyDecision::Reject { alternative, .. } => {
                self.push_system_message(&format!("Cast rejected this spell. {alternative}"));
                SlashCommandResult::Handled
            }
        }
    }

    fn execute_cast_plan(&mut self, plan: CastPlan) -> SlashCommandResult {
        let explicit_callee_id = match &plan.intent {
            CastIntent::FamiliarSpell { familiar_id, .. } => Some(familiar_id.clone()),
            _ => None,
        };

        match plan.intent {
            CastIntent::NaturalSpell { ref prompt }
            | CastIntent::HarnessSpell { ref prompt, .. }
            | CastIntent::FamiliarSpell { ref prompt, .. } => {
                let Some(plan_harness) = plan.harness else {
                    self.push_system_message(
                        "No harness available. Run `coven doctor` to install Codex or Claude Code.",
                    );
                    return SlashCommandResult::Handled;
                };
                if let Some(session) = self.run_harness_prompt(plan_harness.harness.id(), prompt) {
                    if should_keep_launch_inline(&plan) {
                        self.push_system_message(&format_cast_outcome_for_chat(
                            plan_harness.harness.label(),
                            &session.id,
                        ));
                    }
                    // Emit a delegation event when this chat is running as a familiar.
                    if let (Some(home), Some(caller_id)) =
                        (self.coven_home.as_deref(), self.familiar_id.as_deref())
                    {
                        let callee_id = explicit_callee_id.as_deref().unwrap_or("unknown");
                        match crate::coven_calls::emit_running(
                            home,
                            caller_id,
                            callee_id,
                            prompt,
                            Some(&session.id),
                        ) {
                            Ok(call_id) => self.active_call_id = Some(call_id),
                            Err(_err) => {
                                // Non-fatal: delegation event failures must never block the chat.
                            }
                        }
                    }
                }
            }
            CastIntent::OpenSessions | CastIntent::OpenAllSessions => {
                self.refresh_sessions();
                self.show_session_overlay = true;
            }
            CastIntent::AttachSession { session_id } => self.attach_session(&session_id),
            CastIntent::SummonSession { session_id } => self.summon_session(&session_id),
            CastIntent::ArchiveSession { session_id } => self.archive_session(&session_id),
            CastIntent::KillSession { session_id } => self.kill_session(&session_id),
            CastIntent::SacrificeSession { session_id } => self.sacrifice_session(&session_id),
            CastIntent::Doctor => self.push_doctor_summary(),
            CastIntent::DaemonStatus => self.push_daemon_status_summary(),
            CastIntent::Help => self.show_help = true,
            CastIntent::StartHere | CastIntent::OpenTui => {
                self.show_help = true;
                self.push_system_message(
                    "Command discovery is open. Type a task, /run <harness> <task>, /sessions, or /help.",
                );
            }
            CastIntent::PatchOpenClaw => {
                self.push_system_message(
                    "Patch flow: type `patch openclaw <issue>` as a task, or run `coven patch openclaw` for the guided repair flow.",
                );
            }
            CastIntent::Quest { goal } => {
                self.push_system_message(&format!(
                    "Quest planned for: {goal}. Cast will run each phase through this composer; start with the design phase prompt when ready."
                ));
            }
            CastIntent::Quit => return SlashCommandResult::Quit,
        }
        SlashCommandResult::Handled
    }

    fn resolve_pending_cast_confirmation(&mut self, raw: &str) -> SlashCommandResult {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "accept" | "approve" | "yes" | "y" => {
                if let Some(mut plan) = self.pending_cast_confirmation.take() {
                    plan.decision = SafetyDecision::Proceed;
                    self.push_system_message("Accepted Cast confirmation.");
                    return self.execute_cast_plan(plan);
                }
            }
            "reject" | "cancel" | "no" | "n" => {
                self.pending_cast_confirmation = None;
                self.push_system_message("Rejected Cast confirmation.");
            }
            _ => {
                self.push_system_message(
                    "A Cast confirmation is pending. Type accept to proceed or reject to cancel.",
                );
            }
        }
        SlashCommandResult::Handled
    }

    /// Handle a Ctrl+C press. The first press clears modal/composer state and
    /// arms an exit confirmation; a second press inside [`INTERRUPT_REARM_WINDOW`]
    /// returns [`InterruptOutcome::Quit`] so the caller can break out.
    pub(super) fn handle_interrupt(&mut self) -> InterruptOutcome {
        let now = Instant::now();
        if self
            .last_interrupt_at
            .is_some_and(|t| now.duration_since(t) <= INTERRUPT_REARM_WINDOW)
        {
            return InterruptOutcome::Quit;
        }

        // First press: cancel everything cancellable, then arm exit.
        let had_pending = self.cancel_pending_cast_confirmation();
        let had_input = !self.input.is_empty();
        let interrupted_session = self.interrupt_active_session();
        self.input.clear();
        self.cursor_pos = 0;
        self.slash_suggestion_index = 0;
        self.slash_popup_dismissed = false;

        let advisory = if interrupted_session {
            "Interrupt sent. Press Ctrl+C again to exit."
        } else if had_pending || had_input {
            "Cleared. Press Ctrl+C again to exit."
        } else {
            "Press Ctrl+C again to exit."
        };
        self.push_system_message(advisory);

        self.last_interrupt_at = Some(now);
        InterruptOutcome::Cancelled
    }

    /// Best-effort kill of the active daemon session (used by Ctrl+C and Esc).
    /// Returns true if a session was running and a kill request was sent.
    pub(super) fn interrupt_active_session(&mut self) -> bool {
        let Some(session_id) = self.active_session_id.clone() else {
            return false;
        };
        match self.client.kill_session(&session_id) {
            Ok(()) => {
                self.push_system_message(&format!("Kill sent to session {session_id}."));
                self.poll_session_events();
                true
            }
            Err(error) => {
                self.push_system_message(&format!("Kill failed: {error}"));
                false
            }
        }
    }

    pub(super) fn has_pending_cast_confirmation(&self) -> bool {
        self.pending_cast_confirmation.is_some()
    }

    pub(super) fn cancel_pending_cast_confirmation(&mut self) -> bool {
        if self.pending_cast_confirmation.take().is_some() {
            self.push_system_message("Cancelled Cast confirmation.");
            true
        } else {
            false
        }
    }

    fn default_cast_harness(&self) -> Option<CastHarness> {
        self.active_agent
            .and_then(|idx| self.agents.get(idx))
            .filter(|agent| agent.available)
            .and_then(|agent| CastHarness::from_token(&agent.harness))
            .or_else(cast::default_harness)
    }

    fn cast_slash_with_context(&mut self, raw: &str) -> String {
        if raw.trim().eq_ignore_ascii_case("/kill") {
            if let Some(session_id) = self.active_session_id.clone() {
                return format!("/kill {session_id}");
            }
        }
        raw.to_string()
    }

    fn run_harness_prompt(&mut self, harness: &str, prompt: &str) -> Option<store::SessionRecord> {
        self.is_responding = true;
        self.agent_output_mode = AgentOutputMode::Unknown;
        // Stash the prompt so stale-id recovery can auto-resend it without
        // making the user retype.
        self.last_chat_prompt = Some(prompt.to_string());

        // Fast path for stream-mode harnesses (today: claude). If we
        // already have a long-lived stream session for this harness, send
        // the next user message into it instead of cold-starting a new
        // daemon session.
        if harness::harness_supports_stream_mode(harness) {
            if let Some(stream_id) = self.harness_stream_session_ids.get(harness).cloned() {
                self.active_session_id = Some(stream_id.clone());
                self.active_session_harness = Some(harness.to_string());
                self.chat_owns_active_session = true;
                self.reset_event_poll_failures();
                self.forward_input_to_session(&stream_id, prompt);
                // No SessionRecord to return — the caller's "Started
                // daemon session" outcome is suppressed for warm sends.
                return None;
            }
        }

        let hint = self.conversation_hint_for_harness(harness);
        // Same map that holds the harness-CLI session id also serves as the
        // ledger conversation id, so /sessions can collapse multi-turn
        // threads. Codex's very first turn has no entry yet (we capture
        // from output), so it lands as an ungrouped row — see
        // `docs/chat-persistence.md`.
        let conversation_id = self.harness_conversation_ids.get(harness).cloned();
        let launch_mode = if harness::harness_supports_stream_mode(harness) {
            crate::harness::HarnessLaunchMode::Stream
        } else {
            crate::harness::HarnessLaunchMode::NonInteractive
        };
        let result = LaunchRequest::for_current_dir(harness, prompt).map(|mut request| {
            request.launch_mode = launch_mode;
            let request = match hint {
                Some(hint) => request.with_conversation(hint),
                None => request,
            };
            match conversation_id {
                Some(id) => request.with_conversation_id(id),
                None => request,
            }
        });
        let result = result.and_then(|request| self.client.launch_session(request));
        match result {
            Ok(session) => {
                self.active_session_id = Some(session.id.clone());
                self.active_session_harness = Some(session.harness.clone());
                self.chat_owns_active_session = true;
                self.last_event_seq = None;
                self.reset_event_poll_failures();
                if launch_mode == crate::harness::HarnessLaunchMode::Stream {
                    self.harness_stream_session_ids
                        .insert(harness.to_string(), session.id.clone());
                }
                self.push_system_message("Connected. Waiting for the reply.");
                self.poll_session_events();
                Some(session)
            }
            Err(error) => {
                self.is_responding = false;
                self.push_system_message(&format!(
                    "Daemon launch failed: {error}. Run `coven daemon status` to inspect it; use `coven daemon restart` if it remains unreachable."
                ));
                None
            }
        }
    }

    /// Decide whether a launch for `harness` should ride a resumable chat
    /// session, and if so produce the right hint. For harnesses where we can
    /// pre-assign the session id (claude `--session-id`) the first turn sends
    /// `Init` with a freshly generated UUID. For harnesses that auto-assign
    /// (codex) the first turn sends no hint and the id is captured from
    /// output afterwards via `maybe_capture_codex_session_id`.
    fn conversation_hint_for_harness(
        &mut self,
        harness: &str,
    ) -> Option<harness::ConversationHint> {
        if !harness_supports_chat_resume(harness) {
            return None;
        }
        if let Some(id) = self.harness_conversation_ids.get(harness) {
            return Some(harness::ConversationHint::Resume { id: id.clone() });
        }
        if harness::harness_supports_preassigned_session_id(harness) {
            let id = Uuid::new_v4().to_string();
            self.harness_conversation_ids
                .insert(harness.to_string(), id.clone());
            self.persist_conversations();
            Some(harness::ConversationHint::Init { id })
        } else {
            None
        }
    }

    /// Best-effort write of `harness_conversation_ids` to the per-project
    /// persistence file. Logged on failure (as a system message) but never
    /// fatal — the in-memory map is authoritative for the current session.
    fn persist_conversations(&mut self) {
        let (Some(home), Some(root)) = (self.coven_home.as_deref(), self.project_root.as_deref())
        else {
            return;
        };
        if let Err(error) =
            persistence::save_for_project(home, root, &self.harness_conversation_ids)
        {
            self.push_system_message(&format!(
                "Could not persist chat conversation ids: {error}. Resume across restarts may not work."
            ));
        }
    }

    /// Best-effort delete of the per-project persistence file. Called from
    /// `/clear` so a deliberate reset doesn't silently resume on the next
    /// `coven chat` invocation. Logged on failure but never fatal.
    fn clear_persisted_conversations(&mut self) {
        let (Some(home), Some(root)) = (self.coven_home.as_deref(), self.project_root.as_deref())
        else {
            return;
        };
        if let Err(error) = persistence::clear_for_project(home, root) {
            self.push_system_message(&format!(
                "Could not clear persisted chat conversation ids: {error}."
            ));
        }
    }

    /// Send raw text as stdin to a session — either one the user
    /// `/attach`ed to (PTY-backed) or one of our own long-lived stream
    /// sessions. PTY sessions need a trailing newline so Enter submits;
    /// stream sessions don't, because the daemon wraps the payload in a
    /// JSON envelope verbatim and the inner `\n` would otherwise leak
    /// into the user message text on every turn after the first.
    ///
    /// **Limitation**: this distinguishes stream-vs-PTY by checking our
    /// own `harness_stream_session_ids` map, which only knows about
    /// stream sessions this chat instance launched. If a future
    /// `/attach` connects to a stream session launched by another
    /// process (or a stream session that survived a restart), the check
    /// would mis-treat it as PTY and append the spurious `\n`. Today no
    /// flow produces this state — only chat launches stream sessions
    /// and `/attach` is documented for `coven run`-spawned PTY tasks —
    /// but the proper fix is exposing the session kind on
    /// `SessionRecord` so the daemon is the source of truth.
    fn forward_input_to_session(&mut self, session_id: &str, raw: &str) {
        self.is_responding = true;
        let is_stream = self
            .harness_stream_session_ids
            .values()
            .any(|id| id == session_id);
        let payload = if is_stream {
            raw.to_string()
        } else {
            format!("{raw}\n")
        };
        let result = self.client.send_input(session_id, &payload);
        match result {
            Ok(()) => self.poll_session_events(),
            Err(error) => {
                self.is_responding = false;
                self.push_system_message(&format!("Input rejected: {error}"));
                // For stream-mode failures, the long-lived child is
                // almost certainly dead (daemon returns NotLiveError
                // when the registry entry is gone, which only happens
                // after the wait thread reaped the process). Drop the
                // tracking entry and its buffer so the next user
                // message cold-starts a fresh stream session instead
                // of looping into the same dead pipe.
                if is_stream {
                    self.harness_stream_session_ids
                        .retain(|_, id| id != session_id);
                    self.stream_json_buffers.remove(session_id);
                    if self.active_session_id.as_deref() == Some(session_id) {
                        self.active_session_id = None;
                        self.active_session_harness = None;
                        self.chat_owns_active_session = false;
                    }
                }
            }
        }
    }

    pub(super) fn attach_session(&mut self, session_id: &str) {
        match self.client.get_session(session_id) {
            Ok(session) => {
                self.active_session_id = Some(session.id.clone());
                self.active_session_harness = Some(session.harness.clone());
                self.chat_owns_active_session = false;
                self.last_event_seq = None;
                self.agent_output_mode = AgentOutputMode::Unknown;
                self.reset_event_poll_failures();
                self.push_system_message(&format!(
                    "Attached to daemon session {} ({}, {})",
                    session.id, session.harness, session.status
                ));
                self.poll_session_events();
            }
            Err(error) => self.push_system_message(&format!("Attach failed: {error}")),
        }
    }

    fn kill_session(&mut self, session_id: &str) {
        match self.client.kill_session(session_id) {
            Ok(()) => {
                self.push_system_message(&format!("Kill accepted for session {session_id}."));
                self.poll_session_events();
            }
            Err(error) => self.push_system_message(&format!("Kill failed: {error}")),
        }
    }

    fn archive_session(&mut self, session_id: &str) {
        match self.client.archive_session(session_id) {
            Ok(()) => self.push_system_message(&format!("Archived session {session_id}.")),
            Err(error) => self.push_system_message(&format!("Archive failed: {error}")),
        }
    }

    fn summon_session(&mut self, session_id: &str) {
        match self.client.summon_session(session_id) {
            Ok(session) => {
                self.push_system_message(&format!("Summoned session {session_id}."));
                self.active_session_id = Some(session.id.clone());
                self.active_session_harness = Some(session.harness.clone());
                // Summon attaches to an externally-spawned (or
                // previously-archived) session; treat it like /attach so
                // typing forwards to its PTY stdin instead of cold-
                // starting a chat turn over it.
                self.chat_owns_active_session = false;
                self.last_event_seq = None;
                self.agent_output_mode = AgentOutputMode::Unknown;
                self.reset_event_poll_failures();
                self.push_system_message(&format!(
                    "Attached to daemon session {} ({}, {})",
                    session.id, session.harness, session.status
                ));
                self.poll_session_events();
            }
            Err(error) => self.push_system_message(&format!("Summon failed: {error}")),
        }
    }

    fn sacrifice_session(&mut self, session_id: &str) {
        match self.client.sacrifice_session(session_id) {
            Ok(()) => {
                if self.active_session_id.as_deref() == Some(session_id) {
                    self.active_session_id = None;
                    self.active_session_harness = None;
                    self.chat_owns_active_session = false;
                }
                self.push_system_message(&format!("Sacrificed session {session_id}."));
            }
            Err(error) => self.push_system_message(&format!("Sacrifice failed: {error}")),
        }
    }

    pub(super) fn refresh_sessions(&mut self) {
        match self.client.list_sessions() {
            Ok(sessions) => self.sessions = sessions,
            Err(error) => self.push_system_message(&format!("Failed to load sessions: {error}")),
        }
    }

    fn push_doctor_summary(&mut self) {
        let project = std::env::current_dir()
            .ok()
            .and_then(|cwd| project::canonical_project_root(&cwd).ok())
            .map(|root| root.display().to_string())
            .unwrap_or_else(|| "not inside a git/project root yet".to_string());
        let store_path = self.coven_home.clone().unwrap_or_else(coven_home_dir);
        let harnesses = harness::built_in_harnesses();
        let mut lines = vec![
            "Doctor".to_string(),
            format!("  Store    {}", store_path.display()),
            format!("  Project  {project}"),
            "  Harnesses".to_string(),
        ];
        for harness in &harnesses {
            let status = if harness.available {
                "ready"
            } else {
                "missing"
            };
            lines.push(format!(
                "    {:<11} `{}` is {status}",
                harness.label, harness.executable
            ));
        }
        let next = harnesses
            .iter()
            .find(|harness| harness.id == "codex" && harness.available)
            .or_else(|| harnesses.iter().find(|harness| harness.available))
            .map(|harness| {
                format!(
                    "  Next     coven run {} \"explain this repo in 5 bullets\"",
                    harness.id
                )
            })
            .unwrap_or_else(|| {
                "  Next     install or authenticate a supported harness".to_string()
            });
        lines.push(next);
        // Capabilities
        lines.push("  Capabilities".to_string());
        let home = std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
        for (harness_id, label) in &[("codex", "Codex"), ("claude", "Claude")] {
            let m = if *harness_id == "codex" {
                crate::capabilities::scan_codex_capabilities(&home)
            } else {
                crate::capabilities::scan_claude_capabilities(&home)
            };
            let instr = if m.global_instructions.present {
                "✓"
            } else {
                "—"
            };
            let skills_n = m.skills.len();
            let plugins_n = m.plugins.len();
            lines.push(format!(
                "    {label:<11} instructions {instr}  automations {skills_n}  plugins {plugins_n}"
            ));
        }
        self.push_system_message(&lines.join("\n"));
    }

    fn push_daemon_status_summary(&mut self) {
        let status = self.refresh_daemon_status();
        self.push_system_message(&format_daemon_status_for_chat(&status));
    }

    pub(super) fn poll_session_events(&mut self) {
        let Some(session_id) = self.active_session_id.clone() else {
            return;
        };
        let now = Instant::now();
        if self
            .event_poll_backoff_until
            .is_some_and(|until| until > now)
        {
            return;
        }
        if self.event_poll_paused_for_api_mismatch {
            return;
        }
        match self.client.list_events(ChatEventQuery {
            session_id: &session_id,
            after_seq: self.last_event_seq,
            limit: Some(200),
        }) {
            Ok(events) => {
                self.reset_event_poll_failures();
                for event in events {
                    // If `push_event_message` swapped the active session
                    // mid-batch (e.g. stale-id recovery auto-relaunched
                    // into a new session and reset `last_event_seq` to
                    // None), stop processing this batch. Continuing
                    // would advance `last_event_seq` to one of the OLD
                    // session's seqs, causing the next poll for the NEW
                    // session to query with a cursor that filters out
                    // the new session's own events.
                    if self.active_session_id.as_deref() != Some(session_id.as_str()) {
                        break;
                    }
                    self.last_event_seq = Some(event.seq);
                    self.push_event_message(&event);
                }
            }
            Err(error) => self.record_event_poll_failure(error),
        }
    }

    fn reset_event_poll_failures(&mut self) {
        self.event_poll_backoff_until = None;
        self.event_poll_failure_streak = 0;
        self.last_event_poll_error = None;
        self.event_poll_paused_for_api_mismatch = false;
    }

    fn record_event_poll_failure(&mut self, error: anyhow::Error) {
        let message = error.to_string();
        if is_api_mismatch_error(&message) {
            self.event_poll_paused_for_api_mismatch = true;
        }
        let repeated_error = self.last_event_poll_error.as_deref() == Some(message.as_str());
        self.event_poll_failure_streak = self.event_poll_failure_streak.saturating_add(1);
        self.event_poll_backoff_until =
            Some(Instant::now() + event_poll_backoff(self.event_poll_failure_streak));
        self.last_event_poll_error = Some(message.clone());
        if !repeated_error {
            let message = if self.event_poll_paused_for_api_mismatch {
                format!("Event follow failed: {message}. polling paused until next input.")
            } else {
                format!("Event follow failed: {message}")
            };
            self.push_system_message(&message);
        }
    }

    /// Codex auto-assigns a session id on its first turn and prints it in
    /// the run header (`session id: <uuid>`). When this chat owns a running
    /// codex session and we haven't captured its id yet, scan the chunk for
    /// the banner so the *next* turn can `codex exec resume <id> <prompt>`.
    fn maybe_capture_codex_session_id(&mut self, data: &str) {
        if !self.chat_owns_active_session {
            return;
        }
        if self.active_session_harness.as_deref() != Some("codex") {
            return;
        }
        if self.harness_conversation_ids.contains_key("codex") {
            return;
        }
        if let Some(id) = extract_codex_session_id(data) {
            self.harness_conversation_ids
                .insert("codex".to_string(), id);
            self.persist_conversations();
        }
    }

    /// If the harness rejected our `Resume` because the prior session no
    /// longer exists (claude or codex local store wiped, server-side
    /// expiry, etc.), drop the stale id from memory and disk and either
    /// auto-resend the original prompt (preferred) or tell the user to
    /// retype if we've already auto-retried this turn. Only fires for
    /// chat-owned sessions where we actually had a stored id to send.
    fn maybe_clear_stale_conversation_id(&mut self, data: &str) {
        if !self.chat_owns_active_session {
            return;
        }
        let Some(harness) = self.active_session_harness.clone() else {
            return;
        };
        if !self.harness_conversation_ids.contains_key(&harness) {
            return;
        }
        if !detect_stale_session(&harness, data) {
            return;
        }
        self.harness_conversation_ids.remove(&harness);
        // The dying stream session (if any) can't be reused: claude rejected
        // its --resume id and is about to exit. Drop it (and its JSON
        // line buffer) so the auto-retry cold-starts a fresh stream
        // process instead of forwarding to a half-dead pipe. The
        // eventual exit event will be ignored thanks to the suppression
        // entry below.
        if let Some(stale_stream_id) = self.harness_stream_session_ids.remove(&harness) {
            self.stream_json_buffers.remove(&stale_stream_id);
        }
        self.persist_conversations();
        // Hide any further output and the eventual exit event for the
        // failed session so the user only sees the system message + the
        // retry's reply.
        if let Some(failed_session_id) = self.active_session_id.clone() {
            self.suppressed_session_ids.insert(failed_session_id);
        }

        // Try to auto-resend so the user doesn't have to retype. Skip if
        // we've already retried this turn (defense against a retry that
        // itself trips the stale phrase — natural flow won't, since a
        // post-drop turn sends no Resume, but be defensive anyway).
        let prompt = self
            .last_chat_prompt
            .clone()
            .filter(|_| !self.auto_retry_consumed);
        match prompt {
            Some(prompt) => {
                self.push_system_message(&format!(
                    "Prior {harness} conversation no longer exists. Starting a new one and re-sending your message."
                ));
                self.auto_retry_consumed = true;
                self.run_harness_prompt(&harness, &prompt);
            }
            None => {
                // No auto-retry: clear the active-session state now so
                // the user's next message isn't gated as "still
                // streaming". Without this, the failed session's
                // events stay suppressed (so exit/kill won't reach
                // the normal state-reset arms in push_event_message),
                // and the chat wedges with `is_responding == true`
                // forever.
                self.is_responding = false;
                self.active_session_id = None;
                self.active_session_harness = None;
                self.chat_owns_active_session = false;
                self.push_system_message(&format!(
                    "Prior {harness} conversation no longer exists. Send your message again to start a fresh one."
                ));
            }
        }
    }

    /// Parse a chunk of stream-mode harness output (newline-delimited JSON)
    /// and turn it into chat-visible messages. Each line is one JSON event:
    /// `assistant.message.content[].text` becomes an agent message; the
    /// `result` event marks the turn complete and clears `is_responding`;
    /// other event types (system init, rate_limit_event, …) are ignored
    /// for now. Malformed lines are silently dropped — stream-mode is too
    /// noisy to surface every parse error.
    fn dispatch_stream_json_output(&mut self, session_id: &str, data: &str) {
        let sender = self.active_agent_label().to_string();
        // Daemon output events come from raw 8KiB reads, so a JSON line
        // can be split across two events. Buffer the trailing partial
        // line and prepend it to the next chunk so we only try to parse
        // complete newline-terminated lines.
        let buffer = self
            .stream_json_buffers
            .entry(session_id.to_string())
            .or_default();
        buffer.push_str(data);
        let (complete, remainder) = match buffer.rfind('\n') {
            Some(idx) => (buffer[..=idx].to_string(), buffer[idx + 1..].to_string()),
            None => (String::new(), std::mem::take(buffer)),
        };
        *buffer = remainder;

        for line in complete.split('\n') {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value): Result<serde_json::Value, _> = serde_json::from_str(trimmed) else {
                continue;
            };
            let Some(kind) = value.get("type").and_then(serde_json::Value::as_str) else {
                continue;
            };
            match kind {
                "assistant" => {
                    let Some(content) = value
                        .get("message")
                        .and_then(|m| m.get("content"))
                        .and_then(serde_json::Value::as_array)
                    else {
                        continue;
                    };
                    let mut chunk = String::new();
                    for block in content {
                        if block.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                            if let Some(text) =
                                block.get("text").and_then(serde_json::Value::as_str)
                            {
                                chunk.push_str(text);
                            }
                        }
                    }
                    if !chunk.is_empty() {
                        if self.streaming_mode.is_live() {
                            self.push_or_append_agent_message(&sender, &chunk);
                        } else {
                            self.buffer_pending_agent_output(&sender, &chunk);
                        }
                    }
                }
                "result" => {
                    self.flush_pending_agent_buffer();
                    self.is_responding = false;
                }
                "system" => {
                    // Daemon wraps stream-mode child stderr in
                    // {"type":"system","subtype":"stderr","text":...} so
                    // chat surfaces auth/setup errors instead of dropping
                    // them. Other system subtypes (init, etc.) stay silent.
                    //
                    // The stderr text comes from a subprocess we don't
                    // control — it can contain ANSI escapes or other
                    // control codes that would corrupt the TUI render.
                    // Run it through `clean_terminal_output` to strip
                    // those before pushing to the transcript. We also
                    // pipe stderr text through `maybe_clear_stale_conversation_id`
                    // here (instead of the broad chunk-level check in
                    // `push_event_message`) so stale-id auto-retry
                    // never fires off assistant prose that happens to
                    // quote the error phrase.
                    let subtype = value
                        .get("subtype")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    if subtype == "stderr" {
                        if let Some(text) = value.get("text").and_then(serde_json::Value::as_str) {
                            self.maybe_clear_stale_conversation_id(text);
                            // If the stale handler just suppressed this
                            // session, skip rendering the raw stderr
                            // line — the auto-retry's "Prior X
                            // conversation no longer exists. Starting a
                            // new one and re-sending your message."
                            // system message tells the user what they
                            // need to know, and the raw harness error
                            // would just be noise after it.
                            if self.suppressed_session_ids.contains(session_id) {
                                continue;
                            }
                            if let Some(safe) = clean_terminal_output(text) {
                                let trimmed = safe.trim_end_matches('\n');
                                if !trimmed.is_empty() {
                                    self.push_system_message(&format!(
                                        "[{sender} stderr] {trimmed}"
                                    ));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn push_event_message(&mut self, event: &store::EventRecord) {
        // Drop events from sessions we've decided to hide (today: failed
        // sessions whose stale-id we already auto-recovered from). Clear
        // the entry once the session has reached a terminal event so the
        // set doesn't grow over the chat lifetime.
        if self.suppressed_session_ids.contains(&event.session_id) {
            if matches!(event.kind.as_str(), "exit" | "kill") {
                self.suppressed_session_ids.remove(&event.session_id);
            }
            return;
        }
        match event.kind.as_str() {
            "output" => {
                if let Some(data) = event_payload_text(event, "data") {
                    self.maybe_capture_codex_session_id(&data);
                    // Stale-id detection is scoped per-mode to avoid
                    // false-positive auto-retries on assistant text
                    // that happens to quote the error phrase. Stream
                    // mode runs stale checks ONLY against the stderr
                    // text inside `{"type":"system","subtype":"stderr"}`
                    // envelopes (see `dispatch_stream_json_output`); we
                    // skip the broad text match here. PTY mode (NonInteractive
                    // codex / fallback claude) doesn't have a JSON
                    // structure to lean on, so we still run the broad
                    // match on the raw chunk.
                    let is_stream = self
                        .harness_stream_session_ids
                        .values()
                        .any(|id| id == &event.session_id);
                    if !is_stream {
                        self.maybe_clear_stale_conversation_id(&data);
                    }
                    // The stale handler may have just suppressed this very
                    // session; if so, skip displaying this chunk too.
                    if self.suppressed_session_ids.contains(&event.session_id) {
                        return;
                    }
                    if is_stream {
                        // Stream-mode output is newline-delimited JSON.
                        // dispatch_stream_json_output extracts assistant
                        // text from envelopes AND scopes stale detection
                        // to system/stderr text only.
                        self.dispatch_stream_json_output(&event.session_id, &data);
                        return;
                    }
                    let sender = self.active_agent_label().to_string();
                    if let Some(text) =
                        human_facing_agent_output(&data, &mut self.agent_output_mode)
                    {
                        if self.streaming_mode.is_live() {
                            self.push_or_append_agent_message(&sender, &text);
                        } else {
                            self.buffer_pending_agent_output(&sender, &text);
                        }
                    }
                }
            }
            "exit" => {
                self.flush_pending_agent_buffer();
                let status =
                    event_payload_text(event, "status").unwrap_or_else(|| "exited".to_string());
                self.is_responding = false;
                // Resolve the delegation call status from the session exit status.
                if let (Some(call_id), Some(home)) =
                    (self.active_call_id.take(), self.coven_home.as_deref())
                {
                    let call_status = if status == "0" || status == "success" {
                        crate::coven_calls::CovenCallStatus::Completed
                    } else {
                        crate::coven_calls::CovenCallStatus::Failed
                    };
                    let ended_at =
                        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                    if let Err(_err) = crate::coven_calls::emit_terminal(
                        home,
                        &call_id,
                        call_status,
                        &ended_at,
                        None,
                    ) {
                        // Non-fatal.
                    }
                }
                if self.active_session_id.as_deref() == Some(event.session_id.as_str()) {
                    self.active_session_id = None;
                    self.active_session_harness = None;
                    self.chat_owns_active_session = false;
                }
                // If a stream session for any harness died, drop its id so
                // the next turn cold-starts a fresh one instead of
                // forwarding to a dead pipe. Also drop its JSON buffer
                // (partial lines from before the exit are stale now).
                self.harness_stream_session_ids
                    .retain(|_, id| id != &event.session_id);
                self.stream_json_buffers.remove(&event.session_id);
                self.agent_output_mode = AgentOutputMode::Unknown;
                self.push_system_message(&format!("Session {status}."));
            }
            "kill" => {
                self.flush_pending_agent_buffer();
                // Delegation call was cancelled by a kill event.
                if let (Some(call_id), Some(home)) =
                    (self.active_call_id.take(), self.coven_home.as_deref())
                {
                    let ended_at =
                        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                    if let Err(_err) = crate::coven_calls::emit_terminal(
                        home,
                        &call_id,
                        crate::coven_calls::CovenCallStatus::Cancelled,
                        &ended_at,
                        None,
                    ) {
                        // Non-fatal.
                    }
                }
                if self.active_session_id.as_deref() == Some(event.session_id.as_str()) {
                    self.active_session_id = None;
                    self.active_session_harness = None;
                    self.chat_owns_active_session = false;
                    self.is_responding = false;
                }
                self.harness_stream_session_ids
                    .retain(|_, id| id != &event.session_id);
                self.stream_json_buffers.remove(&event.session_id);
                self.agent_output_mode = AgentOutputMode::Unknown;
                self.push_system_message("Session kill recorded.");
            }
            _ => {}
        }
    }

    pub(super) fn switch_agent_by_name(&mut self, name: &str) {
        let name_lower = name.to_lowercase();
        if let Some(idx) = self
            .agents
            .iter()
            .position(|a| a.id.to_lowercase() == name_lower || a.label.to_lowercase() == name_lower)
        {
            let agent = &self.agents[idx];
            if agent.available {
                self.active_agent = Some(idx);
                self.push_system_message(&format!(
                    "Switched to {} ({})",
                    agent.label, agent.harness
                ));
            } else {
                self.push_system_message(&format!(
                    "{} is not available. Run `coven doctor` to troubleshoot.",
                    agent.label
                ));
            }
        } else {
            let available: Vec<&str> = self.agents.iter().map(|a| a.id.as_str()).collect();
            self.push_system_message(&format!(
                "Unknown agent \"{name}\". Available: {}",
                available.join(", ")
            ));
        }
    }

    pub(super) fn switch_agent_by_index(&mut self, idx: usize) {
        if let Some(agent) = self.agents.get(idx) {
            if agent.available {
                self.active_agent = Some(idx);
                self.push_system_message(&format!(
                    "Switched to {} ({})",
                    agent.label, agent.harness
                ));
            } else {
                self.push_system_message(&format!(
                    "{} is not available. Run `coven doctor` to troubleshoot.",
                    agent.label
                ));
            }
        }
        self.input_mode = InputMode::Normal;
    }

    fn export_chat(&mut self) {
        if self.messages.is_empty() {
            self.push_system_message("Nothing to export.");
            return;
        }

        let home = dirs_next::home_dir().unwrap_or_default();
        let export_dir = home.join(".coven").join("exports");
        if std::fs::create_dir_all(&export_dir).is_err() {
            self.push_system_message("Failed to create export directory.");
            return;
        }

        let filename = format!("chat-{}.md", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
        let path = export_dir.join(&filename);

        let mut content = String::from("# Coven Chat Export\n\n");
        for msg in &self.messages {
            let role_label = match msg.role {
                MessageRole::User => "**You**",
                MessageRole::Agent => &format!("**{}**", msg.sender),
                MessageRole::System => "*system*",
            };
            content.push_str(&format!(
                "{} ({})\n{}\n\n---\n\n",
                role_label, msg.timestamp, msg.content
            ));
        }

        match std::fs::write(&path, content) {
            Ok(()) => self.push_system_message(&format!("Exported to {}", path.display())),
            Err(e) => self.push_system_message(&format!("Export failed: {e}")),
        }
    }

    pub(super) fn scroll_to_bottom(&mut self) {
        // Will be calculated during render based on content height
        self.scroll_offset = usize::MAX;
    }

    pub(super) fn tick(&mut self) {
        if self.last_tick.elapsed() >= Duration::from_millis(120) {
            self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
            self.last_tick = Instant::now();
            self.poll_session_events();
        }
    }

    pub(super) fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
        self.history_index = None;
        self.reset_slash_popup_state_on_edit();
    }

    pub(super) fn insert_str(&mut self, value: &str) {
        self.input.insert_str(self.cursor_pos, value);
        self.cursor_pos += value.len();
        self.history_index = None;
        self.reset_slash_popup_state_on_edit();
    }

    pub(super) fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub(super) fn delete_char_before_cursor(&mut self) {
        if self.cursor_pos > 0 {
            let prev = self.input[..self.cursor_pos]
                .chars()
                .last()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor_pos -= prev;
            self.input.remove(self.cursor_pos);
            self.reset_slash_popup_state_on_edit();
        }
    }

    pub(super) fn delete_char_at_cursor(&mut self) {
        if self.cursor_pos < self.input.len() {
            self.input.remove(self.cursor_pos);
            self.reset_slash_popup_state_on_edit();
        }
    }

    pub(super) fn move_cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            let prev = self.input[..self.cursor_pos]
                .chars()
                .last()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor_pos -= prev;
        }
    }

    pub(super) fn move_cursor_right(&mut self) {
        if self.cursor_pos < self.input.len() {
            let next = self.input[self.cursor_pos..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor_pos += next;
        }
    }

    pub(super) fn move_cursor_home(&mut self) {
        self.cursor_pos = 0;
    }

    pub(super) fn move_cursor_end(&mut self) {
        self.cursor_pos = self.input.len();
    }

    pub(super) fn delete_word_before_cursor(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        let before = &self.input[..self.cursor_pos];
        let trimmed = before.trim_end();
        let new_end = trimmed
            .rfind(char::is_whitespace)
            .map(|i| i + 1)
            .unwrap_or(0);
        self.input.drain(new_end..self.cursor_pos);
        self.cursor_pos = new_end;
        self.reset_slash_popup_state_on_edit();
    }

    pub(super) fn slash_suggestions(&self) -> Vec<&'static SlashCommand> {
        if self.slash_popup_dismissed {
            return Vec::new();
        }
        let raw = self.input.as_str();
        if !raw.starts_with('/') {
            return Vec::new();
        }
        // Once an argument starts (whitespace anywhere), the popup steps out
        // of the way so the user can type freely. Newlines count too — they
        // appear in multi-line input bodies.
        if raw.chars().any(char::is_whitespace) {
            return Vec::new();
        }
        let prefix = raw.to_ascii_lowercase();
        SLASH_COMMANDS
            .iter()
            .filter(|cmd| cmd.name.starts_with(prefix.as_str()))
            .collect()
    }

    pub(super) fn slash_popup_is_open(&self) -> bool {
        !self.slash_suggestions().is_empty()
    }

    pub(super) fn slash_popup_select_next(&mut self) {
        let len = self.slash_suggestions().len();
        if len <= 1 {
            return;
        }
        self.slash_suggestion_index = (self.slash_suggestion_index + 1) % len;
    }

    pub(super) fn slash_popup_select_prev(&mut self) {
        let len = self.slash_suggestions().len();
        if len <= 1 {
            return;
        }
        self.slash_suggestion_index = if self.slash_suggestion_index == 0 {
            len - 1
        } else {
            self.slash_suggestion_index - 1
        };
    }

    /// Replace the current input with the selected suggestion and a trailing
    /// space so the user can immediately start typing an argument. Returns
    /// true if a completion happened.
    pub(super) fn apply_slash_suggestion(&mut self) -> bool {
        let suggestions = self.slash_suggestions();
        if suggestions.is_empty() {
            return false;
        }
        let idx = self.slash_suggestion_index.min(suggestions.len() - 1);
        let pick = suggestions[idx];
        // If the input already exactly matches the selection (modulo case),
        // there's nothing to complete — let the caller fall through so the
        // command actually runs on Enter.
        if self.input.eq_ignore_ascii_case(pick.name) {
            return false;
        }
        self.input.clear();
        self.input.push_str(pick.name);
        self.input.push(' ');
        self.cursor_pos = self.input.len();
        self.slash_suggestion_index = 0;
        true
    }

    pub(super) fn dismiss_slash_popup(&mut self) {
        self.slash_popup_dismissed = true;
    }

    fn reset_slash_popup_state_on_edit(&mut self) {
        self.slash_suggestion_index = 0;
        self.slash_popup_dismissed = false;
    }

    pub(super) fn history_previous(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        let next_index = self
            .history_index
            .map(|index| index.saturating_sub(1))
            .unwrap_or_else(|| self.input_history.len().saturating_sub(1));
        self.history_index = Some(next_index);
        self.input = self.input_history[next_index].clone();
        self.cursor_pos = self.input.len();
    }

    pub(super) fn history_next(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 >= self.input_history.len() {
            self.history_index = None;
            self.input.clear();
        } else {
            let next_index = index + 1;
            self.history_index = Some(next_index);
            self.input = self.input_history[next_index].clone();
        }
        self.cursor_pos = self.input.len();
    }

    fn record_history(&mut self, raw: &str) {
        if self.input_history.last().map(|entry| entry.as_str()) != Some(raw) {
            self.input_history.push(raw.to_string());
        }
        self.history_index = None;
    }
}

/// Applies a capped exponential backoff so repeated event-poll failures do not
/// flood the transcript or hammer the daemon when it is unavailable.
fn event_poll_backoff(streak: u32) -> Duration {
    let millis = match streak {
        0 | 1 => 500,
        2 => 1_000,
        3 => 2_000,
        4 => 4_000,
        _ => 5_000,
    };
    Duration::from_millis(millis)
}

fn is_api_mismatch_error(message: &str) -> bool {
    message.contains("Coven daemon API mismatch")
}

// ── Discover agents from built-in harnesses ────────────────────────────────

pub(super) fn discover_agents() -> Vec<AgentInfo> {
    harness::built_in_harnesses()
        .into_iter()
        .map(|h| AgentInfo {
            id: h.id.to_string(),
            label: h.label.to_string(),
            harness: h.id.to_string(),
            available: h.available,
        })
        .collect()
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn timestamp_now() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

fn current_project_label() -> String {
    std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "unknown project".to_string())
}

fn split_first_arg(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim();
    let split_idx = trimmed.find(char::is_whitespace)?;
    let first = &trimmed[..split_idx];
    let rest = trimmed[split_idx..].trim();
    (!first.is_empty() && !rest.is_empty()).then_some((first, rest))
}

fn is_chat_local_slash(input: &str) -> bool {
    let command = input
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        command.as_str(),
        "/help"
            | "/h"
            | "/commands"
            | "/palette"
            | "/clear"
            | "/cls"
            | "/new"
            | "/agent"
            | "/a"
            | "/export"
            | "/exit"
            | "/quit"
            | "/q"
            | "/stream"
            | "/streaming"
    )
}

fn short_session_id(session_id: &str) -> String {
    const SHORT_ID_LEN: usize = 8;
    session_id.chars().take(SHORT_ID_LEN).collect()
}

fn should_keep_launch_inline(plan: &CastPlan) -> bool {
    !matches!(plan.intent, CastIntent::NaturalSpell { .. })
        || !matches!(plan.risk(), CastRisk::Safe)
}

/// Whether a chat turn launched against this harness should reuse the prior
/// turn's conversation via the harness CLI's session-resume mechanism. See
/// `docs/chat-persistence.md` for the per-harness mechanics.
fn harness_supports_chat_resume(harness: &str) -> bool {
    matches!(harness, "claude" | "codex")
}

/// Whether `data` (a chunk of harness output) indicates the harness rejected
/// our `Resume` because the session id it carried no longer exists. Both
/// claude and codex unhelpfully exit with code 0 in this case, so we have to
/// pattern-match on their distinctive error wording. See
/// `docs/chat-persistence.md` under "stale-id auto-recovery".
///
/// The match is a broad `contains` because callers scope the input
/// before passing it in. For Stream mode `push_event_message` skips the
/// broad check and `dispatch_stream_json_output` calls this only with
/// the unwrapped `text` of `system/stderr` envelopes, so assistant
/// prose can never trip it. For PTY mode (NonInteractive codex / fallback
/// claude) we still match the whole stdout chunk because there's no
/// JSON structure to lean on; the realistic risk there is a turn-1
/// codex error message that quotes the phrase, which is acceptable
/// given codex's stale error is also turn-1-only.
fn detect_stale_session(harness: &str, data: &str) -> bool {
    match harness {
        "claude" => data.contains("No conversation found with session ID"),
        "codex" => {
            data.contains("no rollout found for thread id") || data.contains("thread/resume failed")
        }
        _ => false,
    }
}

/// Scan `data` (a chunk of cleaned-but-not-line-filtered harness output) for a
/// codex session-id banner line and return the uuid if present. Codex prints
/// `session id: <uuid>` in the header of every `codex exec` run; we capture
/// it so the next chat turn can `codex exec resume <id> <prompt>`.
fn extract_codex_session_id(data: &str) -> Option<String> {
    const PREFIX: &str = "session id:";
    for line in data.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix(PREFIX) else {
            continue;
        };
        let id = rest.trim();
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    None
}

fn format_cast_plan_for_chat(plan: &CastPlan) -> String {
    let harness = plan
        .harness
        .map(|plan_harness| {
            let source = match plan_harness.source {
                CastHarnessSource::UserChose => "user-chosen",
                CastHarnessSource::SafeDefault => "Cast default",
            };
            format!("harness {} · {source}", plan_harness.harness.label())
        })
        .unwrap_or_else(|| "harness none".to_string());
    let risk = match plan.risk() {
        CastRisk::Safe => "[ SAFE ]",
        CastRisk::Confirm => "[ CONFIRM ]",
        CastRisk::Reject => "[ REJECT ]",
    };
    let steps = if plan
        .steps
        .iter()
        .any(|step| step.kind == crate::tui::cast::plan::CastStepKind::LaunchSession)
    {
        "launch project-scoped session".to_string()
    } else {
        plan.steps
            .first()
            .map(|step| step.note.clone())
            .unwrap_or_else(|| "no side effects".to_string())
    };

    let session = plan
        .session_id
        .as_deref()
        .map(|session_id| format!("\n  session  {session_id}"))
        .unwrap_or_default();

    format!("Cast plan\n  {harness}  risk {risk}{session}\n  steps  {steps}")
}

fn format_cast_outcome_for_chat(harness_label: &str, session_id: &str) -> String {
    format!("Cast outcome\n  launched  {harness_label} daemon session\n  session  {session_id}")
}

fn format_daemon_status_for_chat(status: &ChatDaemonStatus) -> String {
    match status {
        ChatDaemonStatus::Running { pid } => {
            format!("Daemon status\n  status  running\n  pid     {pid}")
        }
        ChatDaemonStatus::Stale { pid } => {
            format!("Daemon status\n  status  stale\n  pid     {pid}\n  next    coven daemon restart")
        }
        ChatDaemonStatus::Stopped => {
            "Daemon status\n  status  stopped\n  next    coven daemon start".to_string()
        }
        ChatDaemonStatus::ApiMismatch { expected, actual } => format!(
            "Daemon status\n  status  mismatch\n  expect  {expected}\n  actual  {actual}\n  next    coven daemon restart"
        ),
        ChatDaemonStatus::Unavailable { message } => format!(
            "Daemon status\n  status  unavailable\n  error   {message}\n  next    coven daemon restart"
        ),
    }
}

fn event_payload_text(event: &store::EventRecord, field: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(&event.payload_json)
        .ok()?
        .get(field)?
        .as_str()
        .map(ToOwned::to_owned)
}

fn clean_terminal_output(data: &str) -> Option<String> {
    let mut output = String::new();
    let mut chars = data.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\x1b' => skip_escape_sequence(&mut chars),
            '\r' => {}
            '\n' | '\t' => output.push(ch),
            '\x08' => {
                output.pop();
            }
            ch if ch.is_control() => {}
            ch => output.push(ch),
        }
    }

    // Newlines carry paragraph-break structure even when nothing visible
    // surrounds them, so keep any chunk that has a newline OR any
    // non-whitespace char. Drop only space/tab-only or fully empty chunks —
    // those are pure control noise after escape sequences are stripped.
    let has_structure = output.chars().any(|ch| ch == '\n' || !ch.is_whitespace());
    has_structure.then_some(output)
}

fn human_facing_agent_output(data: &str, mode: &mut AgentOutputMode) -> Option<String> {
    let cleaned = clean_terminal_output(data)?;
    let mut visible = String::new();

    for raw_line in cleaned.split_inclusive('\n') {
        let line = raw_line.trim_end_matches('\n');
        let marker = line.trim();

        if is_assistant_marker(marker) {
            *mode = AgentOutputMode::Assistant;
            continue;
        }
        if is_hidden_transcript_marker(marker) || is_codex_metadata_line(marker) {
            *mode = AgentOutputMode::Hidden;
            continue;
        }

        match mode {
            AgentOutputMode::Assistant | AgentOutputMode::Unknown => visible.push_str(raw_line),
            AgentOutputMode::Hidden => {}
        }
    }

    let has_structure = visible.chars().any(|ch| ch == '\n' || !ch.is_whitespace());
    has_structure.then_some(visible)
}

fn is_assistant_marker(line: &str) -> bool {
    matches!(line, "codex" | "assistant")
}

fn is_hidden_transcript_marker(line: &str) -> bool {
    if matches!(line, "user" | "exec" | "tool" | "bash" | "shell" | "system") {
        return true;
    }
    line.starts_with("hook:")
        || line == "tokens used"
        || line == "Completed"
        || line.starts_with("succeeded in ")
        || line.starts_with("failed in ")
}

fn is_codex_metadata_line(line: &str) -> bool {
    line.starts_with("OpenAI Codex v")
        || line == "--------"
        || line.starts_with("workdir:")
        || line.starts_with("model:")
        || line.starts_with("provider:")
        || line.starts_with("approval:")
        || line.starts_with("sandbox:")
        || line.starts_with("reasoning effort:")
        || line.starts_with("reasoning summaries:")
        || line.starts_with("session id:")
}

fn skip_escape_sequence<I>(chars: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = char>,
{
    let Some(introducer) = chars.next() else {
        return;
    };
    match introducer {
        '[' => skip_csi_sequence(chars),
        ']' => skip_until_string_terminator(chars),
        'P' | '^' | '_' | 'X' => skip_until_string_terminator(chars),
        _ => {}
    }
}

fn skip_csi_sequence<I>(chars: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = char>,
{
    for ch in chars.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&ch) {
            break;
        }
    }
}

fn skip_until_string_terminator<I>(chars: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = char>,
{
    while let Some(ch) = chars.next() {
        if ch == '\x07' {
            break;
        }
        if ch == '\x1b' && chars.peek() == Some(&'\\') {
            chars.next();
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{EventRecord, SessionRecord};
    use crate::tui::chat::client::{ChatClient, ChatDaemonStatus, ChatEventQuery, LaunchRequest};
    use crate::tui::chat::persistence;
    use std::cell::RefCell;
    use std::path::Path;
    use std::rc::Rc;

    fn app_with_agents(agents: Vec<AgentInfo>) -> App {
        let active_agent = agents.iter().position(|agent| agent.available);
        App::new_with_state(
            agents,
            active_agent,
            Box::new(RecordingChatClient::default()),
            None,
        )
    }

    fn agent(id: &str, available: bool) -> AgentInfo {
        AgentInfo {
            id: id.to_string(),
            label: id.to_string(),
            harness: id.to_string(),
            available,
        }
    }

    #[derive(Clone, Default)]
    struct RecordingChatClient {
        calls: Rc<RefCell<Vec<String>>>,
        launched: Rc<RefCell<Vec<LaunchRequest>>>,
        sessions: Rc<RefCell<Vec<SessionRecord>>>,
        events: Rc<RefCell<Vec<EventRecord>>>,
        daemon_status: Rc<RefCell<ChatDaemonStatus>>,
        event_error: Rc<RefCell<Option<String>>>,
        launch_error: Rc<RefCell<Option<String>>>,
        send_input_error: Rc<RefCell<Option<String>>>,
    }

    impl RecordingChatClient {
        fn with_session(session: SessionRecord) -> Self {
            let client = Self::default();
            client.sessions.borrow_mut().push(session);
            client
        }
    }

    impl ChatClient for RecordingChatClient {
        fn daemon_status(&mut self) -> anyhow::Result<ChatDaemonStatus> {
            self.calls.borrow_mut().push("daemon-status".to_string());
            Ok(self.daemon_status.borrow().clone())
        }

        fn launch_session(&mut self, request: LaunchRequest) -> anyhow::Result<SessionRecord> {
            self.calls.borrow_mut().push("launch".to_string());
            self.launched.borrow_mut().push(request.clone());
            if let Some(error) = self.launch_error.borrow().clone() {
                return Err(anyhow::anyhow!(error));
            }
            let session = test_session(&request.id, &request.harness, &request.prompt, "running");
            self.sessions.borrow_mut().push(session.clone());
            Ok(session)
        }

        fn get_session(&mut self, session_id: &str) -> anyhow::Result<SessionRecord> {
            self.calls.borrow_mut().push(format!("get:{session_id}"));
            self.sessions
                .borrow()
                .iter()
                .find(|session| session.id == session_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("session not found"))
        }

        fn list_sessions(&mut self) -> anyhow::Result<Vec<SessionRecord>> {
            self.calls.borrow_mut().push("list".to_string());
            Ok(self.sessions.borrow().clone())
        }

        fn list_events(&mut self, query: ChatEventQuery<'_>) -> anyhow::Result<Vec<EventRecord>> {
            self.calls.borrow_mut().push(format!(
                "events:{}:{}",
                query.session_id,
                query.after_seq.unwrap_or(0)
            ));
            if let Some(error) = self.event_error.borrow().clone() {
                return Err(anyhow::anyhow!(error));
            }
            Ok(self
                .events
                .borrow()
                .iter()
                .filter(|event| event.session_id == query.session_id)
                .filter(|event| query.after_seq.map(|seq| event.seq > seq).unwrap_or(true))
                .cloned()
                .collect())
        }

        fn send_input(&mut self, session_id: &str, data: &str) -> anyhow::Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("input:{session_id}:{data}"));
            if let Some(error) = self.send_input_error.borrow().clone() {
                return Err(anyhow::anyhow!(error));
            }
            Ok(())
        }

        fn kill_session(&mut self, session_id: &str) -> anyhow::Result<()> {
            self.calls.borrow_mut().push(format!("kill:{session_id}"));
            Ok(())
        }

        fn archive_session(&mut self, session_id: &str) -> anyhow::Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("archive:{session_id}"));
            let mut sessions = self.sessions.borrow_mut();
            let session = sessions
                .iter_mut()
                .find(|session| session.id == session_id)
                .ok_or_else(|| anyhow::anyhow!("session not found"))?;
            session.archived_at = Some("2026-05-19T01:00:00Z".to_string());
            Ok(())
        }

        fn summon_session(&mut self, session_id: &str) -> anyhow::Result<SessionRecord> {
            self.calls.borrow_mut().push(format!("summon:{session_id}"));
            let mut sessions = self.sessions.borrow_mut();
            let session = sessions
                .iter_mut()
                .find(|session| session.id == session_id)
                .ok_or_else(|| anyhow::anyhow!("session not found"))?;
            session.archived_at = None;
            Ok(session.clone())
        }

        fn sacrifice_session(&mut self, session_id: &str) -> anyhow::Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("sacrifice:{session_id}"));
            let mut sessions = self.sessions.borrow_mut();
            let index = sessions
                .iter()
                .position(|session| session.id == session_id)
                .ok_or_else(|| anyhow::anyhow!("session not found"))?;
            sessions.remove(index);
            Ok(())
        }
    }

    fn app_with_client(client: RecordingChatClient) -> (App, RecordingChatClient) {
        let mirror = client.clone();
        let mut app = App::new_with_client(Box::new(client));
        app.agents = vec![agent("codex", true), agent("claude", true)];
        app.active_agent = Some(0);
        app.messages.clear();
        (app, mirror)
    }

    fn render_app_plain(app: &mut App, width: u16, height: u16) -> String {
        use ratatui::{backend::TestBackend, Terminal};

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::tui::chat::render::render_ui(frame, app))
            .unwrap();
        crate::tui::chat::render::buffer_to_plain_text(terminal.backend().buffer())
    }

    /// Like `app_with_client` but with `coven_home` + `project_root` wired
    /// so cross-restart persistence is exercised. Returns the mirror plus the
    /// two paths so tests can simulate a restart by constructing a second
    /// App that points at the same persisted store.
    fn app_with_persistence(
        client: RecordingChatClient,
        coven_home: &Path,
        project_root: &Path,
    ) -> (App, RecordingChatClient) {
        let mirror = client.clone();
        let agents = vec![agent("codex", true), agent("claude", true)];
        let mut app = App::new_with_state_and_project_root(
            agents,
            Some(0),
            Box::new(client),
            Some(coven_home.to_path_buf()),
            Some(project_root.to_path_buf()),
        );
        app.messages.clear();
        (app, mirror)
    }

    fn test_session(id: &str, harness: &str, title: &str, status: &str) -> SessionRecord {
        SessionRecord {
            id: id.to_string(),
            project_root: "/tmp/project".to_string(),
            harness: harness.to_string(),
            title: title.to_string(),
            status: status.to_string(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-05-19T00:00:00Z".to_string(),
            updated_at: "2026-05-19T00:00:00Z".to_string(),
            conversation_id: None,
            familiar_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
        }
    }

    fn output_event(seq: i64, session_id: &str, data: &str) -> EventRecord {
        EventRecord {
            seq,
            id: format!("event-{seq}"),
            session_id: session_id.to_string(),
            kind: "output".to_string(),
            payload_json: serde_json::json!({ "data": data }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        }
    }

    /// Build a stream-json `{"type":"system","subtype":"stderr","text":...}\n`
    /// envelope, the wire format the daemon emits for piped-child stderr
    /// lines. Stale-id detection in stream-mode runs ONLY against the
    /// unwrapped `text` of these envelopes (not against assistant
    /// content), so stale tests must use this helper when simulating a
    /// stream session.
    fn stale_stderr_chunk(text: &str) -> String {
        let envelope = serde_json::json!({
            "type": "system",
            "subtype": "stderr",
            "text": text,
        });
        format!("{envelope}\n")
    }

    #[test]
    fn unknown_slash_command_returns_command_name_for_feedback() {
        let mut app = app_with_agents(vec![agent("codex", true)]);

        match app.handle_slash_command("/unknown value") {
            SlashCommandResult::Unknown(command) => assert_eq!(command, "/unknown"),
            other => panic!("expected unknown command result, got {other:?}"),
        }
    }

    #[test]
    fn status_bar_uses_daemon_health_and_session_state() {
        let client = RecordingChatClient::default();
        *client.daemon_status.borrow_mut() = ChatDaemonStatus::Running { pid: 4242 };
        let (mut app, _) = app_with_client(client);
        app.active_session_id = Some("1234567890abcdef".to_string());

        let frame = render_app_plain(&mut app, 100, 10);

        assert!(
            frame.contains("daemon: running"),
            "status row should use daemon health, not active-session inference:\n{frame}"
        );
        assert!(
            frame.contains("session: 12345678"),
            "status row should show compact active session id:\n{frame}"
        );
    }

    #[test]
    fn help_and_slash_palette_hide_unimplemented_commands() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.input = "/".to_string();
        app.cursor_pos = app.input.len();

        let suggestions = app
            .slash_suggestions()
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        for command in ["/delegate", "/trace", "/mem", "/debug"] {
            assert!(
                !suggestions.contains(&command),
                "{command} should stay hidden until it performs real work"
            );
        }

        app.show_help = true;
        let frame = render_app_plain(&mut app, 90, 36);
        assert!(
            !frame.contains("coming soon"),
            "dead commands leaked:\n{frame}"
        );
        assert!(
            !frame.contains("/delegate"),
            "dead command leaked:\n{frame}"
        );
        assert!(!frame.contains("/trace"), "dead command leaked:\n{frame}");
        assert!(!frame.contains("/mem"), "dead command leaked:\n{frame}");
        assert!(!frame.contains("/debug"), "dead command leaked:\n{frame}");
    }

    #[test]
    fn doctor_command_appends_inline_harness_summary() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.input = "/doctor".to_string();

        app.handle_input();

        let transcript = app
            .messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(transcript.contains("Doctor"));
        assert!(transcript.contains("Harnesses"));
        assert!(
            !transcript.contains("Run `coven doctor`"),
            "doctor should run inline, not hand the user back to the shell:\n{transcript}"
        );
    }

    #[test]
    fn daemon_command_appends_inline_status_summary() {
        let client = RecordingChatClient::default();
        *client.daemon_status.borrow_mut() = ChatDaemonStatus::Stale { pid: 99 };
        let (mut app, mirror) = app_with_client(client);
        app.input = "/daemon".to_string();

        app.handle_input();

        let transcript = app
            .messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(mirror.calls.borrow().contains(&"daemon-status".to_string()));
        assert!(transcript.contains("Daemon status"));
        assert!(transcript.contains("stale"));
        assert!(
            !transcript.contains("Run `coven daemon status`"),
            "daemon status should render inline, not hand the user back to the shell:\n{transcript}"
        );
    }

    #[test]
    fn handle_input_clears_unknown_slash_command_and_reports_it() {
        let mut app = app_with_agents(vec![agent("codex", true)]);
        app.input = "/missing".to_string();
        app.cursor_pos = app.input.len();

        let result = app.handle_input();

        assert!(matches!(result, Some(SlashCommandResult::Handled)));
        assert!(app.input.is_empty());
        assert_eq!(app.cursor_pos, 0);
        assert!(app.messages.iter().any(|message| message
            .content
            .contains("unknown Cast slash command `/missing`")
            && message.content.contains("/help")));
    }

    #[test]
    fn agent_command_without_argument_opens_picker_on_active_agent() {
        let mut app = app_with_agents(vec![agent("claude", false), agent("codex", true)]);

        let result = app.handle_slash_command("/agent");

        assert!(matches!(result, SlashCommandResult::Handled));
        assert_eq!(app.input_mode, InputMode::AgentSelect);
        assert_eq!(app.agent_select_index, 1);
    }

    #[test]
    fn unavailable_agent_selection_keeps_current_active_agent() {
        let mut app = app_with_agents(vec![agent("claude", false), agent("codex", true)]);

        app.switch_agent_by_name("claude");

        assert_eq!(app.active_agent, Some(1));
        assert!(app
            .messages
            .last()
            .map(|message| message.content.contains("claude is not available"))
            .unwrap_or(false));
    }

    #[test]
    fn first_claude_chat_turn_attaches_init_conversation_hint() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "hello".to_string();
        app.cursor_pos = app.input.len();

        app.handle_input();

        let launched = mirror.launched.borrow();
        assert_eq!(launched.len(), 1);
        assert_eq!(launched[0].harness, "claude");
        match &launched[0].conversation {
            Some(crate::harness::ConversationHint::Init { id }) => {
                assert!(!id.is_empty(), "Init id must be a non-empty uuid");
            }
            other => panic!("first turn should carry Init hint, got {other:?}"),
        }
    }

    #[test]
    fn second_claude_chat_turn_reuses_init_id_as_resume() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "first".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let first_session_id = app
            .active_session_id()
            .expect("first launch sets id")
            .to_string();
        let init_id = match &mirror.launched.borrow()[0].conversation {
            Some(crate::harness::ConversationHint::Init { id }) => id.clone(),
            other => panic!("first turn should be Init, got {other:?}"),
        };

        // Simulate harness exit so the next turn isn't gated by is_responding.
        app.push_event_message(&EventRecord {
            seq: 1,
            id: "event-1".to_string(),
            session_id: first_session_id,
            kind: "exit".to_string(),
            payload_json: serde_json::json!({ "status": "completed" }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        });

        app.input = "second".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        let launched = mirror.launched.borrow();
        assert_eq!(launched.len(), 2);
        match &launched[1].conversation {
            Some(crate::harness::ConversationHint::Resume { id }) => {
                assert_eq!(id, &init_id, "second turn must resume the first turn's id");
            }
            other => panic!("second turn should carry Resume hint, got {other:?}"),
        }
    }

    #[test]
    fn clear_transcript_drops_conversation_ids_so_next_turn_is_init() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "first".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let session_id = app
            .active_session_id()
            .expect("first launch sets id")
            .to_string();
        app.push_event_message(&EventRecord {
            seq: 1,
            id: "event-1".to_string(),
            session_id,
            kind: "exit".to_string(),
            payload_json: serde_json::json!({ "status": "completed" }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        });

        app.clear_transcript();

        app.input = "fresh".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        let launched = mirror.launched.borrow();
        assert_eq!(launched.len(), 2);
        let init_id_1 = match &launched[0].conversation {
            Some(crate::harness::ConversationHint::Init { id }) => id.clone(),
            other => panic!("expected first Init, got {other:?}"),
        };
        match &launched[1].conversation {
            Some(crate::harness::ConversationHint::Init { id }) => {
                assert_ne!(
                    id, &init_id_1,
                    "/clear should yield a fresh conversation id"
                );
            }
            other => panic!("expected Init after /clear, got {other:?}"),
        }
    }

    #[test]
    fn claude_chat_turn_carries_conversation_id_matching_the_init_uuid() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "hello".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        let launched = mirror.launched.borrow();
        let init_id = match &launched[0].conversation {
            Some(crate::harness::ConversationHint::Init { id }) => id.clone(),
            other => panic!("expected Init, got {other:?}"),
        };
        assert_eq!(
            launched[0].conversation_id.as_deref(),
            Some(init_id.as_str()),
            "claude chat turns must carry conversation_id equal to the session uuid"
        );
    }

    #[test]
    fn codex_first_chat_turn_lands_without_conversation_id_then_subsequent_turns_carry_it() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.active_agent = Some(0); // codex
        app.input = "first".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        // First codex launch: no captured id yet, no conversation_id either.
        assert!(mirror.launched.borrow()[0].conversation_id.is_none());

        let session_id = app.active_session_id().expect("first launch").to_string();
        let captured = "019eaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        app.push_event_message(&output_event(
            1,
            &session_id,
            &format!("session id: {captured}\n"),
        ));
        app.push_event_message(&EventRecord {
            seq: 2,
            id: "event-2".to_string(),
            session_id,
            kind: "exit".to_string(),
            payload_json: serde_json::json!({ "status": "completed" }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        });

        app.input = "follow up".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        let launched = mirror.launched.borrow();
        assert_eq!(launched.len(), 2);
        assert_eq!(
            launched[1].conversation_id.as_deref(),
            Some(captured),
            "second codex turn must carry the captured id as conversation_id"
        );
    }

    #[test]
    fn codex_first_chat_turn_carries_no_hint_so_codex_can_assign_its_own_session_id() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.active_agent = Some(0); // codex
        app.input = "do a thing".to_string();
        app.cursor_pos = app.input.len();

        app.handle_input();

        let launched = mirror.launched.borrow();
        assert_eq!(launched.len(), 1);
        assert_eq!(launched[0].harness, "codex");
        assert!(
            launched[0].conversation.is_none(),
            "codex auto-assigns ids; first turn must not carry a hint"
        );
    }

    #[test]
    fn second_codex_chat_turn_resumes_using_id_captured_from_first_turn_output() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.active_agent = Some(0); // codex
        app.input = "first".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let session_id = app
            .active_session_id()
            .expect("first launch sets id")
            .to_string();

        // Simulate codex emitting its session-id banner mid-stream.
        let captured_id = "019e5998-7130-7872-8d96-a6b67c5b6406";
        app.push_event_message(&output_event(
            1,
            &session_id,
            &format!("OpenAI Codex v0.132.0\n--------\nsession id: {captured_id}\n--------\n"),
        ));
        // And then exit so we can fire the next turn without is_responding gating.
        app.push_event_message(&EventRecord {
            seq: 2,
            id: "event-2".to_string(),
            session_id,
            kind: "exit".to_string(),
            payload_json: serde_json::json!({ "status": "completed" }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        });

        app.input = "follow up".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        let launched = mirror.launched.borrow();
        assert_eq!(launched.len(), 2);
        match &launched[1].conversation {
            Some(crate::harness::ConversationHint::Resume { id }) => {
                assert_eq!(id, captured_id);
            }
            other => panic!("second codex turn must Resume with captured id, got {other:?}"),
        }
    }

    #[test]
    fn codex_session_id_capture_is_not_overridden_by_later_output() {
        let client = RecordingChatClient::default();
        let (mut app, _mirror) = app_with_client(client);
        app.active_agent = Some(0); // codex
        app.input = "first".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let session_id = app
            .active_session_id()
            .expect("first launch sets id")
            .to_string();

        let first_id = "019e5998-7130-7872-8d96-a6b67c5b6406";
        let later_id = "ffffffff-ffff-ffff-ffff-ffffffffffff";
        app.push_event_message(&output_event(
            1,
            &session_id,
            &format!("session id: {first_id}\n"),
        ));
        // Another id later in the same turn must not clobber the captured one.
        app.push_event_message(&output_event(
            2,
            &session_id,
            &format!("session id: {later_id}\n"),
        ));

        assert_eq!(
            app.harness_conversation_ids
                .get("codex")
                .map(String::as_str),
            Some(first_id),
            "first captured id must stick"
        );
    }

    #[test]
    fn first_claude_turn_persists_conversation_id_to_disk() {
        let coven_home = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_persistence(client, coven_home.path(), project_root.path());
        app.active_agent = Some(1); // claude
        app.input = "hello".to_string();
        app.cursor_pos = app.input.len();

        app.handle_input();

        let stored = persistence::load_for_project(coven_home.path(), project_root.path());
        let in_memory = app
            .harness_conversation_ids
            .get("claude")
            .cloned()
            .expect("first claude turn must record an id");
        assert_eq!(
            stored.get("claude").cloned(),
            Some(in_memory),
            "claude conversation id must be persisted to disk after Init"
        );
    }

    #[test]
    fn fresh_app_resumes_persisted_claude_conversation_on_first_send() {
        let coven_home = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let stored_id = "fab1efac-1234-5678-9abc-def012345678";
        let mut seed = std::collections::HashMap::new();
        seed.insert("claude".to_string(), stored_id.to_string());
        persistence::save_for_project(coven_home.path(), project_root.path(), &seed)
            .expect("seed persisted state");

        let client = RecordingChatClient::default();
        let (mut app, mirror) =
            app_with_persistence(client, coven_home.path(), project_root.path());
        assert_eq!(
            app.harness_conversation_ids
                .get("claude")
                .map(String::as_str),
            Some(stored_id),
            "App must load persisted conversation ids on startup"
        );

        app.active_agent = Some(1); // claude
        app.input = "hello again".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        let launched = mirror.launched.borrow();
        match &launched[0].conversation {
            Some(crate::harness::ConversationHint::Resume { id }) => {
                assert_eq!(
                    id, stored_id,
                    "first turn after restart must Resume with persisted id"
                );
            }
            other => panic!("expected Resume on first turn after restart, got {other:?}"),
        }
    }

    #[test]
    fn codex_session_id_capture_is_persisted_to_disk() {
        let coven_home = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_persistence(client, coven_home.path(), project_root.path());
        app.active_agent = Some(0); // codex
        app.input = "first".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let session_id = app
            .active_session_id()
            .expect("first launch sets id")
            .to_string();

        let captured_id = "019eaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        app.push_event_message(&output_event(
            1,
            &session_id,
            &format!("session id: {captured_id}\n"),
        ));

        let stored = persistence::load_for_project(coven_home.path(), project_root.path());
        assert_eq!(
            stored.get("codex").map(String::as_str),
            Some(captured_id),
            "codex session id must be persisted as soon as it's captured"
        );
    }

    #[test]
    fn first_claude_chat_turn_launches_in_stream_mode_and_tracks_the_daemon_session_id() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "hello".to_string();
        app.cursor_pos = app.input.len();

        app.handle_input();

        let launched = mirror.launched.borrow();
        assert_eq!(launched.len(), 1, "first claude turn must launch once");
        assert_eq!(
            launched[0].launch_mode,
            crate::harness::HarnessLaunchMode::Stream,
            "claude chat turns must take the stream path",
        );
        let session_id = app
            .active_session_id()
            .expect("first launch sets active session id")
            .to_string();
        assert_eq!(
            app.harness_stream_session_ids.get("claude").cloned(),
            Some(session_id),
            "first stream launch must register its daemon session id under claude"
        );
    }

    #[test]
    fn stream_send_failure_drops_tracking_so_next_turn_cold_starts() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "first".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let stream_session = app.active_session_id().expect("first launch").to_string();
        assert!(app.harness_stream_session_ids.contains_key("claude"));

        // Complete the first turn so the next message isn't gated by
        // is_responding, then arm the mock so the next send_input fails
        // (e.g. daemon NotLiveError).
        let result_chunk =
            r#"{"type":"result","subtype":"success","is_error":false}"#.to_string() + "\n";
        app.push_event_message(&output_event(1, &stream_session, &result_chunk));
        *mirror.send_input_error.borrow_mut() = Some("simulated NotLive".to_string());

        app.input = "second".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        // The send to the dead stream session failed — chat must drop
        // the tracking entry so it doesn't loop back to the same dead
        // pipe on the third message. Both the per-harness id and the
        // per-session JSON buffer should be gone, and active state
        // cleared so the user isn't gated by stale is_responding.
        assert!(
            !app.harness_stream_session_ids.contains_key("claude"),
            "send failure on stream session must drop the per-harness id so the next turn cold-starts"
        );
        assert!(!app.stream_json_buffers.contains_key(&stream_session));
        assert!(app.active_session_id().is_none());
        assert!(!app.is_responding);

        // Now disarm the mock and prove the next message launches fresh.
        *mirror.send_input_error.borrow_mut() = None;
        let launches_before_retype = mirror.launched.borrow().len();
        app.input = "third".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        assert_eq!(
            mirror.launched.borrow().len(),
            launches_before_retype + 1,
            "after the dead-stream cleanup, next message must cold-start a fresh launch"
        );
    }

    #[test]
    fn second_claude_chat_turn_reuses_the_stream_session_via_send_input_not_launch() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "first".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let stream_session_id = app.active_session_id().expect("first launch").to_string();
        assert_eq!(mirror.launched.borrow().len(), 1);

        // Stream-mode sessions don't fire an exit between turns; instead
        // each turn ends with a `result` event that clears is_responding.
        // Simulate that so the next user message isn't gated.
        let result_chunk =
            r#"{"type":"result","subtype":"success","is_error":false}"#.to_string() + "\n";
        app.push_event_message(&output_event(1, &stream_session_id, &result_chunk));
        assert!(!app.is_responding);

        app.input = "second".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        assert_eq!(
            mirror.launched.borrow().len(),
            1,
            "second turn must NOT cold-start a new launch when a stream session exists"
        );
        assert!(
            mirror
                .calls
                .borrow()
                .iter()
                .any(|call| call == &format!("input:{stream_session_id}:second")),
            "second turn must forward via send_input to the existing stream session WITHOUT a trailing newline (the daemon wraps payload verbatim in a JSON envelope; a literal \\n would leak into the user message text)"
        );
    }

    #[test]
    fn codex_chat_turn_does_not_take_the_stream_path() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.active_agent = Some(0); // codex
        app.input = "do a thing".to_string();
        app.cursor_pos = app.input.len();

        app.handle_input();

        let launched = mirror.launched.borrow();
        assert_eq!(launched.len(), 1);
        assert_eq!(
            launched[0].launch_mode,
            crate::harness::HarnessLaunchMode::NonInteractive,
            "codex doesn't support stream mode; must fall back to non-interactive"
        );
        assert!(
            app.harness_stream_session_ids.is_empty(),
            "codex turns must not register a stream session id"
        );
    }

    #[test]
    fn stream_json_assistant_output_renders_as_chat_message() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "hi".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let session_id = app.active_session_id().expect("first launch").to_string();

        let chunk =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello, Val."}]}}"#
                .to_string()
                + "\n";
        app.push_event_message(&output_event(1, &session_id, &chunk));

        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("Hello, Val.") && matches!(m.role, MessageRole::Agent)),
            "stream-json assistant text must be rendered as an agent message"
        );
        // is_responding stays true until the result event arrives.
        assert!(app.is_responding);

        let result_chunk =
            r#"{"type":"result","subtype":"success","is_error":false}"#.to_string() + "\n";
        app.push_event_message(&output_event(2, &session_id, &result_chunk));
        assert!(!app.is_responding, "result event must clear is_responding");
    }

    #[test]
    fn stream_json_assistant_split_across_two_output_chunks_still_renders_correctly() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "hi".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let session_id = app.active_session_id().expect("first launch").to_string();

        // Realistic case: the daemon's 8KiB read split a single JSON line
        // exactly in the middle. Without buffering, both halves are
        // unparseable and the assistant text would be dropped silently.
        let full_line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello from split."}]}}"#;
        let split_at = full_line.len() / 2;
        let (head, tail) = full_line.split_at(split_at);

        app.push_event_message(&output_event(1, &session_id, head));
        // After the first chunk there's no newline yet, so nothing renders.
        assert!(
            !app.messages
                .iter()
                .any(|m| m.content.contains("Hello from split.")),
            "first chunk alone must not render — line isn't complete"
        );

        // The second chunk completes the line; render now.
        app.push_event_message(&output_event(2, &session_id, &format!("{tail}\n")));
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("Hello from split.")),
            "rejoined line must parse and render after the trailing newline arrives"
        );
    }

    #[test]
    fn stream_json_stderr_envelope_renders_as_system_message() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "hi".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let session_id = app.active_session_id().expect("first launch").to_string();

        let stderr_chunk =
            r#"{"type":"system","subtype":"stderr","text":"warning: auth token expiring soon"}"#
                .to_string()
                + "\n";
        app.push_event_message(&output_event(1, &session_id, &stderr_chunk));

        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("warning: auth token expiring soon")),
            "stream-json stderr envelope must surface as a system message in the transcript"
        );
    }

    #[test]
    fn shutdown_kills_tracked_stream_sessions_and_clears_state() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "hi".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let stream_id = app.active_session_id().expect("first launch").to_string();
        assert!(!app.harness_stream_session_ids.is_empty());

        app.shutdown();

        assert!(
            app.harness_stream_session_ids.is_empty(),
            "shutdown must clear tracked stream session ids"
        );
        assert!(
            mirror
                .calls
                .borrow()
                .iter()
                .any(|c| c == &format!("kill:{stream_id}")),
            "shutdown must issue a kill for each tracked stream session so chat exit doesn't leak a claude process"
        );
    }

    #[test]
    fn stream_session_exit_event_also_drops_the_per_session_json_buffer() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "hi".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let session_id = app.active_session_id().expect("first launch").to_string();

        // Feed a partial JSON line so the buffer has content to leak.
        app.push_event_message(&output_event(
            1,
            &session_id,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"par"#,
        ));
        assert!(
            app.stream_json_buffers.contains_key(&session_id),
            "partial line must be buffered"
        );

        app.push_event_message(&EventRecord {
            seq: 2,
            id: "event-exit".to_string(),
            session_id: session_id.clone(),
            kind: "exit".to_string(),
            payload_json: serde_json::json!({ "status": "completed" }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        });
        assert!(
            !app.stream_json_buffers.contains_key(&session_id),
            "exit must drop the per-session JSON buffer so it doesn't leak across the chat"
        );
    }

    #[test]
    fn clear_transcript_suppresses_the_orphan_kill_event_so_it_doesnt_echo_after_chat_cleared() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "hi".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let stream_id = app.active_session_id().expect("first launch").to_string();

        app.clear_transcript();

        // The kill request fires synchronously; the daemon's resulting
        // `kill` event arrives later via polling. /clear must
        // pre-suppress the failed session so that event doesn't push
        // "Session kill recorded." back into the just-cleared transcript.
        assert!(
            app.suppressed_session_ids.contains(&stream_id),
            "killed stream session must be suppressed so its kill event doesn't echo after /clear"
        );
        // And the active-session state must be cleared so the next
        // user input isn't gated by stale is_responding.
        assert!(app.active_session_id().is_none());
        assert!(!app.is_responding);

        // Simulate the delayed kill event arriving — it must NOT push
        // "Session kill recorded." into the transcript now.
        app.push_event_message(&EventRecord {
            seq: 1,
            id: "event-kill".to_string(),
            session_id: stream_id.clone(),
            kind: "kill".to_string(),
            payload_json: "{}".to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        });
        assert!(
            !app.messages
                .iter()
                .any(|m| m.content.contains("Session kill recorded")),
            "kill event for a suppressed stream session must not surface"
        );
    }

    #[test]
    fn clear_transcript_drops_stream_json_buffers_too() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "hi".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let session_id = app.active_session_id().expect("first launch").to_string();

        app.push_event_message(&output_event(
            1,
            &session_id,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"par"#,
        ));
        assert!(app.stream_json_buffers.contains_key(&session_id));

        app.clear_transcript();
        assert!(
            !app.stream_json_buffers.contains_key(&session_id),
            "/clear must drop per-session JSON buffers along with the stream session ids"
        );
    }

    #[test]
    fn stream_session_exit_event_drops_the_tracked_id_so_next_turn_cold_starts() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "first".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let session_id = app.active_session_id().expect("first launch").to_string();

        // Simulate the stream process dying (crash, kill, etc.).
        app.push_event_message(&EventRecord {
            seq: 1,
            id: "event-exit".to_string(),
            session_id: session_id.clone(),
            kind: "exit".to_string(),
            payload_json: serde_json::json!({ "status": "failed" }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        });
        assert!(
            app.harness_stream_session_ids.is_empty(),
            "exit must drop the dead stream session from the per-harness map"
        );

        // Next turn cold-starts a fresh stream session instead of forwarding.
        app.input = "second".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        assert_eq!(mirror.launched.borrow().len(), 2);
    }

    #[test]
    fn slash_new_drops_conversation_ids_but_preserves_visible_transcript() {
        let coven_home = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_persistence(client, coven_home.path(), project_root.path());
        app.active_agent = Some(1); // claude
        app.input = "first".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let messages_after_first_turn = app.messages.len();
        assert!(
            app.harness_conversation_ids.contains_key("claude"),
            "first claude turn must seed an id"
        );
        assert!(
            persistence::conversations_file(coven_home.path(), project_root.path()).exists(),
            "first turn must have persisted the id"
        );

        app.input = "/new".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        // Conversation ids gone from both memory and disk.
        assert!(app.harness_conversation_ids.is_empty());
        assert!(
            !persistence::conversations_file(coven_home.path(), project_root.path()).exists(),
            "/new must delete the persistence file too"
        );

        // Visible transcript preserved (plus the /new system message).
        assert!(
            app.messages.len() > messages_after_first_turn,
            "/new must keep prior messages and add at least its own system message"
        );
        assert!(
            app.messages
                .iter()
                .any(|m| m.content == "first" && matches!(m.role, MessageRole::User)),
            "the user message from the prior turn must still be visible after /new"
        );
        assert!(app
            .messages
            .iter()
            .any(|m| m.content.contains("Started a new conversation")));
    }

    #[test]
    fn first_chat_turn_after_slash_new_sends_init_not_resume() {
        let coven_home = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let client = RecordingChatClient::default();
        let (mut app, mirror) =
            app_with_persistence(client, coven_home.path(), project_root.path());
        app.active_agent = Some(1); // claude
        app.input = "first".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let first_id = match &mirror.launched.borrow()[0].conversation {
            Some(crate::harness::ConversationHint::Init { id }) => id.clone(),
            other => panic!("turn 1 should be Init, got {other:?}"),
        };
        // Mark the first turn as completed so the next launch isn't gated.
        let session_id = app.active_session_id().expect("first launch").to_string();
        app.push_event_message(&EventRecord {
            seq: 1,
            id: "event-1".to_string(),
            session_id,
            kind: "exit".to_string(),
            payload_json: serde_json::json!({ "status": "completed" }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        });

        app.input = "/new".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        app.input = "fresh topic".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        let launched = mirror.launched.borrow();
        assert_eq!(launched.len(), 2);
        match &launched[1].conversation {
            Some(crate::harness::ConversationHint::Init { id }) => {
                assert_ne!(id, &first_id, "/new must yield a fresh conversation id");
            }
            other => panic!("first turn after /new should be Init, got {other:?}"),
        }
    }

    #[test]
    fn slash_new_is_a_chat_local_command_not_routed_through_cast() {
        assert!(is_chat_local_slash("/new"));
        assert!(is_chat_local_slash("/NEW"));
    }

    #[test]
    fn clear_transcript_wipes_persisted_conversations_file() {
        let coven_home = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_persistence(client, coven_home.path(), project_root.path());
        app.active_agent = Some(1); // claude
        app.input = "hello".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        assert!(
            persistence::conversations_file(coven_home.path(), project_root.path()).exists(),
            "first turn should have created the persistence file"
        );

        app.clear_transcript();

        assert!(
            !persistence::conversations_file(coven_home.path(), project_root.path()).exists(),
            "/clear must delete the persistence file so restart starts fresh"
        );
        assert!(app.harness_conversation_ids.is_empty());
    }

    #[test]
    fn app_without_coven_home_does_not_attempt_persistence() {
        // Sanity check: tests that don't pass a coven_home (the default
        // `app_with_client` path) must keep working without touching disk.
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "hello".to_string();
        app.cursor_pos = app.input.len();

        app.handle_input();

        assert!(app.harness_conversation_ids.contains_key("claude"));
        assert!(app.coven_home.is_none());
    }

    #[test]
    fn detect_stale_session_matches_known_per_harness_phrases() {
        assert!(detect_stale_session(
            "claude",
            "No conversation found with session ID: 00000000-0000-0000-0000-000000000000"
        ));
        assert!(detect_stale_session(
            "codex",
            "Error: thread/resume: thread/resume failed: no rollout found for thread id 00000000-..."
        ));
        assert!(detect_stale_session(
            "codex",
            "thread/resume failed: something else"
        ));
        // Different harness id doesn't match either phrase.
        assert!(!detect_stale_session(
            "hermes",
            "No conversation found with session ID: x"
        ));
        // Plain content with neither phrase.
        assert!(!detect_stale_session("claude", "Hi Persist."));
        assert!(!detect_stale_session("codex", "session id: 019e..."));
    }

    #[test]
    fn stale_claude_resume_replaces_id_and_auto_resends_original_prompt() {
        let coven_home = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let stored_id = "fab1efac-1234-5678-9abc-def012345678";
        let mut seed = std::collections::HashMap::new();
        seed.insert("claude".to_string(), stored_id.to_string());
        persistence::save_for_project(coven_home.path(), project_root.path(), &seed)
            .expect("seed persisted state");

        let client = RecordingChatClient::default();
        let (mut app, mirror) =
            app_with_persistence(client, coven_home.path(), project_root.path());
        app.active_agent = Some(1); // claude
        app.input = "hello again".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let session_id = app
            .active_session_id()
            .expect("first launch sets id")
            .to_string();

        // Simulate claude rejecting our stale --resume.
        app.push_event_message(&output_event(
            1,
            &session_id,
            &stale_stderr_chunk(&format!(
                "No conversation found with session ID: {stored_id}"
            )),
        ));

        // Stale id must be gone — but auto-retry should have created a
        // fresh one in its place (claude pre-assigns via --session-id).
        let new_id = app
            .harness_conversation_ids
            .get("claude")
            .cloned()
            .expect("auto-retry should have stored a fresh claude id");
        assert_ne!(new_id, stored_id, "fresh id must not equal the stale one");
        let stored = persistence::load_for_project(coven_home.path(), project_root.path());
        assert_eq!(
            stored.get("claude").cloned(),
            Some(new_id.clone()),
            "fresh id must be persisted to disk"
        );

        // Two launches: the original (Resume with stale id) and the auto-
        // retry (Init with the fresh id, carrying the same prompt).
        let launched = mirror.launched.borrow();
        assert_eq!(launched.len(), 2);
        assert_eq!(launched[0].prompt, "hello again");
        assert_eq!(launched[1].prompt, "hello again");
        match &launched[1].conversation {
            Some(crate::harness::ConversationHint::Init { id }) => {
                assert_eq!(id, &new_id);
            }
            other => panic!("auto-retry should carry Init with the new id, got {other:?}"),
        }
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("re-sending your message")),
            "user must see a system message about the auto-retry"
        );
    }

    #[test]
    fn stale_recovery_hides_raw_error_chunk_and_failed_session_exit_from_transcript() {
        let coven_home = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let stored_id = "fab1efac-1234-5678-9abc-def012345678";
        let mut seed = std::collections::HashMap::new();
        seed.insert("claude".to_string(), stored_id.to_string());
        persistence::save_for_project(coven_home.path(), project_root.path(), &seed)
            .expect("seed persisted state");

        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_persistence(client, coven_home.path(), project_root.path());
        app.active_agent = Some(1); // claude
        app.input = "hello again".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let failed_session = app.active_session_id().expect("first launch").to_string();

        // The chunk that contains the stale phrase.
        app.push_event_message(&output_event(
            1,
            &failed_session,
            &stale_stderr_chunk(&format!(
                "No conversation found with session ID: {stored_id}"
            )),
        ));
        // A trailing chunk from the same failed session (ANSI cleanup, etc.).
        app.push_event_message(&output_event(
            2,
            &failed_session,
            "trailing teardown noise\n",
        ));
        // And finally the failed session's exit event.
        app.push_event_message(&EventRecord {
            seq: 3,
            id: "event-exit".to_string(),
            session_id: failed_session.clone(),
            kind: "exit".to_string(),
            payload_json: serde_json::json!({ "status": "completed" }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        });

        // None of the failed session's content (raw error, trailing noise,
        // "Session completed.") should appear in the transcript.
        let transcript: Vec<&str> = app.messages.iter().map(|m| m.content.as_str()).collect();
        for content in &transcript {
            assert!(
                !content.contains("No conversation found with session ID"),
                "raw stale error must be hidden, found: {content}"
            );
            assert!(
                !content.contains("trailing teardown noise"),
                "trailing output from the failed session must be hidden, found: {content}"
            );
            assert!(
                !content.contains("Session completed"),
                "orphaned exit message from the failed session must be hidden, found: {content}"
            );
        }
        // The system message and the retry's "Connected" line should be visible.
        assert!(transcript
            .iter()
            .any(|c| c.contains("re-sending your message")));
        assert!(transcript.iter().any(|c| c.contains("Connected")));
        // Suppression entry must be cleared once the exit is consumed.
        assert!(!app.suppressed_session_ids.contains(&failed_session));
    }

    #[test]
    fn suppression_only_applies_to_the_failed_session_not_other_sessions() {
        let coven_home = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let stored_id = "fab1efac-1234-5678-9abc-def012345678";
        let mut seed = std::collections::HashMap::new();
        seed.insert("claude".to_string(), stored_id.to_string());
        persistence::save_for_project(coven_home.path(), project_root.path(), &seed)
            .expect("seed persisted state");

        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_persistence(client, coven_home.path(), project_root.path());
        app.active_agent = Some(1); // claude
        app.input = "hello".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let failed_session = app.active_session_id().expect("first launch").to_string();

        app.push_event_message(&output_event(
            1,
            &failed_session,
            &stale_stderr_chunk(&format!(
                "No conversation found with session ID: {stored_id}"
            )),
        ));
        let retry_session = app.active_session_id().expect("retry session").to_string();
        assert_ne!(retry_session, failed_session);

        // Output from the retry session must still be rendered. The retry
        // is a stream-mode claude session, so the chunk is a stream-json
        // assistant event rather than plain text.
        let assistant_chunk = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hi from the new conversation."}]}}"#.to_string() + "\n";
        app.push_event_message(&output_event(2, &retry_session, &assistant_chunk));

        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("Hi from the new conversation")),
            "retry-session output must not be suppressed"
        );
    }

    #[test]
    fn poll_session_events_stops_advancing_cursor_when_active_session_changes_mid_batch() {
        let coven_home = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let stored_id = "fab1efac-1234-5678-9abc-def012345678";
        let mut seed = std::collections::HashMap::new();
        seed.insert("claude".to_string(), stored_id.to_string());
        persistence::save_for_project(coven_home.path(), project_root.path(), &seed)
            .expect("seed persisted state");

        let client = RecordingChatClient::default();
        let (mut app, mirror) =
            app_with_persistence(client, coven_home.path(), project_root.path());
        app.active_agent = Some(1); // claude
        app.input = "hi".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let old_session = app.active_session_id().expect("first launch").to_string();

        // Pre-load three events for the OLD session: a harmless first
        // one, a stale-error in the middle, and a "trailing noise"
        // event afterward. Without the active-session-id guard in
        // poll_session_events, processing the trailing event would
        // overwrite `last_event_seq` after the stale handler had
        // reset it to None for the new session, leaving a poisoned
        // cursor.
        let stored_id_for_error = stored_id.to_string();
        let old_session_for_events = old_session.clone();
        mirror.events.borrow_mut().extend(vec![
            EventRecord {
                seq: 10,
                id: "ev-10".to_string(),
                session_id: old_session_for_events.clone(),
                kind: "output".to_string(),
                payload_json: serde_json::json!({ "data": "" }).to_string(),
                created_at: "2026-05-19T00:00:00Z".to_string(),
            },
            EventRecord {
                seq: 11,
                id: "ev-11".to_string(),
                session_id: old_session_for_events.clone(),
                kind: "output".to_string(),
                payload_json: serde_json::json!({
                    "data": stale_stderr_chunk(&format!(
                        "No conversation found with session ID: {stored_id_for_error}"
                    ))
                })
                .to_string(),
                created_at: "2026-05-19T00:00:00Z".to_string(),
            },
            EventRecord {
                seq: 12,
                id: "ev-12".to_string(),
                session_id: old_session_for_events,
                kind: "output".to_string(),
                payload_json: serde_json::json!({ "data": "trailing noise after stale\n" })
                    .to_string(),
                created_at: "2026-05-19T00:00:00Z".to_string(),
            },
        ]);

        app.poll_session_events();

        // Active session should have swapped to the retry session.
        let new_session = app
            .active_session_id()
            .expect("auto-retry must have set a new active session");
        assert_ne!(new_session, old_session);

        // Cursor must be at None (the value the auto-retry reset to),
        // NOT Some(12) from the trailing OLD-session event. If it were
        // Some(12), the next poll for the new session would query with
        // after_seq=12 and skip any new-session events that arrived
        // with smaller seqs.
        assert_eq!(
            app.last_event_seq, None,
            "active-session swap during a batch must stop the loop from advancing the cursor past the swap"
        );
    }

    #[test]
    fn stale_recovery_only_auto_retries_once_per_user_turn() {
        let coven_home = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let stored_id = "fab1efac-1234-5678-9abc-def012345678";
        let mut seed = std::collections::HashMap::new();
        seed.insert("claude".to_string(), stored_id.to_string());
        persistence::save_for_project(coven_home.path(), project_root.path(), &seed)
            .expect("seed persisted state");

        let client = RecordingChatClient::default();
        let (mut app, mirror) =
            app_with_persistence(client, coven_home.path(), project_root.path());
        app.active_agent = Some(1); // claude
        app.input = "hello".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let first_session = app.active_session_id().expect("first launch").to_string();

        // First stale event → consumes the auto-retry budget.
        app.push_event_message(&output_event(
            1,
            &first_session,
            &stale_stderr_chunk(&format!(
                "No conversation found with session ID: {stored_id}"
            )),
        ));
        let after_first_retry = mirror.launched.borrow().len();
        assert_eq!(after_first_retry, 2, "first stale event triggers a retry");
        let retry_session = app.active_session_id().expect("retry sets id").to_string();
        let retry_id = app.harness_conversation_ids.get("claude").cloned().unwrap();

        // Simulate the retry itself also somehow hitting stale (pathological
        // — claude wouldn't really say this for an Init session — but we
        // guard against it to bound the loop).
        app.push_event_message(&output_event(
            2,
            &retry_session,
            &stale_stderr_chunk(&format!(
                "No conversation found with session ID: {retry_id}"
            )),
        ));
        assert_eq!(
            mirror.launched.borrow().len(),
            after_first_retry,
            "second stale event in the same turn must not auto-retry again"
        );
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("Send your message again")),
            "second stale event falls back to asking the user to retype"
        );
        // The fallback path must also clear the wedged state so the
        // user's NEXT message can actually be sent — otherwise
        // is_responding stays true forever (failed session's exit
        // event is suppressed, normal state-reset arms in
        // push_event_message never run).
        assert!(
            !app.is_responding,
            "after the retry-exhausted fallback, is_responding must be cleared so the next message isn't gated"
        );
        assert!(
            app.active_session_id().is_none(),
            "after the retry-exhausted fallback, active_session_id must be cleared so the next message launches fresh"
        );

        // And prove the chat is actually usable: send a new message, it
        // should produce a fresh launch instead of being rejected.
        let launches_before_retype = mirror.launched.borrow().len();
        app.input = "second attempt".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        assert_eq!(
            mirror.launched.borrow().len(),
            launches_before_retype + 1,
            "user's manual retype after retry-exhausted fallback must produce a fresh launch, not a still-streaming rejection"
        );
    }

    #[test]
    fn stale_codex_resume_drops_codex_id_only() {
        let coven_home = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        // Seed both claude and codex; only codex should get dropped.
        let mut seed = std::collections::HashMap::new();
        seed.insert("claude".to_string(), "claude-uuid".to_string());
        seed.insert("codex".to_string(), "codex-uuid".to_string());
        persistence::save_for_project(coven_home.path(), project_root.path(), &seed)
            .expect("seed persisted state");

        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_persistence(client, coven_home.path(), project_root.path());
        app.active_agent = Some(0); // codex
        app.input = "hello again".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let session_id = app
            .active_session_id()
            .expect("first launch sets id")
            .to_string();

        app.push_event_message(&output_event(
            1,
            &session_id,
            "Error: thread/resume: thread/resume failed: no rollout found for thread id codex-uuid\n",
        ));

        assert!(!app.harness_conversation_ids.contains_key("codex"));
        assert!(
            app.harness_conversation_ids.contains_key("claude"),
            "claude id must not be touched by a codex stale event"
        );
        let stored = persistence::load_for_project(coven_home.path(), project_root.path());
        assert!(!stored.contains_key("codex"));
        assert_eq!(
            stored.get("claude").map(String::as_str),
            Some("claude-uuid")
        );
    }

    #[test]
    fn stale_pattern_in_attached_session_output_does_not_drop_chat_ids() {
        let coven_home = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let mut seed = std::collections::HashMap::new();
        seed.insert("claude".to_string(), "claude-uuid".to_string());
        persistence::save_for_project(coven_home.path(), project_root.path(), &seed)
            .expect("seed persisted state");

        let attached = test_session("attached-session", "claude", "external", "running");
        let client = RecordingChatClient::with_session(attached);
        let (mut app, _) = app_with_persistence(client, coven_home.path(), project_root.path());
        app.attach_session("attached-session");
        assert!(!app.chat_owns_active_session);

        // Output from the attached session contains the stale phrase, but
        // since chat doesn't own this session we must not touch our own
        // persisted ids.
        app.push_event_message(&output_event(
            1,
            "attached-session",
            "No conversation found with session ID: irrelevant\n",
        ));

        assert!(
            app.harness_conversation_ids.contains_key("claude"),
            "attached-session output must not clobber chat-owned ids"
        );
    }

    #[test]
    fn stale_pattern_with_no_stored_id_is_a_noop() {
        let coven_home = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_persistence(client, coven_home.path(), project_root.path());
        app.active_agent = Some(0); // codex
        app.input = "hi".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        let session_id = app
            .active_session_id()
            .expect("first launch sets id")
            .to_string();
        assert!(!app.harness_conversation_ids.contains_key("codex"));

        // Stale phrase arrives during a turn that had no stored codex id —
        // nothing to drop, nothing to warn about.
        app.push_event_message(&output_event(
            1,
            &session_id,
            "thread/resume failed: bogus\n",
        ));

        assert!(
            !app.messages
                .iter()
                .any(|m| m.content.contains("no longer exists")),
            "must not emit a misleading warning when there was no stored id"
        );
    }

    #[test]
    fn extract_codex_session_id_parses_banner_lines_only() {
        assert_eq!(
            extract_codex_session_id("session id: 019e5998-7130-7872-8d96-a6b67c5b6406"),
            Some("019e5998-7130-7872-8d96-a6b67c5b6406".to_string())
        );
        assert_eq!(
            extract_codex_session_id("workdir: /tmp\n--------\nsession id: abc-123\n"),
            Some("abc-123".to_string())
        );
        assert_eq!(extract_codex_session_id("session id:\n"), None);
        assert_eq!(extract_codex_session_id("hello world"), None);
    }

    #[test]
    fn chat_input_while_responding_does_not_launch_a_second_session() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.active_agent = Some(1); // claude
        app.input = "first".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();
        assert!(app.is_responding, "first turn should set is_responding");

        // Second send while previous reply is still streaming.
        app.input = "too soon".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        assert_eq!(
            mirror.launched.borrow().len(),
            1,
            "second send while is_responding must not launch a fresh session"
        );
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("still streaming")));
    }

    #[test]
    fn plain_chat_input_launches_non_interactive_daemon_session_without_mock_response() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.input = "summarize the repo".to_string();
        app.cursor_pos = app.input.len();

        let result = app.handle_input();

        assert!(matches!(result, Some(SlashCommandResult::Handled)));
        let launched = mirror.launched.borrow();
        assert_eq!(launched.len(), 1);
        assert_eq!(launched[0].harness, "codex");
        assert_eq!(launched[0].prompt, "summarize the repo");
        assert_eq!(
            launched[0].launch_mode,
            crate::harness::HarnessLaunchMode::NonInteractive
        );
        assert!(app.active_session_id().is_some());
        assert!(app.messages.iter().any(|message| message
            .content
            .contains("Connected. Waiting for the reply.")));
        assert!(!app
            .messages
            .iter()
            .any(|message| message.content.contains("placeholder response")));
    }

    #[test]
    fn launched_chat_session_stays_responding_until_exit_event() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.input = "summarize the repo".to_string();
        app.cursor_pos = app.input.len();

        app.handle_input();

        let session_id = app.active_session_id().expect("session should be active");
        assert!(app.is_responding);

        app.push_event_message(&EventRecord {
            seq: 1,
            id: "event-1".to_string(),
            session_id: session_id.to_string(),
            kind: "exit".to_string(),
            payload_json: serde_json::json!({ "status": "completed" }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        });

        assert_eq!(app.active_session_id(), None);
        assert!(!app.is_responding);
    }

    #[test]
    fn daemon_launch_failure_surfaces_status_guidance_inline() {
        let client = RecordingChatClient::default();
        *client.launch_error.borrow_mut() = Some("connection refused".to_string());
        let (mut app, _) = app_with_client(client);
        app.input = "fix the failing tests".to_string();
        app.cursor_pos = app.input.len();

        app.handle_input();

        assert!(app.messages.iter().any(|message| message
            .content
            .contains("Daemon launch failed: connection refused")
            && message.content.contains("coven daemon status")
            && !message.content.contains("coven daemon start")));
    }

    #[test]
    fn plain_chat_input_launches_without_operational_cards_in_transcript() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.input = "fix the failing tests".to_string();
        app.cursor_pos = app.input.len();

        app.handle_input();

        let transcript = app
            .messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(transcript.contains("Starting Codex"));
        assert!(!transcript.contains("Cast plan"));
        assert!(!transcript.contains("Cast outcome"));
        assert!(!transcript.contains("Started daemon session"));
        assert!(
            !transcript.contains("session-"),
            "safe natural chat should not expose daemon ids inline: {transcript}"
        );
    }

    #[test]
    fn slash_run_input_appends_cast_plan_before_daemon_launch() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.input = "/run claude review the diff".to_string();
        app.cursor_pos = app.input.len();

        let result = app.handle_input();

        assert!(matches!(result, Some(SlashCommandResult::Handled)));
        let launched = mirror.launched.borrow();
        assert_eq!(launched.len(), 1);
        assert_eq!(launched[0].harness, "claude");
        assert_eq!(launched[0].prompt, "review the diff");
        let plan_index = app
            .messages
            .iter()
            .position(|message| message.content.contains("Cast plan"))
            .expect("chat transcript should include Cast plan");
        let launch_index = app
            .messages
            .iter()
            .position(|message| {
                message
                    .content
                    .contains("Connected. Waiting for the reply.")
            })
            .expect("safe slash plan should launch");
        assert!(plan_index < launch_index);
        assert!(app.messages[plan_index]
            .content
            .contains("harness Claude Code · user-chosen"));
    }

    #[test]
    fn slash_attach_input_appends_cast_plan_before_attaching() {
        let client = RecordingChatClient::with_session(test_session(
            "session-1",
            "codex",
            "Existing",
            "running",
        ));
        let (mut app, mirror) = app_with_client(client);
        app.input = "/attach session-1".to_string();
        app.cursor_pos = app.input.len();

        let result = app.handle_input();

        assert!(matches!(result, Some(SlashCommandResult::Handled)));
        assert_eq!(app.active_session_id(), Some("session-1"));
        assert!(mirror.calls.borrow().contains(&"get:session-1".to_string()));
        let plan_index = app
            .messages
            .iter()
            .position(|message| message.content.contains("Cast plan"))
            .expect("chat transcript should include Cast plan");
        let attach_index = app
            .messages
            .iter()
            .position(|message| message.content.contains("Attached to daemon session"))
            .expect("attach should still work");
        assert!(plan_index < attach_index);
    }

    #[test]
    fn slash_kill_input_appends_cast_plan_before_killing_session() {
        let client = RecordingChatClient::with_session(test_session(
            "session-1",
            "codex",
            "Existing",
            "running",
        ));
        let (mut app, mirror) = app_with_client(client);
        app.input = "/kill session-1".to_string();
        app.cursor_pos = app.input.len();

        let result = app.handle_input();

        assert!(matches!(result, Some(SlashCommandResult::Handled)));
        assert!(mirror
            .calls
            .borrow()
            .contains(&"kill:session-1".to_string()));
        let plan_index = app
            .messages
            .iter()
            .position(|message| message.content.contains("Cast plan"))
            .expect("chat transcript should include Cast plan");
        let kill_index = app
            .messages
            .iter()
            .position(|message| {
                message
                    .content
                    .contains("Kill accepted for session session-1")
            })
            .expect("kill should still work");
        assert!(plan_index < kill_index);
    }

    #[test]
    fn slash_kill_without_id_uses_active_session_through_cast_plan() {
        let client = RecordingChatClient::with_session(test_session(
            "session-1",
            "codex",
            "Existing",
            "running",
        ));
        let (mut app, mirror) = app_with_client(client);
        app.active_session_id = Some("session-1".to_string());
        app.input = "/kill".to_string();
        app.cursor_pos = app.input.len();

        let result = app.handle_input();

        assert!(matches!(result, Some(SlashCommandResult::Handled)));
        assert!(mirror
            .calls
            .borrow()
            .contains(&"kill:session-1".to_string()));
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("Cast plan")
                && message.content.contains("session-1")));
    }

    #[test]
    fn slash_archive_input_appends_cast_plan_before_archiving_session() {
        let client = RecordingChatClient::with_session(test_session(
            "session-1",
            "codex",
            "Existing",
            "completed",
        ));
        let (mut app, mirror) = app_with_client(client);
        app.input = "/archive session-1".to_string();
        app.cursor_pos = app.input.len();

        let result = app.handle_input();

        assert!(matches!(result, Some(SlashCommandResult::Handled)));
        assert!(mirror
            .calls
            .borrow()
            .contains(&"archive:session-1".to_string()));
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("Cast plan")
                && message.content.contains("session-1")));
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("Archived session session-1")));
    }

    #[test]
    fn slash_summon_input_appends_cast_plan_before_summoning_and_attaching() {
        let mut session = test_session("session-1", "codex", "Existing", "completed");
        session.archived_at = Some("2026-05-18T00:00:00Z".to_string());
        let client = RecordingChatClient::with_session(session);
        let (mut app, mirror) = app_with_client(client);
        app.input = "/summon session-1".to_string();
        app.cursor_pos = app.input.len();

        let result = app.handle_input();

        assert!(matches!(result, Some(SlashCommandResult::Handled)));
        assert!(mirror
            .calls
            .borrow()
            .contains(&"summon:session-1".to_string()));
        assert_eq!(app.active_session_id(), Some("session-1"));
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("Cast plan")
                && message.content.contains("session-1")));
    }

    #[test]
    fn slash_sacrifice_waits_for_confirmation_then_deletes_session() {
        let client = RecordingChatClient::with_session(test_session(
            "session-1",
            "codex",
            "Existing",
            "completed",
        ));
        let (mut app, mirror) = app_with_client(client);
        app.input = "/sacrifice session-1".to_string();
        app.cursor_pos = app.input.len();

        app.handle_input();

        assert!(app.pending_cast_confirmation.is_some());
        assert!(!mirror
            .calls
            .borrow()
            .contains(&"sacrifice:session-1".to_string()));

        app.input = "accept".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        assert!(app.pending_cast_confirmation.is_none());
        assert!(mirror
            .calls
            .borrow()
            .contains(&"sacrifice:session-1".to_string()));
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("Sacrificed session session-1")));
    }

    #[test]
    fn informational_cast_slashes_do_not_fall_through_to_unwired_message() {
        for input in ["/start", "/tui", "/patch", "/quest ship chat mode"] {
            let client = RecordingChatClient::default();
            let (mut app, _) = app_with_client(client);
            app.input = input.to_string();
            app.cursor_pos = app.input.len();

            let result = app.handle_input();

            assert!(matches!(result, Some(SlashCommandResult::Handled)));
            assert!(app
                .messages
                .iter()
                .any(|message| message.content.contains("Cast plan")));
            assert!(!app
                .messages
                .iter()
                .any(|message| message.content.contains("not wired yet")));
        }
    }

    #[test]
    fn risky_chat_input_waits_for_confirmation_and_accept_launches_without_duplicate_plan() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.input = "publish the package".to_string();
        app.cursor_pos = app.input.len();

        app.handle_input();

        assert!(app.pending_cast_confirmation.is_some());
        assert!(mirror.launched.borrow().is_empty());
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("Confirmation required")));

        app.input = "accept".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        assert!(app.pending_cast_confirmation.is_none());
        assert_eq!(mirror.launched.borrow().len(), 1);
        assert_eq!(
            app.messages
                .iter()
                .filter(|message| message.content.contains("Cast plan"))
                .count(),
            1
        );
    }

    #[test]
    fn escape_cancels_pending_confirmation_before_accept_can_launch() {
        let client = RecordingChatClient::default();
        let (mut app, mirror) = app_with_client(client);
        app.input = "publish the package".to_string();
        app.cursor_pos = app.input.len();

        app.handle_input();
        app.cancel_pending_cast_confirmation();
        app.input = "accept".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        assert!(app.pending_cast_confirmation.is_none());
        assert!(!mirror
            .launched
            .borrow()
            .iter()
            .any(|request| request.prompt == "publish the package"));
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("Cancelled Cast confirmation")));
    }

    #[test]
    fn completed_chat_session_clears_active_session_so_next_message_launches_cleanly() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.active_session_id = Some("session-1".to_string());

        app.push_event_message(&EventRecord {
            seq: 1,
            id: "event-1".to_string(),
            session_id: "session-1".to_string(),
            kind: "exit".to_string(),
            payload_json: serde_json::json!({ "status": "completed" }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        });

        assert_eq!(app.active_session_id(), None);
        assert!(!app.is_responding);
    }

    #[test]
    fn kill_event_clears_active_session_so_next_message_launches_cleanly() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.active_session_id = Some("session-1".to_string());
        app.is_responding = true;

        app.push_event_message(&EventRecord {
            seq: 1,
            id: "event-1".to_string(),
            session_id: "session-1".to_string(),
            kind: "kill".to_string(),
            payload_json: serde_json::json!({ "status": "killed" }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        });

        assert_eq!(app.active_session_id(), None);
        assert!(!app.is_responding);
    }

    #[test]
    fn followup_chat_input_forwards_to_attached_daemon_session() {
        let client = RecordingChatClient::with_session(test_session(
            "session-1",
            "codex",
            "Existing",
            "running",
        ));
        let (mut app, mirror) = app_with_client(client);
        app.attach_session("session-1");
        app.input = "next step".to_string();
        app.cursor_pos = app.input.len();

        let result = app.handle_input();

        assert!(matches!(result, Some(SlashCommandResult::Handled)));
        assert!(mirror
            .calls
            .borrow()
            .contains(&"input:session-1:next step\n".to_string()));
    }

    #[test]
    fn confirmation_words_forward_to_active_session_without_pending_cast_confirmation() {
        let client = RecordingChatClient::with_session(test_session(
            "session-1",
            "codex",
            "Existing",
            "running",
        ));
        let (mut app, mirror) = app_with_client(client);
        app.attach_session("session-1");
        app.input = "yes".to_string();
        app.cursor_pos = app.input.len();

        let result = app.handle_input();

        assert!(matches!(result, Some(SlashCommandResult::Handled)));
        assert!(mirror
            .calls
            .borrow()
            .contains(&"input:session-1:yes\n".to_string()));
        assert!(!app
            .messages
            .iter()
            .any(|message| message.content.contains("No Cast confirmation is pending")));
    }

    #[test]
    fn attach_session_loads_daemon_events_into_transcript() {
        let client = RecordingChatClient::with_session(test_session(
            "session-1",
            "codex",
            "Existing",
            "running",
        ));
        client
            .events
            .borrow_mut()
            .push(output_event(1, "session-1", "hello from daemon"));
        let (mut app, mirror) = app_with_client(client);

        app.handle_slash_command("/attach session-1");

        assert_eq!(app.active_session_id(), Some("session-1"));
        assert!(mirror
            .calls
            .borrow()
            .contains(&"events:session-1:0".to_string()));
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("hello from daemon")));
    }

    #[test]
    fn chat_output_events_are_terminal_sanitized_and_coalesced() {
        let client = RecordingChatClient::with_session(test_session(
            "session-1",
            "codex",
            "Existing",
            "running",
        ));
        client.events.borrow_mut().extend([
            output_event(1, "session-1", "\x1b[?2004h\x1b[39;49m"),
            output_event(2, "session-1", "\x1b[2J\x1b[1;1HHello"),
            output_event(3, "session-1", "\x1b[39;49m world\x1b[0m\r\n"),
        ]);
        let (mut app, _) = app_with_client(client);

        app.handle_slash_command("/attach session-1");

        let agent_messages: Vec<_> = app
            .messages
            .iter()
            .filter(|message| matches!(message.role, MessageRole::Agent))
            .collect();
        assert_eq!(agent_messages.len(), 1);
        assert_eq!(agent_messages[0].content, "Hello world\n");
        assert!(!agent_messages[0].content.contains('\x1b'));
        assert!(!agent_messages[0].content.contains("[39;49m"));
        assert!(!agent_messages[0].content.contains("[?2004h"));
    }

    #[test]
    fn codex_transcript_output_keeps_assistant_text_and_hides_tool_details() {
        let client = RecordingChatClient::with_session(test_session(
            "session-1",
            "codex",
            "Existing",
            "running",
        ));
        client.events.borrow_mut().extend([
            output_event(
                1,
                "session-1",
                "OpenAI Codex v0.133.0\r\n--------\r\nworkdir: /tmp/project\r\nmodel: gpt-5.5\r\n--------\r\nuser\r\nhi there\r\nhook: SessionStart\r\ncodex\r\nI can help with that.\r\nexec\r\n/bin/zsh -lc \"cat secret\"\r\n  succeeded in 0ms:\r\nprivate tool output\r\n",
            ),
            output_event(
                2,
                "session-1",
                "codex\r\nHere is the useful answer.\r\n",
            ),
            output_event(3, "session-1", "tokens used\r\n12,345\r\n"),
        ]);
        let (mut app, _) = app_with_client(client);

        app.handle_slash_command("/attach session-1");

        let agent_text = app
            .messages
            .iter()
            .filter(|message| matches!(message.role, MessageRole::Agent))
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(agent_text.contains("I can help with that."));
        assert!(agent_text.contains("Here is the useful answer."));
        assert!(!agent_text.contains("OpenAI Codex"));
        assert!(!agent_text.contains("workdir:"));
        assert!(!agent_text.contains("hook:"));
        assert!(!agent_text.contains("/bin/zsh"));
        assert!(!agent_text.contains("private tool output"));
        assert!(!agent_text.contains("tokens used"));
    }

    #[test]
    fn clean_terminal_output_strips_osc_title_terminated_by_bel() {
        // `ESC ] 0 ; <title> BEL` is the canonical xterm title-setting OSC.
        // Both the introducer and the payload must be fully consumed.
        let cleaned = clean_terminal_output("before\x1b]0;Window Title\x07after")
            .expect("non-empty after sanitization");
        assert_eq!(cleaned, "beforeafter");
        assert!(!cleaned.contains('\x1b'));
        assert!(!cleaned.contains("Window Title"));
        assert!(!cleaned.contains('\x07'));
    }

    #[test]
    fn clean_terminal_output_strips_osc_hyperlink_terminated_by_st() {
        // OSC 8 hyperlinks use the ESC-backslash String Terminator, not BEL.
        // The visible "link text" between the opening and closing OSC must
        // survive; everything else (URL, terminators) must be stripped.
        let input = "\x1b]8;;https://example.com/\x1b\\link text\x1b]8;;\x1b\\!";
        let cleaned = clean_terminal_output(input).expect("non-empty after sanitization");
        assert_eq!(cleaned, "link text!");
        assert!(!cleaned.contains('\x1b'));
        assert!(!cleaned.contains("example.com"));
    }

    #[test]
    fn clean_terminal_output_applies_backspaces_to_prior_chars() {
        // `\x08` pops the most recently emitted char so harness output that
        // uses backspace for in-place rewrites (e.g. progress spinners) does
        // not leave the pre-rewrite text in the chat transcript.
        let cleaned =
            clean_terminal_output("Hello\x08\x08world").expect("non-empty after sanitization");
        assert_eq!(cleaned, "Helworld");
    }

    #[test]
    fn clean_terminal_output_drops_messages_that_are_pure_control_noise() {
        // Cursor-visibility toggles, mode sets, and similar invisible-only
        // sequences must not create empty chat bubbles.
        assert_eq!(clean_terminal_output("\x1b[?25l\x1b[?25h"), None);
        assert_eq!(clean_terminal_output("\x1b]0;just a title\x07"), None);
        assert_eq!(clean_terminal_output("\r\r\r"), None);
        // Pure space/tab without any newline is still invisible noise.
        assert_eq!(clean_terminal_output("   "), None);
        assert_eq!(clean_terminal_output("\t\t"), None);
    }

    #[test]
    fn clean_terminal_output_preserves_newline_only_chunks_for_paragraph_breaks() {
        // When the daemon streams a markdown reply line-by-line, blank source
        // lines arrive as `\n`-only payloads. Dropping them collapses the
        // paragraph structure on the way to the message body, so headings
        // and tables end up stuck to the next block. Keep any chunk that
        // carries a newline.
        assert_eq!(clean_terminal_output("\n"), Some("\n".to_string()));
        assert_eq!(clean_terminal_output("\n\n"), Some("\n\n".to_string()));
        // Even mixed with control noise the newline must survive.
        assert_eq!(
            clean_terminal_output("\x1b[?25l\n\x1b[?25h"),
            Some("\n".to_string())
        );
    }

    #[test]
    fn clean_terminal_output_preserves_tabs_and_newlines() {
        // Tabs and newlines are the only whitespace control chars we keep —
        // they carry layout information harnesses rely on for readability.
        let cleaned =
            clean_terminal_output("col1\tcol2\nrow2\tend").expect("non-empty after sanitization");
        assert_eq!(cleaned, "col1\tcol2\nrow2\tend");
    }

    #[test]
    fn poll_session_events_backs_off_and_coalesces_repeated_failures() {
        let client = RecordingChatClient::default();
        *client.event_error.borrow_mut() = Some("daemon unavailable".to_string());
        let (mut app, mirror) = app_with_client(client);
        app.active_session_id = Some("session-1".to_string());

        app.poll_session_events();
        app.poll_session_events();
        app.event_poll_backoff_until = Some(Instant::now() - Duration::from_millis(1));
        app.poll_session_events();

        let calls = mirror.calls.borrow();
        assert_eq!(
            calls
                .iter()
                .filter(|call| *call == "events:session-1:0")
                .count(),
            2
        );
        assert_eq!(
            app.messages
                .iter()
                .filter(|message| message.content == "Event follow failed: daemon unavailable")
                .count(),
            1
        );
    }

    #[test]
    fn api_mismatch_stops_event_polling_until_next_user_input() {
        let client = RecordingChatClient::default();
        *client.event_error.borrow_mut() = Some(
            "Coven daemon API mismatch: expected coven.daemon.v1, got coven.daemon.v0".to_string(),
        );
        let (mut app, mirror) = app_with_client(client);
        app.active_session_id = Some("session-1".to_string());

        app.poll_session_events();
        app.event_poll_backoff_until = Some(Instant::now() - Duration::from_millis(1));
        app.poll_session_events();

        assert_eq!(
            mirror
                .calls
                .borrow()
                .iter()
                .filter(|call| *call == "events:session-1:0")
                .count(),
            1
        );
        assert!(app.messages.iter().any(|message| {
            message.content.contains("Coven daemon API mismatch")
                && message.content.contains("polling paused")
        }));

        app.input = "continue".to_string();
        app.cursor_pos = app.input.len();
        app.handle_input();

        assert_eq!(
            mirror
                .calls
                .borrow()
                .iter()
                .filter(|call| *call == "events:session-1:0")
                .count(),
            2
        );
    }

    #[test]
    fn live_streaming_appends_output_chunks_immediately() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.active_session_id = Some("session-1".to_string());
        assert!(app.streaming_mode().is_live());

        app.push_event_message(&output_event(1, "session-1", "Hello "));
        app.push_event_message(&output_event(2, "session-1", "world!\n"));

        let agent_messages: Vec<_> = app
            .messages
            .iter()
            .filter(|message| matches!(message.role, MessageRole::Agent))
            .collect();
        assert_eq!(agent_messages.len(), 1);
        assert_eq!(agent_messages[0].content, "Hello world!\n");
    }

    #[test]
    fn streamed_blank_line_chunks_keep_paragraph_breaks_in_message_body() {
        // Regression: prior to keeping newline-only chunks, splitting a reply
        // by lines and streaming each one separately erased the paragraph
        // boundaries because the blank-line events were silently dropped.
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.active_session_id = Some("session-1".to_string());

        for (idx, chunk) in ["First paragraph.\n", "\n", "Second paragraph.\n"]
            .iter()
            .enumerate()
        {
            app.push_event_message(&output_event((idx as i64) + 1, "session-1", chunk));
        }

        let agent: Vec<_> = app
            .messages
            .iter()
            .filter(|message| matches!(message.role, MessageRole::Agent))
            .collect();
        assert_eq!(agent.len(), 1);
        assert_eq!(
            agent[0].content, "First paragraph.\n\nSecond paragraph.\n",
            "the blank-line chunk between paragraphs must survive"
        );
    }

    #[test]
    fn spinner_frames_render_visible_glyphs_so_responding_never_looks_dead() {
        // Regression guard: the table was previously eight empty strings,
        // which made the status bar render "responding..." with no animation
        // at all. Real frames must carry at least one visible grapheme each.
        assert!(!SPINNER_FRAMES.is_empty());
        for (idx, frame) in SPINNER_FRAMES.iter().enumerate() {
            assert!(
                frame.chars().any(|c| !c.is_whitespace()),
                "spinner frame {idx} is blank ({frame:?}); spinner would look frozen",
            );
        }
    }

    #[test]
    fn status_bar_keeps_composing_indicator_at_eighty_columns() {
        use ratatui::{backend::TestBackend, Terminal};

        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.handle_slash_command("/stream off");
        // A realistic long cwd previously pushed the rightmost segment off the
        // status bar; the project label must yield first so the spinner +
        // (composing) tail always survives.
        app.project_label = "/Users/buns/Documents/GitHub/OpenCoven/coven".to_string();
        app.active_session_id = Some("demo-session".to_string());
        app.is_responding = true;
        app.push_event_message(&output_event(1, "demo-session", "partial reply"));
        assert!(app.has_pending_batched_output());

        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        terminal
            .draw(|frame| crate::tui::chat::render::render_ui(frame, &mut app))
            .unwrap();
        let frame = crate::tui::chat::render::buffer_to_plain_text(terminal.backend().buffer());

        assert!(
            frame.contains("stream: off"),
            "stream chip missing at 80 cols:\n{frame}"
        );
        assert!(
            frame.contains("(composing)"),
            "composing suffix clipped at 80 cols:\n{frame}"
        );
    }

    #[test]
    fn batched_streaming_holds_output_until_session_exits() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.handle_slash_command("/stream off");
        app.active_session_id = Some("session-1".to_string());
        app.is_responding = true;

        app.push_event_message(&output_event(1, "session-1", "first chunk "));
        app.push_event_message(&output_event(2, "session-1", "second chunk\n"));

        let agent_count_before_exit = app
            .messages
            .iter()
            .filter(|message| matches!(message.role, MessageRole::Agent))
            .count();
        assert_eq!(agent_count_before_exit, 0);
        assert!(app.has_pending_batched_output());

        app.push_event_message(&EventRecord {
            seq: 3,
            id: "event-3".to_string(),
            session_id: "session-1".to_string(),
            kind: "exit".to_string(),
            payload_json: serde_json::json!({ "status": "completed" }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        });

        let agent_messages: Vec<_> = app
            .messages
            .iter()
            .filter(|message| matches!(message.role, MessageRole::Agent))
            .collect();
        assert_eq!(agent_messages.len(), 1);
        assert_eq!(agent_messages[0].content, "first chunk second chunk\n");
        assert!(!app.has_pending_batched_output());
        assert!(!app.is_responding);
    }

    #[test]
    fn batched_streaming_flushes_pending_buffer_on_kill_event() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.handle_slash_command("/stream off");
        app.active_session_id = Some("session-1".to_string());
        app.is_responding = true;

        app.push_event_message(&output_event(1, "session-1", "partial work"));
        assert!(app.has_pending_batched_output());

        app.push_event_message(&EventRecord {
            seq: 2,
            id: "event-2".to_string(),
            session_id: "session-1".to_string(),
            kind: "kill".to_string(),
            payload_json: serde_json::json!({ "status": "killed" }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        });

        let agent_messages: Vec<_> = app
            .messages
            .iter()
            .filter(|message| matches!(message.role, MessageRole::Agent))
            .collect();
        assert_eq!(agent_messages.len(), 1);
        assert_eq!(agent_messages[0].content, "partial work");
    }

    #[test]
    fn turning_streaming_back_on_flushes_pending_batched_output() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.handle_slash_command("/stream off");
        app.active_session_id = Some("session-1".to_string());

        app.push_event_message(&output_event(1, "session-1", "queued reply"));
        assert!(app.has_pending_batched_output());

        app.handle_slash_command("/stream on");

        let agent_messages: Vec<_> = app
            .messages
            .iter()
            .filter(|message| matches!(message.role, MessageRole::Agent))
            .collect();
        assert_eq!(agent_messages.len(), 1);
        assert_eq!(agent_messages[0].content, "queued reply");
        assert!(!app.has_pending_batched_output());
        assert!(app.streaming_mode().is_live());
    }

    #[test]
    fn stream_slash_command_toggles_and_reports_status() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        assert!(app.streaming_mode().is_live());

        app.handle_slash_command("/stream");
        assert!(!app.streaming_mode().is_live());
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("Streaming off")));

        app.handle_slash_command("/stream status");
        assert!(app
            .messages
            .iter()
            .any(|message| message.content == "Streaming is off."));

        app.handle_slash_command("/stream on");
        assert!(app.streaming_mode().is_live());
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("Streaming on")));
    }

    #[test]
    fn stream_slash_command_rejects_unknown_argument_without_changing_mode() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        let starting_mode = app.streaming_mode();

        app.handle_slash_command("/stream please");

        assert_eq!(app.streaming_mode(), starting_mode);
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("Unknown /stream argument")));
    }

    #[test]
    fn stream_slash_is_treated_as_local_so_cast_never_intercepts_it() {
        // Regression guard: /stream must short-circuit through
        // handle_slash_command, not fall into the Cast parser (which would
        // emit a "unknown spell" message and never flip the toggle).
        assert!(is_chat_local_slash("/stream"));
        assert!(is_chat_local_slash("/stream off"));
        assert!(is_chat_local_slash("/streaming on"));
    }

    #[test]
    fn slash_popup_only_opens_when_input_is_a_slash_prefix_without_arguments() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);

        // Empty input: no popup
        assert!(!app.slash_popup_is_open());

        // Slash prefix: popup shows
        app.input = "/he".to_string();
        app.cursor_pos = app.input.len();
        assert!(app.slash_popup_is_open());
        let suggestions = app.slash_suggestions();
        assert!(suggestions.iter().any(|cmd| cmd.name == "/help"));

        // Argument started: popup closes so the user can type freely.
        app.input = "/run codex".to_string();
        app.cursor_pos = app.input.len();
        assert!(!app.slash_popup_is_open());

        // Non-slash input: no popup at all.
        app.input = "hello world".to_string();
        app.cursor_pos = app.input.len();
        assert!(!app.slash_popup_is_open());
    }

    #[test]
    fn slash_popup_filters_case_insensitively_by_prefix() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);

        app.input = "/CL".to_string();
        app.cursor_pos = app.input.len();
        let suggestions = app.slash_suggestions();
        let names: Vec<&str> = suggestions.iter().map(|cmd| cmd.name).collect();
        assert_eq!(names, vec!["/clear"]);
    }

    #[test]
    fn apply_slash_suggestion_completes_into_input_and_adds_trailing_space() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);

        app.input = "/he".to_string();
        app.cursor_pos = app.input.len();
        // First suggestion for /he* should be /help.
        let applied = app.apply_slash_suggestion();
        assert!(applied);
        assert_eq!(app.input, "/help ");
        assert_eq!(app.cursor_pos, app.input.len());
        // After completion the popup auto-closes because the input now
        // contains whitespace.
        assert!(!app.slash_popup_is_open());
    }

    #[test]
    fn apply_slash_suggestion_is_no_op_when_input_already_matches_selection() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);

        app.input = "/help".to_string();
        app.cursor_pos = app.input.len();
        // Exact match shouldn't re-complete (which would let Enter still run
        // the command normally).
        let applied = app.apply_slash_suggestion();
        assert!(!applied);
        assert_eq!(app.input, "/help");
    }

    #[test]
    fn slash_popup_navigation_wraps_around_the_filtered_list() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);

        // Typing just `/` should surface every command.
        app.input = "/".to_string();
        app.cursor_pos = app.input.len();
        let total = app.slash_suggestions().len();
        assert!(total >= 2);

        for _ in 0..total {
            app.slash_popup_select_next();
        }
        assert_eq!(app.slash_suggestion_index, 0, "next should wrap to start");

        app.slash_popup_select_prev();
        assert_eq!(
            app.slash_suggestion_index,
            total - 1,
            "prev from top should wrap to last entry",
        );
    }

    #[test]
    fn clear_transcript_drops_messages_resets_scroll_and_logs_a_marker() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.push_user_message("hello");
        app.push_agent_message("codex", "world");
        app.scroll_offset = 12;

        app.clear_transcript();

        // The only remaining message should be the "Chat cleared." marker.
        assert_eq!(app.messages.len(), 1);
        assert!(matches!(app.messages[0].role, MessageRole::System));
        assert!(app.messages[0].content.contains("Chat cleared"));
        assert_eq!(app.scroll_offset, 0);
    }

    #[test]
    fn handle_interrupt_first_press_clears_input_and_arms_exit_advisory() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.input = "in-flight prompt".to_string();
        app.cursor_pos = app.input.len();

        let outcome = app.handle_interrupt();

        assert_eq!(outcome, InterruptOutcome::Cancelled);
        assert!(app.input.is_empty());
        assert_eq!(app.cursor_pos, 0);
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("Press Ctrl+C again to exit")));
    }

    #[test]
    fn second_ctrl_c_within_window_returns_quit() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);

        assert_eq!(app.handle_interrupt(), InterruptOutcome::Cancelled);
        // Without waiting (so we stay inside the rearm window), a second
        // press should request quit.
        assert_eq!(app.handle_interrupt(), InterruptOutcome::Quit);
    }

    #[test]
    fn interrupt_with_active_session_sends_kill_to_daemon() {
        let session = test_session("abc-123", "codex", "task", "running");
        let client = RecordingChatClient::with_session(session.clone());
        let calls = client.calls.clone();
        let (mut app, _) = app_with_client(client);
        app.active_session_id = Some(session.id.clone());

        assert_eq!(app.handle_interrupt(), InterruptOutcome::Cancelled);

        let recorded = calls.borrow().clone();
        assert!(
            recorded.iter().any(|call| call == "kill:abc-123"),
            "expected kill to be sent on Ctrl+C, got: {recorded:?}",
        );
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("Interrupt sent")));
    }

    #[test]
    fn esc_path_kills_active_session_when_nothing_else_to_cancel() {
        let session = test_session("xyz-9", "claude", "task", "running");
        let client = RecordingChatClient::with_session(session.clone());
        let calls = client.calls.clone();
        let (mut app, _) = app_with_client(client);
        app.active_session_id = Some(session.id.clone());

        // Mirror the event-loop arm: with empty input and no popup, Esc
        // should reach interrupt_active_session.
        assert!(app.input.is_empty());
        assert!(!app.slash_popup_is_open());

        let interrupted = app.interrupt_active_session();
        assert!(interrupted);

        let recorded = calls.borrow().clone();
        assert!(
            recorded.iter().any(|call| call == "kill:xyz-9"),
            "expected kill call from Esc-style interrupt, got: {recorded:?}",
        );
    }

    #[test]
    fn dismissing_the_slash_popup_keeps_it_closed_until_input_edits() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);

        app.input = "/he".to_string();
        app.cursor_pos = app.input.len();
        assert!(app.slash_popup_is_open());

        app.dismiss_slash_popup();
        assert!(!app.slash_popup_is_open());

        // Typing another char should re-open it — dismissal is single-shot.
        app.insert_char('l');
        assert!(app.slash_popup_is_open());
    }
}
