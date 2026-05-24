//! Chat TUI render functions. Pure view code; reads `App` state and emits
//! ratatui widgets. The entry point is `render_ui`; the other render_* fns
//! are private helpers it composes.

use ratatui::{
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::{
    self, Status, AGENT_LABEL, BACKDROP, BORDER_DIM, DIM, HINT_KEY, HINT_LABEL, PRIMARY,
    PRIMARY_STRONG, SCROLL_TRACK, SURFACE, SURFACE_STRONG, TEXT, TEXT_DIM, USER_LABEL,
};

use super::app::{App, InputMode, MessageRole, SPINNER_FRAMES};
use super::highlight;

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

    // Slash popup floats just above the input box so it never overlaps the
    // composer. Drawn before help/session overlays so those still take
    // precedence when both would be visible.
    if app.slash_popup_is_open() {
        render_slash_popup(f, app, chunks[2]);
    }

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

    let mut in_code_block = false;
    let mut code_lang: Option<highlight::Lang> = None;
    let mut code_state = highlight::TokenizerState::default();
    let mut last_was_blank = true;

    for raw_line in content.lines() {
        let line = raw_line.trim_end_matches(['\r']);

        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            if !in_code_block {
                // Opening fence — language tag is the first whitespace-delimited
                // word after the backticks (e.g. ```rust, ```ts {filename=…}).
                let tag = trimmed.trim_start_matches('`');
                code_lang = tag
                    .split_whitespace()
                    .next()
                    .and_then(highlight::tokenizer_for);
                // Fresh comment/string state per block — never let one block
                // bleed into the next.
                code_state = highlight::TokenizerState::default();
            } else {
                code_lang = None;
            }
            in_code_block = !in_code_block;
            // Don't render the fence itself; it carried structure, not text.
            continue;
        }

        if in_code_block {
            let visible = truncate_for_width(line, wrap_width.saturating_sub(4));
            // Code blocks are verbatim — no inline-markdown parsing inside.
            // With a known language tag, hand the visible line to the syntax
            // tokenizer so keywords/strings/numbers/comments pick up brand
            // tints; without one, fall back to a single text-styled span.
            let mut spans = vec![Span::styled("  \u{2502} ", dim_style)];
            if let Some(lang) = code_lang {
                spans.extend(highlight::highlight_line(
                    &visible,
                    lang,
                    text_style,
                    &mut code_state,
                ));
            } else {
                spans.push(Span::styled(visible, text_style));
            }
            lines.push(Line::from(spans));
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

        if let Some((level, heading)) = strip_heading_prefix(line) {
            let style = heading_style_for(level);
            let wrap_target = wrap_width.saturating_sub(2).max(1);
            for wl in textwrap::wrap(heading, wrap_target) {
                let mut spans = vec![Span::styled("  ", style)];
                spans.extend(parse_inline_markdown(&wl, style));
                lines.push(Line::from(spans));
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
                let mut spans = vec![Span::styled(indent_first.clone(), text_style)];
                spans.extend(parse_inline_markdown(&first, text_style));
                lines.push(Line::from(spans));
            }
            for cont in wrapped {
                let mut spans = vec![Span::styled(indent_cont.clone(), text_style)];
                spans.extend(parse_inline_markdown(&cont, text_style));
                lines.push(Line::from(spans));
            }
            continue;
        }

        if is_table_row(line) {
            // Tables only survive when cell alignment is preserved; wrapping
            // a row fragments the pipes and trashes the columns. Truncate
            // with `\u{2026}` so the row stays on one visual line — losing
            // the right edge is far easier to read than losing the columns.
            // Table cells render verbatim (no inline parsing) so `|`, `*`,
            // and `` ` `` inside cells stay literal.
            let body = line.trim_end();
            let visible_budget = wrap_width.saturating_sub(2).max(1);
            let visible = truncate_for_width(body, visible_budget);
            lines.push(Line::from(Span::styled(format!("  {visible}"), text_style)));
            continue;
        }

        let wrap_target = wrap_width.saturating_sub(2).max(1);
        for wl in textwrap::wrap(line, wrap_target) {
            let mut spans = vec![Span::styled("  ", text_style)];
            spans.extend(parse_inline_markdown(&wl, text_style));
            lines.push(Line::from(spans));
        }
    }

    if in_code_block {
        // A fence opened mid-stream and hasn't closed yet; leave a subtle
        // marker so the reader knows the code block is still flowing.
        lines.push(Line::from(Span::styled("  \u{2502} \u{2026}", dim_style)));
    }
}

