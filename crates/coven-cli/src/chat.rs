use std::io::{self, stdout};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
    Frame, Terminal,
};

use crate::harness;

// ── OpenCoven palette (256-color) ──────────────────────────────────────────

const PURPLE: Color = Color::Indexed(141); // #af87ff — signature purple
const GOLD: Color = Color::Indexed(220); // #ffd700 — accent gold
const MOON: Color = Color::Indexed(117); // #87d7ff — cool accent
const DIM_FG: Color = Color::Indexed(243); // muted gray
const SURFACE: Color = Color::Indexed(235); // dark surface
const SURFACE_LIGHT: Color = Color::Indexed(237); // slightly lighter surface

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
enum InputMode {
    Normal,
    AgentSelect,
}

#[derive(Clone, Debug)]
enum SlashCommandResult {
    Handled,
    Quit,
    #[allow(dead_code)]
    Unknown(String),
}

// ── App state ──────────────────────────────────────────────────────────────

struct App {
    messages: Vec<ChatMessage>,
    input: String,
    cursor_pos: usize,
    scroll_offset: usize,
    agents: Vec<AgentInfo>,
    active_agent: Option<usize>,
    input_mode: InputMode,
    agent_select_index: usize,
    show_help: bool,
    spinner_frame: usize,
    is_responding: bool,
    last_tick: Instant,
}

const SPINNER_FRAMES: &[&str] = &["", "", "", "", "", "", "", ""];

