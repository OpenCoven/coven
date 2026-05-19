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
    let agent_name = app.active_agent_label();
    let harness = app.active_agent_harness();
    let session = app.active_session_id().unwrap_or("no-session");

    let status_spans = vec![
        Span::styled(
            " \u{2666} coven chat ",
            theme::ratatui_style(PRIMARY).bold(),
        ),
        Span::styled(" \u{2502} ", theme::ratatui_style(DIM)),
        Span::styled(
            format!("\u{25C9} {agent_name}"),
            theme::ratatui_style(AGENT_LABEL).bold(),
        ),
        Span::styled(format!(" ({harness})"), theme::ratatui_style(DIM)),
        Span::styled(" \u{2502} ", theme::ratatui_style(DIM)),
        Span::styled(session.to_string(), theme::ratatui_style(DIM)),
        Span::styled(" \u{2502} ", theme::ratatui_style(DIM)),
        if app.is_responding {
            Span::styled(
                format!("{} responding...", SPINNER_FRAMES[app.spinner_frame]),
                theme::ratatui_style(DIM),
            )
        } else {
            Span::styled("\u{2713} ready", theme::status_style(Status::Ready))
        },
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
                    MessageRole::User => theme::ratatui_style(TEXT),
                    MessageRole::Agent => theme::ratatui_style(TEXT_DIM),
                    MessageRole::System => theme::ratatui_style(PRIMARY),
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
            Span::styled(" /help", theme::ratatui_style(HINT_KEY).bold()),
            Span::styled(" commands  ", theme::ratatui_style(HINT_LABEL)),
            Span::styled("/agent", theme::ratatui_style(HINT_KEY).bold()),
            Span::styled(" switch  ", theme::ratatui_style(HINT_LABEL)),
            Span::styled("PgUp/PgDn", theme::ratatui_style(HINT_KEY).bold()),
            Span::styled(" scroll  ", theme::ratatui_style(HINT_LABEL)),
            Span::styled("Shift+Enter", theme::ratatui_style(HINT_KEY).bold()),
            Span::styled(" newline  ", theme::ratatui_style(HINT_LABEL)),
            Span::styled("Ctrl+K", theme::ratatui_style(HINT_KEY).bold()),
            Span::styled(" palette  ", theme::ratatui_style(HINT_LABEL)),
            Span::styled("Ctrl+C", theme::ratatui_style(HINT_KEY).bold()),
            Span::styled(" quit  ", theme::ratatui_style(HINT_LABEL)),
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
mod tests {
    use super::*;

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
