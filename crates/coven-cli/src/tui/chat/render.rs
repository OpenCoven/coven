//! Chat TUI render functions. Pure view code; reads `App` state and emits
//! ratatui widgets. The entry point is `render_ui`; the other render_* fns
//! are private helpers it composes.

use ratatui::{
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::theme::{
    self, Status, AGENT_LABEL, BACKDROP, BORDER_DIM, DIM, HINT_KEY, HINT_LABEL, PRIMARY,
    PRIMARY_STRONG, SCROLL_TRACK, SURFACE, SURFACE_STRONG, TEXT, TEXT_DIM, USER_LABEL,
};

use super::app::{App, InputMode, MessageRole, SPINNER_FRAMES};

pub(super) fn render_ui(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // Guard against impossibly small terminals
    if area.width < 10 || area.height < 5 {
        let msg = Paragraph::new("Terminal too small").style(theme::ratatui_style(PRIMARY));
        f.render_widget(msg, area);
        return;
    }

    // Background fill
    f.render_widget(
        Block::default().style(Style::default().bg(theme::ratatui_color(BACKDROP))),
        area,
    );

    let input_height = input_height(app);
    let chunks = Layout::vertical([
        Constraint::Length(1), // top status bar
        Constraint::Min(6),    // chat messages
        Constraint::Length(input_height),
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

    if app.show_session_overlay {
        render_session_overlay(f, app, area);
    }
}

fn render_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let harness = app.active_agent_harness();
    let project = app.project_label();
    let daemon_status = if app.active_session_id().is_some() {
        "running"
    } else {
        "ready"
    };

    let stream_label = app.streaming_mode().status_label();
    let head_text = format!(" coven {harness} ");
    let separator = "\u{00b7} ";
    let separator_padded = " \u{00b7} ";
    let daemon_text = format!("daemon: {daemon_status}");
    let stream_text = format!("stream: {stream_label}");
    let state_text = if app.is_responding {
        let composing = if app.has_pending_batched_output() {
            " (composing)"
        } else {
            ""
        };
        format!(
            "{} responding...{composing}",
            SPINNER_FRAMES[app.spinner_frame]
        )
    } else {
        "\u{2713} ready".to_string()
    };

    // Compute the project-label budget from what the rest of the row actually
    // needs, so the rightmost segment never clips when daemon: running and the
    // batched "(composing)" suffix push the tail wider than usual.
    let fixed_width = UnicodeWidthStr::width(head_text.as_str())
        + UnicodeWidthStr::width(separator)
        + UnicodeWidthStr::width(separator_padded) * 3
        + UnicodeWidthStr::width(daemon_text.as_str())
        + UnicodeWidthStr::width(stream_text.as_str())
        + UnicodeWidthStr::width(state_text.as_str());
    let project_budget = (area.width as usize).saturating_sub(fixed_width);
    let project_text = truncate_for_width(project, project_budget);

    let state_style = if app.is_responding {
        theme::ratatui_style(DIM)
    } else {
        theme::status_style(Status::Ready)
    };

    let status_spans = vec![
        Span::styled(head_text, theme::ratatui_style(PRIMARY).bold()),
        Span::styled(separator, theme::ratatui_style(DIM)),
        Span::styled(project_text, theme::ratatui_style(DIM)),
        Span::styled(separator_padded, theme::ratatui_style(DIM)),
        Span::styled(daemon_text, theme::ratatui_style(DIM)),
        Span::styled(separator_padded, theme::ratatui_style(DIM)),
        Span::styled(stream_text, theme::ratatui_style(DIM)),
        Span::styled(separator_padded, theme::ratatui_style(DIM)),
        Span::styled(state_text, state_style),
    ];

    let status_line = Line::from(status_spans);
    let status =
        Paragraph::new(status_line).style(Style::default().bg(theme::ratatui_color(SURFACE)));
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
            MessageRole::User => (theme::ratatui_style(USER_LABEL).bold(), "\u{25B6} You"),
            MessageRole::Agent => (theme::ratatui_style(AGENT_LABEL).bold(), ""),
            MessageRole::System => (theme::ratatui_style(PRIMARY).italic(), "\u{2731} "),
        };

        let sender_text = match msg.role {
            MessageRole::User => prefix.to_string(),
            MessageRole::Agent => format!("\u{2736} {}", msg.sender),
            MessageRole::System => format!("{prefix}{}", msg.content),
        };

        if matches!(msg.role, MessageRole::System) {
            for (idx, content_line) in msg.content.lines().enumerate() {
                let prefix = if idx == 0 { "\u{2731} " } else { "  " };
                lines.push(Line::from(Span::styled(
                    format!("{prefix}{content_line}"),
                    sender_style,
                )));
            }
            continue;
        }

        lines.push(Line::from(Span::styled(sender_text, sender_style)));

        let wrap_width = if width > 4 { width - 2 } else { width };
        match msg.role {
            MessageRole::Agent => append_agent_content_lines(&mut lines, &msg.content, wrap_width),
            _ => {
                let style = match msg.role {
                    MessageRole::User => theme::ratatui_style(TEXT),
                    _ => theme::ratatui_style(PRIMARY),
                };
                for content_line in msg.content.lines() {
                    if content_line.is_empty() {
                        lines.push(Line::from(""));
                        continue;
                    }
                    for wl in textwrap::wrap(content_line, wrap_width) {
                        lines.push(Line::from(Span::styled(format!("  {wl}"), style)));
                    }
                }
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
        .style(Style::default().bg(theme::ratatui_color(BACKDROP)));

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
                .track_style(theme::ratatui_style(SCROLL_TRACK))
                .thumb_style(theme::ratatui_style(PRIMARY)),
            area,
            &mut scrollbar_state,
        );
    }
}

/// Render an agent message body with light markdown awareness so harness
/// output stays human-readable: code fences become a left-bar code block,
/// `# ` headings stand out, `- `/`* ` bullets keep their marker on the first
/// wrapped line and indent continuations under the text, and runs of blank
/// lines collapse to a single separator.
fn append_agent_content_lines<'a>(lines: &mut Vec<Line<'a>>, content: &str, wrap_width: usize) {
    let text_style = theme::ratatui_style(TEXT);
    let dim_style = theme::ratatui_style(TEXT_DIM);
    let heading_style = theme::ratatui_style(PRIMARY).bold();

    let mut in_code_block = false;
    let mut last_was_blank = true;

    for raw_line in content.lines() {
        let line = raw_line.trim_end_matches(['\r']);

        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            // Don't render the fence itself; it carried structure, not text.
            continue;
        }

        if in_code_block {
            let visible = if wrap_width > 4 && line.chars().count() > wrap_width - 4 {
                line.chars().take(wrap_width - 4).collect::<String>()
            } else {
                line.to_string()
            };
            lines.push(Line::from(vec![
                Span::styled("  \u{2502} ", dim_style),
                Span::styled(visible, text_style),
            ]));
            last_was_blank = false;
            continue;
        }

        if line.trim().is_empty() {
            if !last_was_blank {
                lines.push(Line::from(""));
                last_was_blank = true;
            }
            continue;
        }
        last_was_blank = false;

        if let Some(heading) = strip_heading_prefix(line) {
            let wrap_target = wrap_width.saturating_sub(2).max(1);
            for wl in textwrap::wrap(heading, wrap_target) {
                lines.push(Line::from(Span::styled(format!("  {wl}"), heading_style)));
            }
            continue;
        }

        if let Some((indent, marker, body)) = strip_bullet_prefix(line) {
            // Preserve the source indent so nested bullets stay visually
            // distinct, but clamp it so very deep nesting still leaves the
            // body at least two thirds of the row on narrow terminals.
            let max_indent = wrap_width.saturating_sub(6) / 3;
            let pad = " ".repeat(indent.min(max_indent));
            let indent_first = format!("  {pad}{marker}");
            let indent_cont = format!("  {pad}  ");
            let wrap_target = wrap_width
                .saturating_sub(indent_first.chars().count())
                .max(1);
            let mut wrapped = textwrap::wrap(body, wrap_target).into_iter();
            if let Some(first) = wrapped.next() {
                lines.push(Line::from(Span::styled(
                    format!("{indent_first}{first}"),
                    text_style,
                )));
            }
            for cont in wrapped {
                lines.push(Line::from(Span::styled(
                    format!("{indent_cont}{cont}"),
                    text_style,
                )));
            }
            continue;
        }

        let wrap_target = wrap_width.saturating_sub(2).max(1);
        for wl in textwrap::wrap(line, wrap_target) {
            lines.push(Line::from(Span::styled(format!("  {wl}"), text_style)));
        }
    }

    if in_code_block {
        // A fence opened mid-stream and hasn't closed yet; leave a subtle
        // marker so the reader knows the code block is still flowing.
        lines.push(Line::from(Span::styled("  \u{2502} \u{2026}", dim_style)));
    }
}

