//! Chat application state, behavior, and helpers. Owns `App` and all of its
//! methods; provides `discover_agents` and the spinner-frame data.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::{
    harness, store,
    tui::cast::{
        self, build_plan, parse_spell,
        plan::{CastHarnessSource, CastPlan},
        safety::{CastRisk, SafetyDecision},
        CastHarness, CastIntent,
    },
};

use super::client::{coven_home_dir, ChatClient, ChatEventQuery, DaemonChatClient, LaunchRequest};
use super::settings::{self, ChatSettings, StreamingMode};

// ── Data types ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum MessageRole {
    User,
    Agent,
    System,
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
    pub(super) sessions: Vec<store::SessionRecord>,
    pub(super) show_session_overlay: bool,
    pub(super) input_history: Vec<String>,
    pub(super) history_index: Option<usize>,
    pending_cast_confirmation: Option<CastPlan>,
    streaming_mode: StreamingMode,
    pending_agent_buffer: Option<(String, String)>,
    coven_home: Option<PathBuf>,
    client: Box<dyn ChatClient>,
}

pub(super) const SPINNER_FRAMES: &[&str] = &["", "", "", "", "", "", "", ""];

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
        let streaming_mode = coven_home
            .as_deref()
            .map(|home| settings::load_from(home).streaming)
            .unwrap_or_default();
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
            sessions: Vec::new(),
            show_session_overlay: false,
            input_history: Vec::new(),
            history_index: None,
            pending_cast_confirmation: None,
            streaming_mode,
            pending_agent_buffer: None,
            coven_home,
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

    pub(super) fn handle_input(&mut self) -> Option<SlashCommandResult> {
        let raw = self.input.trim().to_string();
        self.input.clear();
        self.cursor_pos = 0;

        if raw.is_empty() {
            return Some(SlashCommandResult::Handled);
        }

        self.event_poll_paused_for_api_mismatch = false;

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

        if let Some(session_id) = self.active_session_id.clone() {
            self.forward_input_to_session(&session_id, &raw);
        } else {
            self.launch_chat_session(&raw);
        }
        self.scroll_to_bottom();
        Some(SlashCommandResult::Handled)
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
                self.messages.clear();
                self.scroll_offset = 0;
                self.push_system_message("Chat cleared.");
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
            "/delegate" => {
                if arg.is_empty() {
                    self.push_system_message("Usage: /delegate <agent> <task>");
                } else {
                    self.push_system_message(&format!("Delegating: {arg} (coming soon)"));
                }
                SlashCommandResult::Handled
            }
            "/trace" => {
                self.push_system_message("Trace display coming soon.");
                SlashCommandResult::Handled
            }
            "/mem" => {
                if arg.is_empty() {
                    self.push_system_message("Usage: /mem <query>");
                } else {
                    self.push_system_message(&format!(
                        "Searching agent memory for \"{arg}\"... (coming soon)"
                    ));
                }
                SlashCommandResult::Handled
            }
            "/debug" => {
                self.push_system_message("Debug mode coming soon.");
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
        self.push_system_message(&format_cast_plan_for_chat(&plan));

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
        match plan.intent {
            CastIntent::NaturalSpell { prompt } | CastIntent::HarnessSpell { prompt, .. } => {
                let Some(plan_harness) = plan.harness else {
                    self.push_system_message(
                        "No harness available. Run `coven doctor` to install Codex or Claude Code.",
                    );
                    return SlashCommandResult::Handled;
                };
                if let Some(session) = self.run_harness_prompt(plan_harness.harness.id(), &prompt) {
                    self.push_system_message(&format_cast_outcome_for_chat(
                        plan_harness.harness.label(),
                        &session.id,
                    ));
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
            CastIntent::Doctor => self.push_system_message("Run `coven doctor` for setup checks."),
            CastIntent::DaemonStatus => {
                self.push_system_message("Run `coven daemon status` to inspect the local daemon.")
            }
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
        let result = LaunchRequest::for_current_dir(harness, prompt)
            .and_then(|request| self.client.launch_session(request));
        match result {
            Ok(session) => {
                self.active_session_id = Some(session.id.clone());
                self.last_event_seq = None;
                self.reset_event_poll_failures();
                self.push_system_message(&format!(
                    "Started daemon session {} ({})",
                    session.id, session.harness
                ));
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

    fn forward_input_to_session(&mut self, session_id: &str, raw: &str) {
        self.is_responding = true;
        let result = self.client.send_input(session_id, &format!("{raw}\n"));
        match result {
            Ok(()) => self.poll_session_events(),
            Err(error) => {
                self.is_responding = false;
                self.push_system_message(&format!("Input rejected: {error}"));
            }
        }
    }

    pub(super) fn attach_session(&mut self, session_id: &str) {
        match self.client.get_session(session_id) {
            Ok(session) => {
                self.active_session_id = Some(session.id.clone());
                self.last_event_seq = None;
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
                self.last_event_seq = None;
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

    fn push_event_message(&mut self, event: &store::EventRecord) {
        match event.kind.as_str() {
            "output" => {
                if let Some(data) = event_payload_text(event, "data") {
                    let sender = self.active_agent_label().to_string();
                    if let Some(text) = clean_terminal_output(&data) {
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
                if self.active_session_id.as_deref() == Some(event.session_id.as_str()) {
                    self.active_session_id = None;
                }
                self.push_system_message(&format!("Session {status}."));
            }
            "kill" => {
                self.flush_pending_agent_buffer();
                if self.active_session_id.as_deref() == Some(event.session_id.as_str()) {
                    self.active_session_id = None;
                    self.is_responding = false;
                }
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
    }

    pub(super) fn insert_str(&mut self, value: &str) {
        self.input.insert_str(self.cursor_pos, value);
        self.cursor_pos += value.len();
        self.history_index = None;
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
        }
    }

    pub(super) fn delete_char_at_cursor(&mut self) {
        if self.cursor_pos < self.input.len() {
            self.input.remove(self.cursor_pos);
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
            | "/agent"
            | "/a"
            | "/export"
            | "/exit"
            | "/quit"
            | "/q"
            | "/delegate"
            | "/trace"
            | "/mem"
            | "/debug"
            | "/stream"
            | "/streaming"
    )
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
    use crate::tui::chat::client::{ChatClient, ChatEventQuery, LaunchRequest};
    use std::cell::RefCell;
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
        event_error: Rc<RefCell<Option<String>>>,
        launch_error: Rc<RefCell<Option<String>>>,
    }

    impl RecordingChatClient {
        fn with_session(session: SessionRecord) -> Self {
            let client = Self::default();
            client.sessions.borrow_mut().push(session);
            client
        }
    }

    impl ChatClient for RecordingChatClient {
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

    #[test]
    fn unknown_slash_command_returns_command_name_for_feedback() {
        let mut app = app_with_agents(vec![agent("codex", true)]);

        match app.handle_slash_command("/unknown value") {
            SlashCommandResult::Unknown(command) => assert_eq!(command, "/unknown"),
            other => panic!("expected unknown command result, got {other:?}"),
        }
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
    fn plain_chat_input_launches_daemon_session_without_mock_response() {
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
        assert!(app
            .messages
            .iter()
            .any(|message| message.content.contains("Started daemon session")));
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
    fn plain_chat_input_appends_cast_plan_before_daemon_launch() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.input = "fix the failing tests".to_string();
        app.cursor_pos = app.input.len();

        app.handle_input();

        let plan_index = app
            .messages
            .iter()
            .position(|message| message.content.contains("Cast plan"))
            .expect("chat transcript should include Cast plan");
        let launch_index = app
            .messages
            .iter()
            .position(|message| message.content.contains("Started daemon session"))
            .expect("safe plan should launch");
        assert!(plan_index < launch_index);
        assert!(app.messages[plan_index].content.contains("risk [ SAFE ]"));
        assert!(app.messages[plan_index]
            .content
            .contains("steps  launch project-scoped session"));
    }

    #[test]
    fn safe_chat_launch_appends_cast_outcome_after_daemon_launch() {
        let client = RecordingChatClient::default();
        let (mut app, _) = app_with_client(client);
        app.input = "fix the failing tests".to_string();
        app.cursor_pos = app.input.len();

        app.handle_input();

        let launch_index = app
            .messages
            .iter()
            .position(|message| message.content.contains("Started daemon session"))
            .expect("safe plan should launch");
        let outcome_index = app
            .messages
            .iter()
            .position(|message| message.content.contains("Cast outcome"))
            .expect("chat transcript should include Cast outcome");
        assert!(launch_index < outcome_index);
        assert!(app.messages[outcome_index]
            .content
            .contains("launched  Codex daemon session"));
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
            .position(|message| message.content.contains("Started daemon session"))
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
}