/// Map a heading level (1..=4) to a distinct style so the visual hierarchy
/// reads cleanly even when the terminal can't render the brand purple
/// (NoColor / piped output). Modifier mix carries the hierarchy on its own:
/// H1 is bold + underlined, H2 is bold, H3 is bold + italic, H4 is italic.
fn heading_style_for(level: u8) -> Style {
    match level {
        1 => theme::ratatui_style(PRIMARY).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        2 => theme::ratatui_style(PRIMARY).add_modifier(Modifier::BOLD),
        3 => theme::ratatui_style(PRIMARY_STRONG).add_modifier(Modifier::BOLD | Modifier::ITALIC),
        _ => theme::ratatui_style(TEXT_DIM).add_modifier(Modifier::ITALIC),
    }
}

fn strip_heading_prefix(line: &str) -> Option<(u8, &str)> {
    for (prefix, level) in [("#### ", 4u8), ("### ", 3), ("## ", 2), ("# ", 1)] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some((level, rest));
        }
    }
    None
}

fn strip_bullet_prefix(line: &str) -> Option<(usize, &'static str, &str)> {
    let trimmed = line.trim_start();
    let indent = leading_whitespace_columns(line, line.len() - trimmed.len());
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return Some((indent, "\u{2022} ", rest));
    }
    if let Some(rest) = trimmed.strip_prefix("* ") {
        return Some((indent, "\u{2022} ", rest));
    }
    None
}

fn leading_whitespace_columns(line: &str, byte_len: usize) -> usize {
    line[..byte_len]
        .chars()
        .map(|ch| match ch {
            '\t' => 4,
            ' ' => 1,
            _ => ch.width().unwrap_or(1),
        })
        .sum()
}