fn strip_heading_prefix(line: &str) -> Option<&str> {
    for prefix in ["#### ", "### ", "## ", "# "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    None
}

fn strip_bullet_prefix(line: &str) -> Option<(usize, &'static str, &str)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return Some((indent, "\u{2022} ", rest));
    }
    if let Some(rest) = trimmed.strip_prefix("* ") {
        return Some((indent, "\u{2022} ", rest));
    }
    None
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
            theme::ratatui_color(PRIMARY)
        } else {
            theme::ratatui_color(BORDER_DIM)
        }))
        .title(Span::styled(
            format!(" {prompt_label} "),
            theme::ratatui_style(PRIMARY).bold(),
        ))
        .style(Style::default().bg(theme::ratatui_color(SURFACE)));

    let input_widget = Paragraph::new(app.input.as_str())
        .block(input_block)
        .style(theme::ratatui_style(TEXT))
        .wrap(Wrap { trim: false });

    f.render_widget(input_widget, area);

    // Position cursor
    if area.width > 2 && area.height > 1 {
        let (cursor_line, cursor_col) = cursor_line_col(&app.input, app.cursor_pos);
        let cursor_x = area.x + 1 + cursor_col.min(area.width.saturating_sub(2) as usize) as u16;
        let cursor_y = area.y + 1 + cursor_line.min(area.height.saturating_sub(2) as usize) as u16;
        if cursor_x < area.x + area.width.saturating_sub(1)
            && cursor_y < area.y + area.height.saturating_sub(1)
        {
            f.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

fn render_hint_bar(f: &mut Frame, app: &App, area: Rect) {
    let hints = if app.input_mode == InputMode::AgentSelect {
        vec![
            Span::styled(" \u{2191}\u{2193}", theme::ratatui_style(HINT_KEY).bold()),
            Span::styled(" navigate  ", theme::ratatui_style(HINT_LABEL)),
            Span::styled("Enter", theme::ratatui_style(HINT_KEY).bold()),
            Span::styled(" select  ", theme::ratatui_style(HINT_LABEL)),
            Span::styled("Esc", theme::ratatui_style(HINT_KEY).bold()),
            Span::styled(" cancel", theme::ratatui_style(HINT_LABEL)),
        ]
    } else {
        vec![
            Span::styled(" > ", theme::ratatui_style(HINT_KEY).bold()),
            Span::styled(
                "Try \"review this branch\" or /help",
                theme::ratatui_style(HINT_LABEL),
            ),
        ]
    };

    let hint_line = Paragraph::new(Line::from(hints)).style(
        Style::default()
            .bg(theme::ratatui_color(SURFACE))
            .fg(theme::ratatui_color(HINT_LABEL)),
    );
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
                ("/palette", "Toggle this command palette"),
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
                ("/sessions", "Open daemon session overlay"),
                ("/attach <id>", "Attach to daemon session"),
                ("/run <harness> <prompt>", "Launch via daemon"),
                ("/kill [id]", "Ask daemon to kill a live session"),
            ],
        ),
        (
            "Output",
            vec![
                ("/stream", "Toggle live agent streaming (persisted)"),
                ("/stream on|off", "Force live or batched output"),
                ("/stream status", "Show current streaming mode"),
            ],
        ),
        (
            "Advanced",
            vec![
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
            theme::ratatui_style(PRIMARY_STRONG).bold(),
        )));
        for (cmd, desc) in commands {
            lines.push(Line::from(vec![
                Span::styled(format!("    {cmd:<22}"), theme::ratatui_style(PRIMARY)),
                Span::styled(*desc, theme::ratatui_style(TEXT)),
            ]));
        }
        lines.push(Line::from(""));
    }

    let help_block = Block::default()
        .title(Span::styled(
            " \u{2731} Coven Commands ",
            theme::ratatui_style(PRIMARY).bold(),
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(theme::ratatui_style(PRIMARY))
        .style(Style::default().bg(theme::ratatui_color(SURFACE)));

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
                theme::ratatui_style(PRIMARY_STRONG)
                    .bold()
                    .bg(theme::ratatui_color(SURFACE_STRONG))
            } else if !agent.available {
                theme::ratatui_style(DIM)
            } else {
                theme::ratatui_style(TEXT)
            };

            ListItem::new(Line::from(vec![
                Span::styled(format!(" {indicator} "), style),
                Span::styled(&agent.label, style),
                Span::styled(
                    format!(" ({}){availability}", agent.harness),
                    theme::ratatui_style(DIM),
                ),
            ]))
        })
        .collect();

    let agent_block = Block::default()
        .title(Span::styled(
            " \u{2736} Select Agent ",
            theme::ratatui_style(PRIMARY_STRONG).bold(),
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(theme::ratatui_style(PRIMARY_STRONG))
        .style(Style::default().bg(theme::ratatui_color(SURFACE)));

    let list = List::new(items).block(agent_block);
    f.render_widget(list, popup_area);
}

fn render_session_overlay(f: &mut Frame, app: &App, area: Rect) {
    let overlay_width = 80u16.min(area.width.saturating_sub(4));
    let overlay_height = 18u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(overlay_width)) / 2;
    let y = (area.height.saturating_sub(overlay_height)) / 2;
    let popup_area = Rect::new(x, y, overlay_width, overlay_height);

    f.render_widget(Clear, popup_area);

    let mut lines: Vec<Line<'_>> = vec![
        Line::from(Span::styled(
            "  Sessions",
            theme::ratatui_style(PRIMARY_STRONG).bold(),
        )),
        Line::from(Span::styled(
            "  /attach <id> to follow, /kill <id> to stop, r refresh, Esc close",
            theme::ratatui_style(DIM),
        )),
        Line::from(""),
    ];

    if app.sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No active sessions returned by the daemon.",
            theme::ratatui_style(DIM),
        )));
    } else {
        for session in app.sessions.iter().take(10) {
            let marker = if app.active_session_id() == Some(session.id.as_str()) {
                ">"
            } else {
                " "
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {marker} "), theme::ratatui_style(PRIMARY)),
                Span::styled(
                    format!("{:<8}", session.status),
                    theme::status_style(Status::Ready),
                ),
                Span::styled(
                    format!(" {:<7} ", session.harness),
                    theme::ratatui_style(DIM),
                ),
                Span::styled(
                    truncate_for_width(&session.id, 12),
                    theme::ratatui_style(PRIMARY),
                ),
                Span::styled("  ", theme::ratatui_style(DIM)),
                Span::styled(
                    truncate_for_width(
                        &session.title,
                        popup_area.width.saturating_sub(36) as usize,
                    ),
                    theme::ratatui_style(TEXT),
                ),
            ]));
        }
    }

    let block = Block::default()
        .title(Span::styled(
            " daemon session overlay ",
            theme::ratatui_style(PRIMARY).bold(),
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(theme::ratatui_style(PRIMARY))
        .style(Style::default().bg(theme::ratatui_color(SURFACE_STRONG)));

    let overlay = Paragraph::new(Text::from(lines))
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(overlay, popup_area);
}

fn input_height(app: &App) -> u16 {
    let line_count = input_line_count(&app.input) as u16;
    (line_count + 2).clamp(3, 8)
}

fn cursor_line_col(input: &str, cursor_pos: usize) -> (usize, usize) {
    let cursor_pos = cursor_pos.min(input.len());
    let before = &input[..cursor_pos];
    let line = before.bytes().filter(|byte| *byte == b'\n').count();
    let col = before
        .rsplit_once('\n')
        .map(|(_, tail)| UnicodeWidthStr::width(tail))
        .unwrap_or_else(|| UnicodeWidthStr::width(before));
    (line, col)
}

fn input_line_count(input: &str) -> usize {
    input.bytes().filter(|byte| *byte == b'\n').count() + 1
}

fn truncate_for_width(value: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(value) <= max_width {
        return value.to_string();
    }
    let mut output = String::new();
    for ch in value.chars() {
        let next_width = UnicodeWidthStr::width(output.as_str())
            + UnicodeWidthStr::width(ch.to_string().as_str());
        if next_width >= max_width.saturating_sub(1) {
            break;
        }
        output.push(ch);
    }
    output.push('…');
    output
}

#[cfg(test)]
pub(crate) fn render_chat_frame_plain_for_test(width: u16, height: u16) -> String {
    use ratatui::{backend::TestBackend, Terminal};

    use super::{app::AgentInfo, client::DaemonChatClient};

    let agents = vec![AgentInfo {
        id: "codex".to_string(),
        label: "codex".to_string(),
        harness: "codex".to_string(),
        available: true,
    }];
    let mut app = App::new_with_state(agents, Some(0), Box::<DaemonChatClient>::default(), None);
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render_ui(frame, &mut app))
        .expect("render chat frame");

    buffer_to_plain_text(terminal.backend().buffer())
}

