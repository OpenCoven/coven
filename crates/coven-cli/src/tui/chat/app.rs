//! Chat application state, behavior, and helpers. Owns `App` and all of its
//! methods; provides `discover_agents` and the spinner-frame data.

use std::time::{Duration, Instant};

use crate::{harness, store};

use super::client::{ChatClient, ChatEventQuery, DaemonChatClient, LaunchRequest};

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
    pub(super) sessions: Vec<store::SessionRecord>,
    pub(super) show_session_overlay: bool,
    pub(super) input_history: Vec<String>,
    pub(super) history_index: Option<usize>,
    client: Box<dyn ChatClient>,
}

pub(super) const SPINNER_FRAMES: &[&str] = &["", "", "", "", "", "", "", ""];

impl App {
    pub(super) fn new() -> Self {
        let agents = discover_agents();
        let active_agent = agents.iter().position(|a| a.available);
        Self::new_with_state(agents, active_agent, Box::<DaemonChatClient>::default())
    }

    fn new_with_state(
        agents: Vec<AgentInfo>,
        active_agent: Option<usize>,
        client: Box<dyn ChatClient>,
    ) -> Self {
        let mut app = App {
            messages: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            scroll_offset: 0,
            agents,
            active_agent,
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
            sessions: Vec::new(),
            show_session_overlay: false,
            input_history: Vec::new(),
            history_index: None,
            client,
        };

        app.push_system_message(
            "Welcome to the Coven. Type a message to chat, or /help for commands.",
        );

        if let Some(idx) = app.active_agent {
            let agent = &app.agents[idx];
            app.push_system_message(&format!(
                "Active agent: {} ({})",
                agent.label, agent.harness
            ));
        } else {
            app.push_system_message("No agents available. Run `coven doctor` to check your setup.");
        }

        app
    }

    #[cfg(test)]
    pub(super) fn new_with_client(client: Box<dyn ChatClient>) -> Self {
        let agents = discover_agents();
        let active_agent = agents.iter().position(|a| a.available);
        Self::new_with_state(agents, active_agent, client)
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

        if raw.starts_with('/') {
            return Some(self.handle_slash_command(&raw));
        }

        self.record_history(&raw);
        self.push_user_message(&raw);
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
                self.run_harness_prompt(harness, prompt);
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
            _ => SlashCommandResult::Unknown(cmd),
        }
    }

    fn launch_chat_session(&mut self, prompt: &str) {
        let Some(agent) = self
            .active_agent
            .and_then(|idx| self.agents.get(idx))
            .cloned()
        else {
            self.push_system_message(
                "No active agent. Use /agent to select one, or run `coven doctor`.",
            );
            return;
        };
        if !agent.available {
            self.push_system_message(&format!(
                "{} is not available. Run `coven doctor` to troubleshoot.",
                agent.label
            ));
            return;
        }
        self.run_harness_prompt(&agent.harness, prompt);
    }

    fn run_harness_prompt(&mut self, harness: &str, prompt: &str) {
        self.is_responding = true;
        let result = LaunchRequest::for_current_dir(harness, prompt)
            .and_then(|request| self.client.launch_session(request));
        self.is_responding = false;
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
            }
            Err(error) => self.push_system_message(&format!("Daemon launch failed: {error}")),
        }
    }

    fn forward_input_to_session(&mut self, session_id: &str, raw: &str) {
        self.is_responding = true;
        let result = self.client.send_input(session_id, &format!("{raw}\n"));
        self.is_responding = false;
        match result {
            Ok(()) => self.poll_session_events(),
            Err(error) => self.push_system_message(&format!("Input rejected: {error}")),
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
    }

    fn record_event_poll_failure(&mut self, error: anyhow::Error) {
        let message = error.to_string();
        let repeated_error = self.last_event_poll_error.as_deref() == Some(message.as_str());
        self.event_poll_failure_streak = self.event_poll_failure_streak.saturating_add(1);
        self.event_poll_backoff_until =
            Some(Instant::now() + event_poll_backoff(self.event_poll_failure_streak));
        self.last_event_poll_error = Some(message.clone());
        if !repeated_error {
            self.push_system_message(&format!("Event follow failed: {message}"));
        }
    }

    fn push_event_message(&mut self, event: &store::EventRecord) {
        match event.kind.as_str() {
            "output" => {
                if let Some(data) = event_payload_text(event, "data") {
                    let sender = self.active_agent_label().to_string();
                    self.push_agent_message(&sender, &data);
                }
            }
            "exit" => {
                let status =
                    event_payload_text(event, "status").unwrap_or_else(|| "exited".to_string());
                self.is_responding = false;
                self.push_system_message(&format!("Session {status}."));
            }
            "kill" => self.push_system_message("Session kill recorded."),
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

fn split_first_arg(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim();
    let split_idx = trimmed.find(char::is_whitespace)?;
    let first = &trimmed[..split_idx];
    let rest = trimmed[split_idx..].trim();
    (!first.is_empty() && !rest.is_empty()).then_some((first, rest))
}

fn event_payload_text(event: &store::EventRecord, field: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(&event.payload_json)
        .ok()?
        .get(field)?
        .as_str()
        .map(ToOwned::to_owned)
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

        match result {
            Some(SlashCommandResult::Unknown(command)) => assert_eq!(command, "/missing"),
            other => panic!("expected unknown command result, got {other:?}"),
        }
        assert!(app.input.is_empty());
        assert_eq!(app.cursor_pos, 0);
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
}