/// Lines beginning with `|` (after any leading indent) look like markdown
/// table rows. They have to render unwrapped so column boundaries survive;
/// wrapping at width turns the table into pipe-and-dash spaghetti.
fn is_table_row(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// Walk a single line of text and split it into styled spans so common
/// markdown inline markers — `` `code` ``, `**bold**`, and `*italic*` —
/// render with the right modifier instead of leaving their literal
/// punctuation in the chat. The parser is intentionally lite: it never
/// nests across lines, ignores escapes, and only treats `*`/`**` as
/// emphasis when the marker is hugging a non-whitespace char (so
/// arithmetic like `2 * 3 * 4` and casual mentions of asterisks survive
/// as literal text). Inline `` ` `` always wins over `*` so a paragraph
/// like `` `*foo*` `` renders as inline code, not as code-then-italic.
fn parse_inline_markdown<'a>(text: &str, default_style: Style) -> Vec<Span<'a>> {
    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    while i < text.len() {
        let rest = &text[i..];

        if let Some(byte) = rest.as_bytes().first() {
            if *byte == b'`' {
                if let Some(end) = rest[1..].find('`') {
                    flush_inline_buf(&mut spans, &mut buf, default_style);
                    let body = rest[1..1 + end].to_string();
                    let code_style = theme::ratatui_style(TEXT_DIM).add_modifier(Modifier::ITALIC);
                    spans.push(Span::styled(body, code_style));
                    i += 1 + end + 1;
                    continue;
                }
            }

            if let Some(after) = rest.strip_prefix("**") {
                let opens_on_word = after
                    .chars()
                    .next()
                    .is_some_and(|c| !c.is_whitespace() && c != '*');
                if opens_on_word {
                    if let Some(end) = after.find("**") {
                        let body = &after[..end];
                        let closes_on_word =
                            body.chars().next_back().is_some_and(|c| !c.is_whitespace());
                        if closes_on_word {
                            flush_inline_buf(&mut spans, &mut buf, default_style);
                            let bold = default_style.add_modifier(Modifier::BOLD);
                            spans.push(Span::styled(body.to_string(), bold));
                            i += 2 + end + 2;
                            continue;
                        }
                    }
                }
            }

            if *byte == b'*' && !rest.starts_with("**") {
                let after = &rest[1..];
                let opens_on_word = after
                    .chars()
                    .next()
                    .is_some_and(|c| !c.is_whitespace() && c != '*');
                if opens_on_word {
                    // Find the next single `*` (not `**`) whose preceding char
                    // is non-whitespace — that's the closing marker per
                    // commonmark-ish emphasis rules.
                    let mut search_at = 0usize;
                    let mut found: Option<usize> = None;
                    while let Some(p) = after[search_at..].find('*') {
                        let abs = search_at + p;
                        let body = &after[..abs];
                        let last_ok = body.chars().next_back().is_some_and(|c| !c.is_whitespace());
                        let not_double = after.as_bytes().get(abs + 1) != Some(&b'*');
                        if last_ok && not_double {
                            found = Some(abs);
                            break;
                        }
                        search_at = abs + 1;
                    }
                    if let Some(end) = found {
                        flush_inline_buf(&mut spans, &mut buf, default_style);
                        let italic = default_style.add_modifier(Modifier::ITALIC);
                        spans.push(Span::styled(after[..end].to_string(), italic));
                        i += 1 + end + 1;
                        continue;
                    }
                }
            }
        }

        // Literal character — including any opener that didn't pair off.
        let ch = rest.chars().next().expect("rest is non-empty here");
        buf.push(ch);
        i += ch.len_utf8();
    }

    flush_inline_buf(&mut spans, &mut buf, default_style);
    spans
}

fn flush_inline_buf<'a>(spans: &mut Vec<Span<'a>>, buf: &mut String, style: Style) {
    if !buf.is_empty() {
        spans.push(Span::styled(std::mem::take(buf), style));
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
    let hints = hint_bar_spans(app);
    let hint_line = Paragraph::new(Line::from(hints)).style(
        Style::default()
            .bg(theme::ratatui_color(SURFACE))
            .fg(theme::ratatui_color(HINT_LABEL)),
    );
    f.render_widget(hint_line, area);
}

/// State-driven hint bar. The composer's bottom row changes based on what
/// the user can usefully press right now — the slash popup, agent picker,
/// session/help overlay, a pending Cast confirmation, or a live session
/// each get their own hint set so first-time users discover the surface
/// area faster than a single static example would let them.
fn hint_bar_spans<'a>(app: &App) -> Vec<Span<'a>> {
    fn key(label: &str) -> Span<'static> {
        Span::styled(label.to_string(), theme::ratatui_style(HINT_KEY).bold())
    }
    fn label(text: &str) -> Span<'static> {
        Span::styled(text.to_string(), theme::ratatui_style(HINT_LABEL))
    }
    fn separator() -> Span<'static> {
        Span::styled("  ", theme::ratatui_style(HINT_LABEL))
    }

    if app.input_mode == InputMode::AgentSelect {
        return vec![
            label(" "),
            key("\u{2191}\u{2193}"),
            label(" navigate"),
            separator(),
            key("Enter"),
            label(" select"),
            separator(),
            key("Esc"),
            label(" cancel"),
        ];
    }

    if app.show_session_overlay {
        return vec![
            label(" "),
            key("r"),
            label(" refresh"),
            separator(),
            key("Esc"),
            label(" close"),
        ];
    }

    if app.show_help {
        return vec![label(" "), key("Esc"), label(" close help")];
    }

    if app.slash_popup_is_open() {
        return vec![
            label(" "),
            key("Tab"),
            label(" complete"),
            separator(),
            key("\u{2191}\u{2193}"),
            label(" pick"),
            separator(),
            key("Enter"),
            label(" run"),
            separator(),
            key("Esc"),
            label(" close"),
        ];
    }

    if app.has_pending_cast_confirmation() {
        return vec![
            label(" "),
            key("accept"),
            label(" / "),
            key("reject"),
            label(" pending Cast confirmation"),
        ];
    }

    if app.active_session_id().is_some() {
        return vec![
            label(" "),
            key("Esc"),
            label(" interrupt"),
            separator(),
            key("Ctrl+C"),
            label(" cancel · twice to exit"),
        ];
    }

    // Default: advertise the keys that unlock the rest of the surface
    // (slash menu, history, help).
    vec![
        label(" "),
        key("/"),
        label(" commands"),
        separator(),
        key("\u{2191}\u{2193}"),
        label(" history"),
        separator(),
        key("Ctrl+K"),
        label(" help"),
    ]
}