#[cfg(test)]
pub(super) fn buffer_to_plain_text(buffer: &ratatui::buffer::Buffer) -> String {
    let width = buffer.area.width as usize;
    buffer
        .content()
        .chunks(width)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_first_frame_opens_on_transcript_composer_and_status() {
        let frame = render_chat_frame_plain_for_test(80, 20);

        assert!(frame.contains("coven codex"));
        assert!(frame.contains("Ready. Type a task or /help."));
        assert!(frame.contains("> Try \"review this branch\" or /help"));
        assert!(!frame.contains("Commands"));
        assert!(!frame.contains("/start"));
        assert!(!frame.contains("Session browser"));
    }

    #[test]
    fn status_bar_advertises_current_streaming_mode() {
        let frame = render_chat_frame_plain_for_test(80, 20);
        assert!(frame.contains("stream: live"));
    }

    #[test]
    fn agent_lines_render_fenced_code_blocks_with_bar_prefix() {
        let mut lines: Vec<Line<'_>> = Vec::new();
        let content = "Run this:\n```\ncargo test\n```\nDone.";
        append_agent_content_lines(&mut lines, content, 40);

        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.to_string()).collect())
            .collect();
        let joined = rendered.join("|");

        assert!(joined.contains("  Run this:"));
        assert!(rendered
            .iter()
            .any(|line| line.contains("\u{2502} cargo test")));
        assert!(!joined.contains("```"));
        assert!(joined.contains("  Done."));
    }

    #[test]
    fn agent_lines_promote_markdown_headings_and_bullets() {
        let mut lines: Vec<Line<'_>> = Vec::new();
        let content = "# Title\n\n- first\n- second item that wraps onto another line";
        append_agent_content_lines(&mut lines, content, 30);

        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.to_string()).collect())
            .collect();

        assert!(rendered.iter().any(|line| line.contains("  Title")));
        assert!(rendered.iter().any(|line| line.contains("\u{2022} first")));
        assert!(rendered.iter().any(|line| line.contains("\u{2022} second")));
        // Wrapped continuation must be indented under the bullet body.
        let bullet_idx = rendered
            .iter()
            .position(|line| line.contains("\u{2022} second"))
            .expect("bullet line present");
        let continuation = &rendered[bullet_idx + 1];
        assert!(continuation.starts_with("    "));
    }

    #[test]
    fn agent_lines_collapse_runs_of_blank_lines_to_a_single_separator() {
        let mut lines: Vec<Line<'_>> = Vec::new();
        let content = "First paragraph.\n\n\n\nSecond paragraph.";
        append_agent_content_lines(&mut lines, content, 40);

        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.to_string()).collect())
            .collect();
        let blanks_between = rendered
            .windows(2)
            .filter(|pair| pair[0].trim().is_empty() && pair[1].trim().is_empty())
            .count();
        assert_eq!(blanks_between, 0);
        assert!(rendered
            .iter()
            .any(|line| line.contains("First paragraph.")));
        assert!(rendered
            .iter()
            .any(|line| line.contains("Second paragraph.")));
    }

    #[test]
    fn agent_lines_preserve_bullet_nesting_indent_and_never_leak_raw_markers() {
        // Six levels of indent, 2 spaces per level. Previously the renderer
        // capped at indent > 4, which flattened the first three levels onto
        // one row and dropped levels 4+ through to plain-text rendering that
        // leaked the raw `- ` markers. After the fix, every level gets its
        // own visual indent and every bullet renders with the `•` marker.
        let mut lines: Vec<Line<'_>> = Vec::new();
        let content = "\
- L0 root
  - L1 child
    - L2 grandchild
      - L3 deep
        - L4 deeper
          - L5 deepest";
        append_agent_content_lines(&mut lines, content, 80);

        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.to_string()).collect())
            .collect();

        let expected_pairs = [
            (0usize, "L0 root"),
            (2, "L1 child"),
            (4, "L2 grandchild"),
            (6, "L3 deep"),
            (8, "L4 deeper"),
            (10, "L5 deepest"),
        ];
        for (indent, body) in expected_pairs {
            let pad = " ".repeat(indent);
            let needle = format!("  {pad}\u{2022} {body}");
            assert!(
                rendered.iter().any(|line| line == &needle),
                "missing nested bullet at indent {indent} for {body:?}; got:\n{rendered:#?}"
            );
        }

        // Raw markdown markers must never leak into the rendered output —
        // every list item should have been converted to a `•` bullet.
        for line in &rendered {
            assert!(
                !line.trim_start().starts_with("- "),
                "raw `- ` marker leaked: {line:?}"
            );
            assert!(
                !line.trim_start().starts_with("* "),
                "raw `* ` marker leaked: {line:?}"
            );
        }
    }

    #[test]
    fn agent_lines_clamp_runaway_bullet_indent_on_narrow_terminals() {
        // At wrap_width=20 the clamp should prevent first-line indent from
        // ever consuming more than ~two thirds of the row, so the body still
        // has room. wrap_width=20 → max_indent = (20-6)/3 = 4.
        let mut lines: Vec<Line<'_>> = Vec::new();
        let content = "                  - very deeply indented bullet";
        append_agent_content_lines(&mut lines, content, 20);

        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.to_string()).collect())
            .collect();

        // First-line indent should be "  " + 4 pad spaces + "• " = 8 chars.
        let first_line = rendered.first().expect("at least one line emitted");
        assert!(
            first_line.starts_with("      \u{2022} "),
            "deep indent did not clamp; line was {first_line:?}"
        );
        // No raw `- ` left behind.
        assert!(!first_line.contains("- "));
    }

    #[test]
    fn agent_lines_emit_unterminated_code_block_marker_during_streaming() {
        let mut lines: Vec<Line<'_>> = Vec::new();
        // Mid-stream chunk: fence opened but closing fence hasn't arrived yet.
        append_agent_content_lines(&mut lines, "```\ncargo run", 40);

        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.to_string()).collect())
            .collect();
        assert!(rendered
            .iter()
            .any(|line| line.contains("\u{2502} cargo run")));
        // Last rendered line should hint that more code is still flowing.
        assert!(rendered
            .last()
            .map(|line| line.contains('\u{2026}'))
            .unwrap_or(false));
    }

    #[test]
    fn cursor_position_counts_trailing_newline_as_next_line() {
        assert_eq!(cursor_line_col("first\n", "first\n".len()), (1, 0));
    }

    #[test]
    fn input_line_count_includes_trailing_empty_line() {
        assert_eq!(input_line_count("first\nsecond\n"), 3);
        assert_eq!(input_line_count(""), 1);
    }
}