impl App {
    fn new() -> Self {
        let agents = discover_agents();
        let active_agent = agents.iter().position(|a| a.available);

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

    fn push_system_message(&mut self, content: &str) {
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

    fn active_agent_label(&self) -> &str {
        self.active_agent
            .and_then(|idx| self.agents.get(idx))
            .map(|a| a.label.as_str())
            .unwrap_or("none")
    }

    fn active_agent_harness(&self) -> &str {
        self.active_agent
            .and_then(|idx| self.agents.get(idx))
            .map(|a| a.harness.as_str())
            .unwrap_or("—")
    }

    fn handle_input(&mut self) -> Option<SlashCommandResult> {
        let raw = self.input.trim().to_string();
        self.input.clear();
        self.cursor_pos = 0;

        if raw.is_empty() {
            return Some(SlashCommandResult::Handled);
        }

        if raw.starts_with('/') {
            return Some(self.handle_slash_command(&raw));
        }

        // Regular chat message
        self.push_user_message(&raw);
        self.simulate_agent_response(&raw);
        self.scroll_to_bottom();
        Some(SlashCommandResult::Handled)
    }

    fn handle_slash_command(&mut self, input: &str) -> SlashCommandResult {
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
                self.push_system_message(
                    "Session management coming soon. Use `coven sessions` in another terminal.",
                );
                SlashCommandResult::Handled
            }
            "/attach" => {
                if arg.is_empty() {
                    self.push_system_message("Usage: /attach <session-id>");
                } else {
                    self.push_system_message(&format!(
                        "Attaching to session {arg}... (coming soon)"
                    ));
                }
                SlashCommandResult::Handled
            }
            "/export" => {
                self.export_chat();
                SlashCommandResult::Handled
            }
            "/run" => {
                if arg.is_empty() {
                    self.push_system_message("Usage: /run <harness> <prompt>");
                } else {
                    self.push_system_message(&format!("Running: {arg} (coming soon)"));
                }
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

    fn switch_agent_by_name(&mut self, name: &str) {
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

    fn switch_agent_by_index(&mut self, idx: usize) {
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

    fn simulate_agent_response(&mut self, user_msg: &str) {
        // MVP: show a placeholder response. Real streaming comes in v0.2.
        let agent_name = self.active_agent_label().to_string();
        if self.active_agent.is_none() {
            self.push_system_message(
                "No active agent. Use /agent to select one, or run `coven doctor`.",
            );
            return;
        }

        self.push_agent_message(
            &agent_name,
            &format!(
                "I received your message: \"{}\"\n\n\
                 (This is a placeholder response. Real agent streaming will connect \
                 to the Coven daemon via the session API in v0.2.)\n\n\
                 To actually run a task, use:\n  \
                 coven run {} \"{}\"",
                truncate_str(user_msg, 80),
                self.active_agent_harness(),
                truncate_str(user_msg, 60),
            ),
        );
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

    fn scroll_to_bottom(&mut self) {
        // Will be calculated during render based on content height
        self.scroll_offset = usize::MAX;
    }

    fn tick(&mut self) {
        if self.last_tick.elapsed() >= Duration::from_millis(120) {
            self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
            self.last_tick = Instant::now();
        }
    }

    fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
    }

    fn delete_char_before_cursor(&mut self) {
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

    fn delete_char_at_cursor(&mut self) {
        if self.cursor_pos < self.input.len() {
            self.input.remove(self.cursor_pos);
        }
    }

    fn move_cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            let prev = self.input[..self.cursor_pos]
                .chars()
                .last()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor_pos -= prev;
        }
    }

    fn move_cursor_right(&mut self) {
        if self.cursor_pos < self.input.len() {
            let next = self.input[self.cursor_pos..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor_pos += next;
        }
    }

    fn move_cursor_home(&mut self) {
        self.cursor_pos = 0;
    }

    fn move_cursor_end(&mut self) {
        self.cursor_pos = self.input.len();
    }

    fn delete_word_before_cursor(&mut self) {
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
}

// ── Discover agents from built-in harnesses ────────────────────────────────

fn discover_agents() -> Vec<AgentInfo> {
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

// ── Rendering ──────────────────────────────────────────────────────────────

fn render_ui(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // Guard against impossibly small terminals
    if area.width < 10 || area.height < 5 {
        let msg = Paragraph::new("Terminal too small").style(Style::default().fg(PURPLE));
        f.render_widget(msg, area);
        return;
    }

    // Background fill
    f.render_widget(
        Block::default().style(Style::default().bg(Color::Black)),
        area,
    );

    // Main layout: status bar (1) + chat area + input area (3)
    let chunks = Layout::vertical([
        Constraint::Length(1), // top status bar
        Constraint::Min(6),    // chat messages
        Constraint::Length(3), // input
        Constraint::Length(1), // bottom hint bar
    ])
    .split(area);

    render_status_bar(f, app, chunks[0]);
    render_messages(f, app, chunks[1]);
    render_input(f, app, chunks[2]);
    render_hint_bar(f, app, chunks[3]);

    if app.show_help {
        render_help_overlay(f, area);
    }

    if app.input_mode == InputMode::AgentSelect {
        render_agent_select(f, app, area);
    }
}

fn render_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let agent_name = app.active_agent_label();
    let harness = app.active_agent_harness();

    let status_spans = vec![
        Span::styled(" \u{2666} coven chat ", Style::default().fg(PURPLE).bold()),
        Span::styled(" \u{2502} ", Style::default().fg(DIM_FG)),
        Span::styled(
            format!("\u{25C9} {agent_name}"),
            Style::default().fg(GOLD).bold(),
        ),
        Span::styled(format!(" ({harness})"), Style::default().fg(DIM_FG)),
        Span::styled(" \u{2502} ", Style::default().fg(DIM_FG)),
        if app.is_responding {
            Span::styled(
                format!("{} responding...", SPINNER_FRAMES[app.spinner_frame]),
                Style::default().fg(MOON),
            )
        } else {
            Span::styled("\u{2713} ready", Style::default().fg(Color::Green))
        },
    ];

    let status_line = Line::from(status_spans);
    let status = Paragraph::new(status_line).style(Style::default().bg(SURFACE));
    f.render_widget(status, area);
}

fn render_messages(f: &mut Frame, app: &mut App, area: Rect) {
    let inner = area.inner(Margin::new(1, 0));
    let width = inner.width as usize;
    if width == 0 {
        return;
    }

    // Build rendered lines from messages
    let mut lines: Vec<Line<'_>> = Vec::new();

    for msg in &app.messages {
        // Blank line between messages (except first)
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }

        // Sender header
        let (sender_style, prefix) = match msg.role {
            MessageRole::User => (Style::default().fg(MOON).bold(), "\u{25B6} You"),
            MessageRole::Agent => (Style::default().fg(GOLD).bold(), ""),
            MessageRole::System => (Style::default().fg(PURPLE).italic(), "\u{2731} "),
        };

        let sender_text = match msg.role {
            MessageRole::User => prefix.to_string(),
            MessageRole::Agent => format!("\u{2736} {}", msg.sender),
            MessageRole::System => format!("{prefix}{}", msg.content),
        };

        if matches!(msg.role, MessageRole::System) {
            // System messages are single-line
            lines.push(Line::from(Span::styled(sender_text, sender_style)));
            continue;
        }

        lines.push(Line::from(Span::styled(sender_text, sender_style)));

        // Message content with simple word wrapping
        let wrap_width = if width > 4 { width - 2 } else { width };
        for content_line in msg.content.lines() {
            if content_line.is_empty() {
                lines.push(Line::from(""));
                continue;
            }
            let wrapped = textwrap::wrap(content_line, wrap_width);
            for wl in wrapped {
                let style = match msg.role {
                    MessageRole::User => Style::default().fg(Color::White),
                    MessageRole::Agent => Style::default().fg(Color::Indexed(252)),
                    MessageRole::System => Style::default().fg(PURPLE),
                };
                lines.push(Line::from(Span::styled(format!("  {wl}"), style)));
            }
        }
    }

    let total_lines = lines.len();
    let visible_height = inner.height as usize;

    // Auto-scroll to bottom
    if app.scroll_offset == usize::MAX || app.scroll_offset + visible_height > total_lines {
        app.scroll_offset = total_lines.saturating_sub(visible_height);
    }

    let visible_lines: Vec<Line<'_>> = lines
        .into_iter()
        .skip(app.scroll_offset)
        .take(visible_height)
        .collect();

    let chat_block = Block::default()
        .borders(Borders::NONE)
        .style(Style::default().bg(Color::Black));

    let messages_widget = Paragraph::new(Text::from(visible_lines))
        .block(chat_block)
        .wrap(Wrap { trim: false });

    f.render_widget(messages_widget, inner);

    // Scrollbar
    if total_lines > visible_height {
        let mut scrollbar_state = ScrollbarState::new(total_lines.saturating_sub(visible_height))
            .position(app.scroll_offset);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("\u{2502}"))
                .thumb_symbol("\u{2588}")
                .track_style(Style::default().fg(Color::Indexed(236)))
                .thumb_style(Style::default().fg(PURPLE)),
            area,
            &mut scrollbar_state,
        );
    }
}

fn render_input(f: &mut Frame, app: &App, area: Rect) {
    let prompt_label = if app.input.starts_with('/') {
        "\u{2731} cmd"
    } else {
        "\u{25B6} chat"
    };

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if app.input.starts_with('/') {
            PURPLE
        } else {
            Color::Indexed(240)
        }))
        .title(Span::styled(
            format!(" {prompt_label} "),
            Style::default().fg(PURPLE).bold(),
        ))
        .style(Style::default().bg(SURFACE));

    let input_widget = Paragraph::new(app.input.as_str())
        .block(input_block)
        .style(Style::default().fg(Color::White));

    f.render_widget(input_widget, area);

    // Position cursor
    if area.width > 2 && area.height > 1 {
        let cursor_x = area.x + 1 + app.cursor_pos as u16;
        let cursor_y = area.y + 1;
        if cursor_x < area.x + area.width.saturating_sub(1) {
            f.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

fn render_hint_bar(f: &mut Frame, app: &App, area: Rect) {
    let hints = if app.input_mode == InputMode::AgentSelect {
        vec![
            Span::styled(" \u{2191}\u{2193}", Style::default().fg(GOLD)),
            Span::styled(" navigate  ", Style::default().fg(DIM_FG)),
            Span::styled("Enter", Style::default().fg(GOLD)),
            Span::styled(" select  ", Style::default().fg(DIM_FG)),
            Span::styled("Esc", Style::default().fg(GOLD)),
            Span::styled(" cancel", Style::default().fg(DIM_FG)),
        ]
    } else {
        vec![
            Span::styled(" /help", Style::default().fg(GOLD)),
            Span::styled(" commands  ", Style::default().fg(DIM_FG)),
            Span::styled("/agent", Style::default().fg(GOLD)),
            Span::styled(" switch  ", Style::default().fg(DIM_FG)),
            Span::styled("Ctrl+C", Style::default().fg(GOLD)),
            Span::styled(" quit  ", Style::default().fg(DIM_FG)),
            Span::styled("PgUp/PgDn", Style::default().fg(GOLD)),
            Span::styled(" scroll", Style::default().fg(DIM_FG)),
        ]
    };

    let hint_line =
        Paragraph::new(Line::from(hints)).style(Style::default().bg(SURFACE).fg(DIM_FG));
    f.render_widget(hint_line, area);
}

fn render_help_overlay(f: &mut Frame, area: Rect) {
    let overlay_width = 60u16.min(area.width.saturating_sub(4));
    let overlay_height = 22u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(overlay_width)) / 2;
    let y = (area.height.saturating_sub(overlay_height)) / 2;
    let popup_area = Rect::new(x, y, overlay_width, overlay_height);

    f.render_widget(Clear, popup_area);

    let help_items = vec![
        (
            "Basics",
            vec![
                ("/help, /h", "Toggle this help overlay"),
                ("/clear, /cls", "Clear chat history"),
                ("/exit, /quit, /q", "Exit Coven chat"),
                ("/export", "Save conversation to ~/.coven/exports/"),
            ],
        ),
        (
            "Agents",
            vec![
                ("/agent", "Open agent picker"),
                ("/agent <name>", "Switch to named agent"),
            ],
        ),
        (
            "Sessions",
            vec![
                ("/session <id>", "Attach to session (coming soon)"),
                ("/attach <id>", "Attach to agent task (coming soon)"),
            ],
        ),
        (
            "Advanced",
            vec![
                ("/run <cmd>", "Execute command (coming soon)"),
                ("/delegate <a> <t>", "Queue task for agent (coming soon)"),
                ("/trace", "Show execution trace (coming soon)"),
                ("/mem <query>", "Search agent memory (coming soon)"),
                ("/debug", "Toggle debug mode (coming soon)"),
            ],
        ),
    ];

    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(""));

    for (section, commands) in &help_items {
        lines.push(Line::from(Span::styled(
            format!("  {section}"),
            Style::default().fg(GOLD).bold(),
        )));
        for (cmd, desc) in commands {
            lines.push(Line::from(vec![
                Span::styled(format!("    {cmd:<22}"), Style::default().fg(PURPLE)),
                Span::styled(*desc, Style::default().fg(Color::White)),
            ]));
        }
        lines.push(Line::from(""));
    }

    let help_block = Block::default()
        .title(Span::styled(
            " \u{2731} Coven Commands ",
            Style::default().fg(PURPLE).bold(),
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PURPLE))
        .style(Style::default().bg(SURFACE));

    let help_widget = Paragraph::new(Text::from(lines))
        .block(help_block)
        .wrap(Wrap { trim: false });

    f.render_widget(help_widget, popup_area);
}

fn render_agent_select(f: &mut Frame, app: &App, area: Rect) {
    let popup_width = 44u16.min(area.width.saturating_sub(4));
    let popup_height = (app.agents.len() as u16 + 4).min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = app
        .agents
        .iter()
        .enumerate()
        .map(|(i, agent)| {
            let is_active = app.active_agent == Some(i);
            let is_selected = app.agent_select_index == i;

            let indicator = if is_active { "\u{25C9}" } else { "\u{25CB}" };
            let availability = if agent.available {
                ""
            } else {
                " [unavailable]"
            };

            let style = if is_selected {
                Style::default().fg(GOLD).bold().bg(SURFACE_LIGHT)
            } else if !agent.available {
                Style::default().fg(DIM_FG)
            } else {
                Style::default().fg(Color::White)
            };

            ListItem::new(Line::from(vec![
                Span::styled(format!(" {indicator} "), style),
                Span::styled(&agent.label, style),
                Span::styled(
                    format!(" ({}){availability}", agent.harness),
                    Style::default().fg(DIM_FG),
                ),
            ]))
        })
        .collect();

    let agent_block = Block::default()
        .title(Span::styled(
            " \u{2736} Select Agent ",
            Style::default().fg(GOLD).bold(),
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(GOLD))
        .style(Style::default().bg(SURFACE));

    let list = List::new(items).block(agent_block);
    f.render_widget(list, popup_area);
}

// ── Event loop ─────────────────────────────────────────────────────────────

pub fn run_chat() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let result = run_event_loop(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|f| render_ui(f, app))?;

        // Poll with timeout for spinner animation
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if app.input_mode == InputMode::AgentSelect {
                        match key.code {
                            KeyCode::Up if app.agent_select_index > 0 => {
                                app.agent_select_index -= 1;
                            }
                            KeyCode::Down if app.agent_select_index + 1 < app.agents.len() => {
                                app.agent_select_index += 1;
                            }
                            KeyCode::Enter => {
                                let idx = app.agent_select_index;
                                app.switch_agent_by_index(idx);
                            }
                            KeyCode::Esc => {
                                app.input_mode = InputMode::Normal;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if app.show_help {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') => {
                                app.show_help = false;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    match key.code {
                        KeyCode::Enter => match app.handle_input() {
                            Some(SlashCommandResult::Quit) => return Ok(()),
                            Some(SlashCommandResult::Unknown(cmd)) => {
                                app.push_system_message(&format!("Unknown command: {cmd}"));
                            }
                            _ => {}
                        },
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(());
                        }
                        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.delete_word_before_cursor();
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.input.clear();
                            app.cursor_pos = 0;
                        }
                        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.move_cursor_home();
                        }
                        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.move_cursor_end();
                        }
                        KeyCode::Char(c) => {
                            app.insert_char(c);
                        }
                        KeyCode::Backspace => {
                            app.delete_char_before_cursor();
                        }
                        KeyCode::Delete => {
                            app.delete_char_at_cursor();
                        }
                        KeyCode::Left => {
                            app.move_cursor_left();
                        }
                        KeyCode::Right => {
                            app.move_cursor_right();
                        }
                        KeyCode::Home => {
                            app.move_cursor_home();
                        }
                        KeyCode::End => {
                            app.move_cursor_end();
                        }
                        KeyCode::PageUp => {
                            let page = terminal.size()?.height.saturating_sub(6) as usize;
                            app.scroll_offset = app.scroll_offset.saturating_sub(page);
                        }
                        KeyCode::PageDown => {
                            let page = terminal.size()?.height.saturating_sub(6) as usize;
                            app.scroll_offset = app.scroll_offset.saturating_add(page);
                            // Will be clamped during render
                        }
                        KeyCode::Esc if !app.input.is_empty() => {
                            app.input.clear();
                            app.cursor_pos = 0;
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => {
                    use crossterm::event::MouseEventKind;
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            app.scroll_offset = app.scroll_offset.saturating_sub(3);
                        }
                        MouseEventKind::ScrollDown => {
                            app.scroll_offset = app.scroll_offset.saturating_add(3);
                        }
                        _ => {}
                    }
                }
                Event::Resize(..) => {
                    // Terminal will redraw on next loop
                }
                _ => {}
            }
        }

        app.tick();
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn timestamp_now() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
        &s[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn app_with_agents(agents: Vec<AgentInfo>) -> App {
        let active_agent = agents.iter().position(|agent| agent.available);
        App {
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
        }
    }

    fn agent(id: &str, available: bool) -> AgentInfo {
        AgentInfo {
            id: id.to_string(),
            label: id.to_string(),
            harness: id.to_string(),
            available,
        }
    }

    #[test]
    fn chat_module_stays_single_file_to_avoid_rust_module_ambiguity() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let split_module_dir = src.join("chat");
        let split_rust_files = if split_module_dir.exists() {
            std::fs::read_dir(&split_module_dir)
                .unwrap()
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        assert!(src.join("chat.rs").is_file());
        assert!(
            !split_module_dir.join("mod.rs").exists(),
            "keep chat in src/chat.rs; src/chat/mod.rs makes `mod chat;` ambiguous"
        );
        assert!(
            split_rust_files.is_empty(),
            "legacy split chat module files must not be restored: {split_rust_files:?}"
        );
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
}