fn render_help_overlay(f: &mut Frame, area: Rect) {
    let overlay_width = 60u16.min(area.width.saturating_sub(4));
    // Fits Basics(5) + Agents(2) + Sessions(4) + Output(3) + Keys(6) +
    // Advanced(4) plus section headers and separators. Clamps to the
    // terminal so very short windows still render something useful even if
    // the bottom rows clip.
    let overlay_height = 34u16.min(area.height.saturating_sub(4));
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
            "Keys",
            vec![
                ("Tab", "Complete the highlighted slash suggestion"),
                ("Up / Down", "Browse history or the slash popup"),
                ("Esc", "Cancel popup, input, or running session"),
                ("Ctrl+C", "Cancel; press twice within 2s to exit"),
                ("Ctrl+L", "Clear the visible transcript"),
                ("Ctrl+D", "Exit Coven chat"),
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

/// Floating slash-command autocomplete. Anchored just above the input area so
/// it acts like a dropdown attached to the composer. Drawn after the input is
/// painted (so the popup never overlaps the cursor) and before the help and
/// session overlays so those can still steal focus visually.
fn render_slash_popup(f: &mut Frame, app: &App, input_area: Rect) {
    let suggestions = app.slash_suggestions();
    if suggestions.is_empty() {
        return;
    }

    // Show up to 8 rows; chrome adds the border (2) so the visible row count
    // is the popup height minus 2.
    let visible_rows = suggestions.len().min(8) as u16;
    let popup_height = visible_rows.saturating_add(2);
    // Width the popup to the longest name + summary combo, clamped to the
    // input width so it never juts past the composer.
    let preferred_width = suggestions
        .iter()
        .map(|cmd| cmd.name.len() + cmd.summary.len() + 6)
        .max()
        .unwrap_or(30) as u16;
    let popup_width = preferred_width
        .max(28)
        .min(input_area.width.max(28))
        .min(60);

    // Anchor under the top of the input box, growing upward.
    let popup_x = input_area.x;
    let popup_y = input_area.y.saturating_sub(popup_height);
    // If there isn't enough room above (very short terminals), bail out — the
    // user can still complete with Tab; we just skip drawing rather than
    // overlaying the transcript.
    if popup_y == input_area.y || popup_height == 0 {
        return;
    }
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);

    let selected = app.slash_suggestion_index.min(suggestions.len() - 1);
    let items: Vec<ListItem> = suggestions
        .iter()
        .enumerate()
        .map(|(idx, cmd)| {
            let is_selected = idx == selected;
            let row_style = if is_selected {
                theme::ratatui_style(PRIMARY_STRONG)
                    .bg(theme::ratatui_color(SURFACE_STRONG))
                    .bold()
            } else {
                theme::ratatui_style(PRIMARY)
            };
            let summary_style = if is_selected {
                theme::ratatui_style(TEXT).bg(theme::ratatui_color(SURFACE_STRONG))
            } else {
                theme::ratatui_style(TEXT_DIM)
            };
            let marker = if is_selected { "▸ " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(marker, row_style),
                Span::styled(format!("{:<10}", cmd.name), row_style),
                Span::styled("  ", summary_style),
                Span::styled(cmd.summary, summary_style),
            ]))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::ratatui_style(PRIMARY))
        .title(Span::styled(
            " commands ",
            theme::ratatui_style(PRIMARY).bold(),
        ))
        .style(Style::default().bg(theme::ratatui_color(SURFACE)));

    let list = List::new(items).block(block);
    f.render_widget(list, popup_area);
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
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(value) <= max_width {
        return value.to_string();
    }
    if max_width == 1 {
        return "\u{2026}".to_string();
    }
    let mut output = String::new();
    let mut output_width = 0usize;
    let content_budget = max_width - 1;
    for ch in value.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if output_width + ch_width > content_budget {
            break;
        }
        output_width += ch_width;
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
        // Default hint advertises slash menu + history + help keys so a
        // first-time user can discover the surface without typing /help.
        assert!(frame.contains("commands"));
        assert!(frame.contains("history"));
        assert!(frame.contains("Ctrl+K"));
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

    /// Helper: pull a code line out of rendered output. A code-block row
    /// starts with the bar-prefix span "  │ "; the search needle matches
    /// against any of the tokenized spans on that row.
    fn find_code_line<'a>(lines: &'a [Line<'a>], needle: &str) -> Option<&'a Line<'a>> {
        lines.iter().find(|line| {
            line.spans.first().map(|s| s.content.as_ref()) == Some("  \u{2502} ")
                && line.spans.iter().any(|s| s.content.contains(needle))
        })
    }

    #[test]
    fn agent_lines_highlight_rust_code_block_keyword_and_string() {
        // A ```rust fence should produce many spans per line — the keyword
        // `let`, the string literal, and surrounding punctuation each land
        // in their own span instead of one verbatim text run.
        let mut lines: Vec<Line<'_>> = Vec::new();
        let content = "Pre.\n```rust\nlet x = \"hi\";\n```\nPost.";
        append_agent_content_lines(&mut lines, content, 60);

        let code_line = find_code_line(&lines, "let").expect("code line emitted");
        assert!(
            code_line.spans.len() >= 4,
            "expected tokenized spans, got {:#?}",
            code_line.spans
        );
        let let_span = code_line
            .spans
            .iter()
            .find(|s| s.content == "let")
            .expect("`let` span present");
        assert!(
            let_span.style.add_modifier.contains(Modifier::BOLD),
            "keyword span should carry BOLD modifier"
        );
        assert!(
            code_line.spans.iter().any(|s| s.content == "\"hi\""),
            "string literal should be its own span"
        );
    }

    #[test]
    fn agent_lines_highlight_typescript_via_ts_alias() {
        // Confirm the `ts` alias resolves to the JS tokenizer and that
        // template literals survive as a single string span.
        let mut lines: Vec<Line<'_>> = Vec::new();
        let content = "```ts\nconst greet = `hi ${name}`;\n```";
        append_agent_content_lines(&mut lines, content, 60);

        let code_line = find_code_line(&lines, "const").expect("code line emitted");
        let const_span = code_line
            .spans
            .iter()
            .find(|s| s.content == "const")
            .expect("`const` span present");
        assert!(const_span.style.add_modifier.contains(Modifier::BOLD));
        assert!(
            code_line.spans.iter().any(|s| s.content == "`hi ${name}`"),
            "template literal should be one string span; got {:#?}",
            code_line.spans
        );
    }

    #[test]
    fn agent_lines_skip_highlighting_for_unknown_or_missing_language() {
        // No language tag → fall back to the legacy single-span code line.
        let mut lines: Vec<Line<'_>> = Vec::new();
        let content = "```\nlet x = 1;\n```";
        append_agent_content_lines(&mut lines, content, 60);
        let code_line = find_code_line(&lines, "let").expect("code line emitted");
        assert_eq!(
            code_line.spans.len(),
            2,
            "plain code block should be bar + verbatim text only"
        );

        // Unknown language → same fallback.
        let mut lines: Vec<Line<'_>> = Vec::new();
        let content = "```cobol\nDISPLAY 'hi'.\n```";
        append_agent_content_lines(&mut lines, content, 60);
        let code_line = find_code_line(&lines, "DISPLAY").expect("code line emitted");
        assert_eq!(code_line.spans.len(), 2);
    }

    #[test]
    fn agent_lines_reset_language_after_close_fence() {
        // A rust fence followed by an unlabeled fence must not carry the
        // rust tokenizer into the second block.
        let mut lines: Vec<Line<'_>> = Vec::new();
        let content = "```rust\nlet a = 1;\n```\n\n```\nlet b = 2;\n```";
        append_agent_content_lines(&mut lines, content, 60);

        let mut code_lines: Vec<&Line<'_>> = lines
            .iter()
            .filter(|line| line.spans.first().map(|s| s.content.as_ref()) == Some("  \u{2502} "))
            .collect();
        assert_eq!(code_lines.len(), 2, "expected two code rows");
        let plain = code_lines.pop().unwrap();
        let rust = code_lines.pop().unwrap();
        assert!(rust.spans.len() > 2, "rust row should be tokenized");
        assert_eq!(plain.spans.len(), 2, "second (unlabeled) row should not");
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
    fn agent_lines_preserve_table_row_alignment_at_wide_widths() {
        // When the table fits inside the wrap budget, every row should land
        // on its own line unwrapped so column boundaries stay intact.
        let mut lines: Vec<Line<'_>> = Vec::new();
        let content = "\
| Mode    | Status chip   | Best for                    |
|---------|---------------|-----------------------------|
| live    | stream: live  | watching long-running plans |
| batched | stream: off   | short queries and demos     |";
        append_agent_content_lines(&mut lines, content, 80);

        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.to_string()).collect())
            .collect();

        assert_eq!(rendered.len(), 4, "every table row should render unwrapped");
        for row in &rendered {
            assert!(
                row.matches('|').count() == 4,
                "row lost a pipe (should have 4): {row:?}"
            );
        }
    }

    #[test]
    fn agent_lines_truncate_table_rows_with_ellipsis_at_narrow_widths() {
        // When the source row is wider than the wrap budget, truncate with
        // an ellipsis so the row stays on one line and the column header
        // remains readable. Losing the right edge beats losing the columns.
        let mut lines: Vec<Line<'_>> = Vec::new();
        let row = "| A | B | C | D | E | F | G | H |";
        append_agent_content_lines(&mut lines, row, 16);

        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.to_string()).collect())
            .collect();

        assert_eq!(rendered.len(), 1, "table row must not wrap");
        let only = &rendered[0];
        assert!(
            only.starts_with("  | A | B"),
            "left edge preserved: {only:?}"
        );
        assert!(only.ends_with('\u{2026}'), "ellipsis applied: {only:?}");
    }

    #[test]
    fn truncate_for_width_never_exceeds_requested_budget() {
        assert_eq!(truncate_for_width("abcdef", 0), "");
        assert_eq!(truncate_for_width("abcdef", 1), "\u{2026}");
        assert_eq!(truncate_for_width("abcdef", 2), "a\u{2026}");
        assert_eq!(truncate_for_width("abcdef", 4), "abc\u{2026}");

        let wide = truncate_for_width("表表abc", 4);
        assert!(UnicodeWidthStr::width(wide.as_str()) <= 4);
        assert!(wide.ends_with('\u{2026}'));
    }

    #[test]
    fn agent_lines_truncate_code_blocks_by_display_width() {
        let mut lines: Vec<Line<'_>> = Vec::new();
        append_agent_content_lines(&mut lines, "```\n表表abc\n```", 6);

        let code_line = find_code_line(&lines, "").expect("code line emitted");
        let visible: String = code_line
            .spans
            .iter()
            .skip(1)
            .map(|span| span.content.as_ref())
            .collect();
        assert!(
            UnicodeWidthStr::width(visible.as_str()) <= 2,
            "visible code text should fit the post-prefix budget: {visible:?}"
        );
    }

    #[test]
    fn agent_lines_treat_tab_bullet_indent_as_columns() {
        let mut lines: Vec<Line<'_>> = Vec::new();
        append_agent_content_lines(&mut lines, "\t- nested", 40);

        let rendered: String = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(
            rendered.starts_with("      \u{2022} nested"),
            "tab indent should expand to four columns after the message margin: {rendered:?}"
        );
    }

    #[test]
    fn agent_lines_render_table_separator_row_without_wrapping() {
        // The `|---|---|...` separator row is what makes a markdown table
        // visually a table; if it wraps the table reads as garbage.
        let mut lines: Vec<Line<'_>> = Vec::new();
        let content = "| Col A | Col B |\n|-------|-------|\n| value | other |";
        append_agent_content_lines(&mut lines, content, 80);

        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.to_string()).collect())
            .collect();
        assert_eq!(rendered.len(), 3);
        assert!(rendered[1].contains("|-------|-------|"));
    }

    #[test]
    fn heading_styles_form_a_distinct_visual_hierarchy() {
        // H1 must be visually loudest, H4 the quietest. We don't pin the
        // exact tokens here — that would be brittle — but we do require the
        // styles for each level to be pairwise different so the hierarchy
        // is actually readable in the chat.
        let styles = [
            heading_style_for(1),
            heading_style_for(2),
            heading_style_for(3),
            heading_style_for(4),
        ];
        for (i, lhs) in styles.iter().enumerate() {
            for (j, rhs) in styles.iter().enumerate() {
                if i != j {
                    assert_ne!(
                        lhs,
                        rhs,
                        "heading level {} and {} share the same style",
                        i + 1,
                        j + 1
                    );
                }
            }
        }
        // H1 must be bold + underlined so it reads as the top of the page.
        assert!(styles[0].add_modifier.contains(Modifier::BOLD));
        assert!(styles[0].add_modifier.contains(Modifier::UNDERLINED));
        // H4 is italic body text — no bold, no underline.
        assert!(!styles[3].add_modifier.contains(Modifier::BOLD));
        assert!(!styles[3].add_modifier.contains(Modifier::UNDERLINED));
        assert!(styles[3].add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn strip_heading_prefix_reports_level() {
        assert_eq!(strip_heading_prefix("# Title"), Some((1u8, "Title")));
        assert_eq!(strip_heading_prefix("## Title"), Some((2, "Title")));
        assert_eq!(strip_heading_prefix("### Title"), Some((3, "Title")));
        assert_eq!(strip_heading_prefix("#### Title"), Some((4, "Title")));
        assert_eq!(strip_heading_prefix("##### Title"), None);
        assert_eq!(strip_heading_prefix("#no space"), None);
        assert_eq!(strip_heading_prefix("paragraph"), None);
    }

    #[test]
    fn inline_markdown_splits_code_bold_and_italic_into_styled_spans() {
        let default_style = theme::ratatui_style(TEXT);
        let spans = parse_inline_markdown("use `cargo` to **build** the *project*", default_style);

        let texts: Vec<String> = spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(
            texts,
            vec![
                "use ".to_string(),
                "cargo".to_string(),
                " to ".to_string(),
                "build".to_string(),
                " the ".to_string(),
                "project".to_string(),
            ]
        );

        // Style hops must follow the markers: plain, code, plain, bold,
        // plain, italic — with the markers themselves stripped.
        assert_eq!(spans[0].style, default_style);
        assert!(spans[1].style.add_modifier.contains(Modifier::ITALIC));
        assert_eq!(spans[2].style, default_style);
        assert!(spans[3].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[4].style, default_style);
        assert!(spans[5].style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn inline_markdown_leaves_unclosed_markers_as_literal_text() {
        let default_style = theme::ratatui_style(TEXT);
        let spans = parse_inline_markdown("an *unclosed italic line", default_style);
        let joined: String = spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(joined, "an *unclosed italic line");
        // Every span should be plain — no modifiers were applied.
        for span in &spans {
            assert!(!span.style.add_modifier.contains(Modifier::ITALIC));
            assert!(!span.style.add_modifier.contains(Modifier::BOLD));
        }
    }

    #[test]
    fn inline_markdown_does_not_match_arithmetic_or_whitespace_padded_stars() {
        // `2 * 3 * 4` and `* a *` should both stay literal because the
        // markers are surrounded by whitespace — emphasis requires the
        // opener to hug a word char and the closer to be preceded by one.
        let default_style = theme::ratatui_style(TEXT);
        for input in ["2 * 3 * 4", "leading: * a *", "trailing space *body *"] {
            let spans = parse_inline_markdown(input, default_style);
            let joined: String = spans.iter().map(|s| s.content.to_string()).collect();
            assert_eq!(joined, input, "literal text mangled for {input:?}");
            for span in &spans {
                assert!(
                    !span.style.add_modifier.contains(Modifier::ITALIC),
                    "false italic match for {input:?}: {:?}",
                    span.content
                );
            }
        }
    }

    #[test]
    fn inline_markdown_preserves_stars_inside_inline_code() {
        // Stars inside an inline-code span should stay literal rather than
        // becoming italic markers.
        let default_style = theme::ratatui_style(TEXT);
        let spans = parse_inline_markdown("see `*foo*` for the literal stars", default_style);
        let texts: Vec<String> = spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(
            texts,
            vec![
                "see ".to_string(),
                "*foo*".to_string(),
                " for the literal stars".to_string(),
            ]
        );
        // The middle span is inline code, not italic.
        assert!(spans[1].style.add_modifier.contains(Modifier::ITALIC)); // code styled italic
                                                                         // The outer text remains plain — no italic spilling out of the code span.
        assert!(!spans[0].style.add_modifier.contains(Modifier::ITALIC));
        assert!(!spans[2].style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn agent_lines_skip_inline_markdown_inside_code_blocks_and_tables() {
        // Inside fenced code: backticks and stars must stay literal so the
        // user sees the actual code they pasted.
        let mut lines: Vec<Line<'_>> = Vec::new();
        append_agent_content_lines(&mut lines, "```\nlet x = `1` * **two**;\n```", 80);
        let code_line = &lines[0];
        let body: String = code_line
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(
            body.contains('`'),
            "backticks must survive inside code: {body:?}"
        );
        assert!(
            body.contains("**"),
            "stars must survive inside code: {body:?}"
        );

        // Inside a table row: same deal — cells render verbatim.
        let mut lines2: Vec<Line<'_>> = Vec::new();
        append_agent_content_lines(&mut lines2, "| `code` | *italic* |", 80);
        let row: String = lines2[0]
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(
            row.contains('`'),
            "backticks survive in table cells: {row:?}"
        );
        assert!(row.contains('*'), "stars survive in table cells: {row:?}");
    }

    #[test]
    fn agent_lines_apply_inline_markdown_inside_headings_and_bullets() {
        let mut lines: Vec<Line<'_>> = Vec::new();
        append_agent_content_lines(
            &mut lines,
            "## A **bold** heading\n\n- a bullet with `code`",
            80,
        );

        // Heading line carries multiple spans now — including a bold body.
        let heading_line = lines
            .iter()
            .find(|line| line.spans.iter().any(|span| span.content.contains("bold")));
        let heading_line = heading_line.expect("heading line present");
        let bold_span = heading_line
            .spans
            .iter()
            .find(|span| span.content == "bold")
            .expect("bold span present in heading");
        assert!(bold_span.style.add_modifier.contains(Modifier::BOLD));

        // Bullet line carries an inline-code span.
        let bullet_line = lines.iter().find(|line| {
            line.spans
                .iter()
                .any(|span| span.content.contains("\u{2022}"))
        });
        let bullet_line = bullet_line.expect("bullet line present");
        let code_span = bullet_line
            .spans
            .iter()
            .find(|span| span.content == "code")
            .expect("inline code span present in bullet body");
        assert!(code_span.style.add_modifier.contains(Modifier::ITALIC));
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
