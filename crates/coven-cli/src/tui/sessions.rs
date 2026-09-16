use std::io::{self, IsTerminal, Write};

use anyhow::{Context, Result};
use crossterm::{
    cursor::MoveTo,
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
};
use serde::Serialize;

use super::is_key_press;
use crate::theme::fit_chars;
use crate::{
    archive_session_command, attach_session, coven_store_path, first_chars,
    prompt_for_required_line, sacrifice_session_command, store, summon_session_command, theme,
    RUNNING_SESSION_STATUS,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionBrowserMove {
    Up,
    Down,
    PreviousAction,
    NextAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionBrowserActionKind {
    Attach,
    Summon,
    Archive,
    Sacrifice,
    Back,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionsCommandMode {
    Browser,
    List,
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SessionBrowserAction {
    pub(crate) key: &'static str,
    pub(crate) label: &'static str,
    pub(crate) help: &'static str,
    pub(crate) kind: SessionBrowserActionKind,
}

pub(crate) const SESSION_BROWSER_FIRST_SESSION_ROW: usize = 5;
const SESSION_BROWSER_MAX_VISIBLE_SESSIONS: usize = 8;
const PLAIN_SESSION_ID_COLUMN_WIDTH: usize = 36;
const PLAIN_SESSION_STATUS_COLUMN_WIDTH: usize = 10;
/// Floor, not a cap: harness ids are open-ended (adapters declare their own,
/// and the built-in engine already ships `coven-code`, which is 10 characters),
/// so the printed width is measured from the rows themselves in
/// [`PlainSessionColumnWidths::for_sessions`].
const PLAIN_SESSION_HARNESS_MIN_COLUMN_WIDTH: usize = 8;
const PLAIN_SESSION_STATE_COLUMN_WIDTH: usize = 8;
/// Titles default to the first `DEFAULT_TITLE_CHARS` characters of the prompt,
/// but `--title` accepts any length. Cap the trailing column at the same width
/// so an over-long title is cut *honestly* — `fit_chars` marks the cut with `…`
/// instead of leaving a complete-looking line.
const PLAIN_SESSION_TITLE_COLUMN_WIDTH: usize = crate::DEFAULT_TITLE_CHARS;

/// Restores raw mode and mouse capture on all exit paths for the session browser.
struct BrowserTerminalGuard {
    raw_mode_enabled: bool,
    mouse_capture_enabled: bool,
}

impl BrowserTerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("failed to enter Coven session browser")?;
        if let Err(error) = execute!(io::stdout(), EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(anyhow::Error::from(error)
                .context("failed to enable Coven session browser mouse support"));
        }
        Ok(Self {
            raw_mode_enabled: true,
            mouse_capture_enabled: true,
        })
    }

    fn restore(&mut self) -> Result<()> {
        let mut first_error = None;

        if self.mouse_capture_enabled {
            if let Err(error) = execute!(io::stdout(), DisableMouseCapture) {
                first_error.get_or_insert_with(|| {
                    anyhow::Error::from(error)
                        .context("failed to disable Coven session browser mouse support")
                });
            } else {
                self.mouse_capture_enabled = false;
            }
        }

        if self.raw_mode_enabled {
            if let Err(error) = disable_raw_mode() {
                first_error.get_or_insert_with(|| {
                    anyhow::Error::from(error).context("failed to leave Coven session browser")
                });
            } else {
                self.raw_mode_enabled = false;
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }

        Ok(())
    }
}

impl Drop for BrowserTerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn list_sessions_plain(include_archived: bool) -> Result<()> {
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let sessions = if include_archived {
        store::list_sessions_including_archived(&conn)?
    } else {
        store::list_sessions(&conn)?
    };

    if sessions.is_empty() {
        if include_archived {
            println!("No Coven sessions yet — active or archived.");
        } else {
            println!("No active Coven sessions yet.");
        }
        // Hints go to stderr so `coven sessions --plain | ...` stays a clean table.
        eprintln!("Start or inspect with:");
        eprintln!("  coven doctor");
        if let Some(harness) = crate::default_harness_id() {
            eprintln!("  coven run {harness} \"explain this repo in 5 bullets\"");
        }
        eprintln!("  coven sessions --all");
    } else {
        println!("{}", render_plain_session_table(&sessions));
        // Cheat-sheet goes to stderr so the stdout table stays parseable.
        eprintln!("\nRituals:");
        eprintln!(
            "  coven summon <session-id>       # restore archived session, then replay/follow"
        );
        eprintln!("  coven archive <session-id>      # hide from active list, keep events");
        eprintln!(
            "  coven sacrifice <session-id> --yes  # delete eligible non-running session; adopted/reserved sessions are retained"
        );
    }

    Ok(())
}

fn list_sessions_json(include_archived: bool) -> Result<()> {
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let sessions = if include_archived {
        store::list_sessions_including_archived(&conn)?
    } else {
        store::list_sessions(&conn)?
    };

    println!("{}", render_sessions_json(&sessions)?);
    Ok(())
}

pub(crate) fn run_command(
    include_archived: bool,
    manage: bool,
    plain: bool,
    json: bool,
) -> Result<()> {
    match sessions_command_mode(
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
        manage,
        plain,
        json,
    ) {
        SessionsCommandMode::Browser => run_browser(include_archived),
        SessionsCommandMode::List => list_sessions_plain(include_archived),
        SessionsCommandMode::Json => list_sessions_json(include_archived),
    }
}

pub(crate) fn sessions_command_mode(
    stdin_terminal: bool,
    stdout_terminal: bool,
    manage: bool,
    plain: bool,
    json: bool,
) -> SessionsCommandMode {
    if json {
        SessionsCommandMode::Json
    } else if plain {
        SessionsCommandMode::List
    } else if manage || (stdin_terminal && stdout_terminal) {
        SessionsCommandMode::Browser
    } else {
        SessionsCommandMode::List
    }
}

pub(crate) fn run_browser(include_archived: bool) -> Result<()> {
    let primary = theme::fg(theme::PRIMARY);
    let reset = theme::reset();
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let sessions = if include_archived {
        store::list_sessions_including_archived(&conn)?
    } else {
        store::list_sessions(&conn)?
    };

    if sessions.is_empty() {
        println!("No Coven sessions to manage yet.");
        println!("Start one with `coven run codex \"explain this repo in 5 bullets\"`.");
        return Ok(());
    }

    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        println!("{}", render_session_browser_frame_plain(&sessions, 0, 0));
        println!("\nTip: run this in a terminal to select a session and choose an action.");
        return Ok(());
    }

    let mut selected_session = 0;
    let mut selected_action = 0;
    let mut terminal = BrowserTerminalGuard::enter()?;
    let selected = loop {
        selected_action = selected_action.min(
            session_browser_actions(&sessions[selected_session])
                .len()
                .saturating_sub(1),
        );
        execute!(io::stdout(), Clear(ClearType::All), MoveTo(0, 0))
            .context("failed to redraw Coven session browser")?;
        print!(
            "{}",
            render_session_browser_frame_for_raw_terminal(
                &sessions,
                selected_session,
                selected_action
            )
        );
        io::stdout()
            .flush()
            .context("failed to flush Coven session browser")?;

        match event::read().context("failed to read session browser input")? {
            Event::Key(key) if is_key_press(key.kind) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected_session = move_session_browser_selection(
                        selected_session,
                        sessions.len(),
                        SessionBrowserMove::Up,
                    );
                    selected_action = 0;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected_session = move_session_browser_selection(
                        selected_session,
                        sessions.len(),
                        SessionBrowserMove::Down,
                    );
                    selected_action = 0;
                }
                KeyCode::Left | KeyCode::BackTab => {
                    selected_action = move_session_browser_selection(
                        selected_action,
                        session_browser_actions(&sessions[selected_session]).len(),
                        SessionBrowserMove::PreviousAction,
                    );
                }
                KeyCode::Right | KeyCode::Tab => {
                    selected_action = move_session_browser_selection(
                        selected_action,
                        session_browser_actions(&sessions[selected_session]).len(),
                        SessionBrowserMove::NextAction,
                    );
                }
                KeyCode::Enter => {
                    let action =
                        session_browser_actions(&sessions[selected_session])[selected_action];
                    break Some((sessions[selected_session].clone(), action.kind));
                }
                KeyCode::Char(value) => {
                    let actions = session_browser_actions(&sessions[selected_session]);
                    if let Some(action) = actions.iter().find(|action| {
                        action
                            .key
                            .chars()
                            .eq(std::iter::once(value.to_ascii_lowercase()))
                    }) {
                        break Some((sessions[selected_session].clone(), action.kind));
                    }
                    if matches!(value, 'q' | 'Q') {
                        break None;
                    }
                }
                KeyCode::Esc => break None,
                _ => {}
            },
            Event::Mouse(mouse) => {
                if !matches!(mouse.kind, MouseEventKind::Down(_)) {
                    continue;
                }
                let displayed_count = sessions.len().min(SESSION_BROWSER_MAX_VISIBLE_SESSIONS);
                if let Some(index) = session_browser_session_row_to_index(
                    mouse.row as usize,
                    displayed_count,
                    sessions.len(),
                ) {
                    selected_session = index;
                    selected_action = 0;
                    continue;
                }

                let has_more_sessions = sessions.len() > SESSION_BROWSER_MAX_VISIBLE_SESSIONS;
                let action_count = session_browser_actions(&sessions[selected_session]).len();
                if let Some(index) = session_browser_action_row_to_index(
                    mouse.row as usize,
                    displayed_count,
                    has_more_sessions,
                    action_count,
                ) {
                    selected_action = index;
                    let action =
                        session_browser_actions(&sessions[selected_session])[selected_action];
                    break Some((sessions[selected_session].clone(), action.kind));
                }
            }
            _ => {}
        }
    };
    terminal.restore()?;
    println!();

    if let Some((session, action)) = selected {
        run_browser_action(&session, action)
    } else {
        println!("{primary}Closed session browser. Nothing changed.{reset}");
        Ok(())
    }
}

pub(crate) fn run_browser_action(
    session: &store::SessionRecord,
    action: SessionBrowserActionKind,
) -> Result<()> {
    let primary = theme::fg(theme::PRIMARY);
    let reset = theme::reset();
    match action {
        SessionBrowserActionKind::Attach => attach_session(&session.id),
        SessionBrowserActionKind::Summon => summon_session_command(&session.id),
        SessionBrowserActionKind::Archive => archive_session_command(&session.id),
        SessionBrowserActionKind::Sacrifice => {
            let conn = store::open_store(&coven_store_path()?)?;
            store::ensure_session_sacrificable(&conn, &session.id)?;
            let confirmation = prompt_for_required_line(&format!(
                "Type `sacrifice` to delete eligible non-running session `{}` and its events (adopted/reserved sessions are retained): ",
                first_chars(&session.id, 12)
            ))?;
            if confirmation != "sacrifice" {
                println!("{primary}Sacrifice cancelled. Nothing changed.{reset}");
                return Ok(());
            }
            sacrifice_session_command(&session.id, true)
        }
        SessionBrowserActionKind::Back => {
            println!("{primary}Closed session browser. Nothing changed.{reset}");
            Ok(())
        }
    }
}

pub(crate) fn session_browser_actions(session: &store::SessionRecord) -> Vec<SessionBrowserAction> {
    let attach_label = if session.status == RUNNING_SESSION_STATUS {
        "Rejoin"
    } else {
        "View Log"
    };
    let attach_help = if session.status == RUNNING_SESSION_STATUS {
        "Follow live output and send input"
    } else {
        "Replay captured output"
    };
    let mut actions = vec![SessionBrowserAction {
        key: "a",
        label: attach_label,
        help: attach_help,
        kind: SessionBrowserActionKind::Attach,
    }];

    if session.archived_at.is_some() {
        actions.push(SessionBrowserAction {
            key: "s",
            label: "Summon",
            help: "Restore this archived session",
            kind: SessionBrowserActionKind::Summon,
        });
    } else if session.status != RUNNING_SESSION_STATUS {
        actions.push(SessionBrowserAction {
            key: "r",
            label: "Archive",
            help: "Hide from active list, keep events",
            kind: SessionBrowserActionKind::Archive,
        });
    }

    if session.status != RUNNING_SESSION_STATUS {
        actions.push(SessionBrowserAction {
            key: "x",
            label: "Sacrifice",
            help: "Delete eligible non-running; adopted/reserved sessions are retained",
            kind: SessionBrowserActionKind::Sacrifice,
        });
    }

    actions.push(SessionBrowserAction {
        key: "q",
        label: "Back",
        help: "Close without changing anything",
        kind: SessionBrowserActionKind::Back,
    });
    actions
}

pub(crate) fn render_session_browser_frame_plain(
    sessions: &[store::SessionRecord],
    selected_session: usize,
    selected_action: usize,
) -> String {
    render_session_browser_frame_with_mode(
        sessions,
        selected_session,
        selected_action,
        theme::TerminalMode::NoColor,
    )
}

fn render_session_browser_frame_for_raw_terminal(
    sessions: &[store::SessionRecord],
    selected_session: usize,
    selected_action: usize,
) -> String {
    render_session_browser_frame_with_mode(
        sessions,
        selected_session,
        selected_action,
        theme::mode(),
    )
    .replace('\n', "\r\n")
}

fn render_session_browser_frame_with_mode(
    sessions: &[store::SessionRecord],
    selected_session: usize,
    selected_action: usize,
    mode: theme::TerminalMode,
) -> String {
    let primary = theme::Fg::with_mode(theme::PRIMARY, mode);
    let primary_strong = theme::Fg::with_mode(theme::PRIMARY_STRONG, mode);
    let field_label = theme::Fg::with_mode(theme::FIELD_LABEL, mode);
    let user_label = theme::Fg::with_mode(theme::USER_LABEL, mode);
    let dim = theme::Fg::with_mode(theme::DIM, mode);
    let reset = theme::Reset::with_mode(mode);
    let selected_session = selected_session.min(sessions.len().saturating_sub(1));
    let selected = &sessions[selected_session];
    let actions = session_browser_actions(selected);
    let selected_action = selected_action.min(actions.len().saturating_sub(1));
    let mut frame = String::new();

    frame.push_str(&format!("{primary_strong}Session browser{reset}\n"));
    frame.push_str(&format!(
        "{user_label}Select work, then choose an action. No IDs to copy.{reset}\n\n"
    ));
    frame.push_str(&format!(
        "{primary_strong}Sessions{reset} {dim}(title | state | harness){reset}\n"
    ));
    frame.push_str(&format!(
        "{dim}Up/Down or click session · Tab/click action · Enter runs{reset}\n"
    ));

    for (index, session) in sessions
        .iter()
        .take(SESSION_BROWSER_MAX_VISIBLE_SESSIONS)
        .enumerate()
    {
        let pointer = if index == selected_session { ">" } else { " " };
        let color = if index == selected_session {
            primary_strong
        } else {
            primary
        };
        frame.push_str(&format!(
            "{color}{} {title:<30} {status:<9} {harness}{reset}\n",
            pointer,
            title = fit_chars(&session.title, 30),
            status = session_browser_status(session),
            harness = fit_chars(&session.harness, 8)
        ));
    }
    if sessions.len() > SESSION_BROWSER_MAX_VISIBLE_SESSIONS {
        frame.push_str(&format!(
            "{dim}... {} more session(s). Use `coven sessions --all` for text list.{reset}\n",
            sessions.len() - SESSION_BROWSER_MAX_VISIBLE_SESSIONS
        ));
    }

    frame.push_str(&format!("\n{primary_strong}Selected{reset}\n"));
    frame.push_str(&format!(
        "{field_label}Title:{reset} {}\n",
        fit_chars(&selected.title, 50)
    ));
    frame.push_str(&format!(
        "{field_label}Internal ID:{reset} {}  {field_label}Runtime:{reset} {}  {field_label}Harness:{reset} {}\n",
        first_chars(&selected.id, 18),
        selected.status,
        selected.harness
    ));
    frame.push_str(&format!(
        "{field_label}Project:{reset} {}\n",
        fit_chars(&selected.project_root, 58)
    ));
    frame.push_str(&format!(
        "{field_label}Updated:{reset} {}  {field_label}State:{reset} {}\n",
        selected.updated_at,
        session_browser_status(selected)
    ));

    frame.push_str(&format!("\n{primary_strong}Actions{reset}\n"));
    for (index, action) in actions.iter().enumerate() {
        let pointer = if index == selected_action { ">" } else { " " };
        let color = if index == selected_action {
            primary_strong
        } else {
            primary
        };
        frame.push_str(&format!(
            "{color}{} [{}] {:<10} {}{reset}\n",
            pointer, action.key, action.label, action.help
        ));
    }
    frame
}

fn session_browser_status(session: &store::SessionRecord) -> &'static str {
    if session.archived_at.is_some() {
        "archived"
    } else if session.status == RUNNING_SESSION_STATUS {
        "running"
    } else {
        "active"
    }
}

pub(crate) fn move_session_browser_selection(
    current: usize,
    item_count: usize,
    direction: SessionBrowserMove,
) -> usize {
    if item_count == 0 {
        return 0;
    }
    match direction {
        SessionBrowserMove::Up | SessionBrowserMove::PreviousAction => {
            current.checked_sub(1).unwrap_or(item_count - 1)
        }
        SessionBrowserMove::Down | SessionBrowserMove::NextAction => (current + 1) % item_count,
    }
}
pub(crate) fn session_browser_session_row_to_index(
    row: usize,
    displayed_count: usize,
    total_count: usize,
) -> Option<usize> {
    let index = row.checked_sub(SESSION_BROWSER_FIRST_SESSION_ROW)?;
    (index < displayed_count && index < total_count).then_some(index)
}

const SESSION_BROWSER_ROWS_BEFORE_SELECTED_SECTION: usize = 4;
const SESSION_BROWSER_ROWS_AFTER_SELECTED_SECTION: usize = 4;

fn session_browser_first_action_row(displayed_count: usize, has_more_sessions: bool) -> usize {
    let more_sessions_row_count = usize::from(has_more_sessions);
    SESSION_BROWSER_FIRST_SESSION_ROW
        + displayed_count
        + more_sessions_row_count
        + SESSION_BROWSER_ROWS_BEFORE_SELECTED_SECTION
        + SESSION_BROWSER_ROWS_AFTER_SELECTED_SECTION
}

pub(crate) fn session_browser_action_row_to_index(
    row: usize,
    displayed_count: usize,
    has_more_sessions: bool,
    action_count: usize,
) -> Option<usize> {
    let first_action_row = session_browser_first_action_row(displayed_count, has_more_sessions);
    let index = row.checked_sub(first_action_row)?;
    (index < action_count).then_some(index)
}

/// Render a cell of the plain table safely.
///
/// Session titles come straight from user prompts, so they can carry anything —
/// a raw newline would split one row across two lines and a raw escape would
/// rewrite the whole table. This follows `main.rs::doctor_prose_line`, the same
/// treatment Doctor gives user-controlled config values: control characters are
/// rendered visibly instead of being passed through to the terminal.
fn plain_table_cell(value: &str) -> String {
    crate::doctor_prose_line(value)
}

fn plain_table_cell_width(value: &str) -> usize {
    plain_table_cell(value).chars().count()
}

fn session_ritual(session: &store::SessionRecord) -> &'static str {
    if session.archived_at.is_some() {
        "archived"
    } else {
        "active"
    }
}

/// Column widths for one rendering of the plain session table.
///
/// `coven sessions --plain` is a machine-readable surface — the hints are
/// pushed to stderr precisely so stdout stays a clean table — which makes
/// alignment a contract rather than a cosmetic. Each width is therefore the
/// larger of its historical minimum and the widest sanitized cell actually
/// printed, so no value can shear the columns that follow it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PlainSessionColumnWidths {
    id: usize,
    status: usize,
    harness: usize,
    state: usize,
}

impl PlainSessionColumnWidths {
    pub(crate) fn for_sessions(sessions: &[store::SessionRecord]) -> Self {
        let mut widths = Self {
            id: PLAIN_SESSION_ID_COLUMN_WIDTH,
            status: PLAIN_SESSION_STATUS_COLUMN_WIDTH,
            harness: PLAIN_SESSION_HARNESS_MIN_COLUMN_WIDTH,
            state: PLAIN_SESSION_STATE_COLUMN_WIDTH,
        };
        for session in sessions {
            widths.id = widths.id.max(plain_table_cell_width(&session.id));
            widths.status = widths.status.max(plain_table_cell_width(&session.status));
            widths.harness = widths.harness.max(plain_table_cell_width(&session.harness));
            widths.state = widths
                .state
                .max(plain_table_cell_width(session_ritual(session)));
        }
        widths
    }
}

pub(crate) fn render_plain_session_table(sessions: &[store::SessionRecord]) -> String {
    let widths = PlainSessionColumnWidths::for_sessions(sessions);
    let mut lines = Vec::with_capacity(sessions.len() + 2);
    lines.push(format!(
        "{:<id_width$} {:<status_width$} {:<harness_width$} {:<state_width$} TITLE",
        "SESSION",
        "STATUS",
        "HARNESS",
        "STATE",
        id_width = widths.id,
        status_width = widths.status,
        harness_width = widths.harness,
        state_width = widths.state
    ));
    lines.push(format!(
        "{:<id_width$} {:<status_width$} {:<harness_width$} {:<state_width$} -----",
        "-------",
        "------",
        "-------",
        "-----",
        id_width = widths.id,
        status_width = widths.status,
        harness_width = widths.harness,
        state_width = widths.state
    ));
    lines.extend(
        sessions
            .iter()
            .map(|session| format_session_line(session, widths)),
    );
    lines.join("\n")
}

pub(crate) fn format_session_line(
    session: &store::SessionRecord,
    widths: PlainSessionColumnWidths,
) -> String {
    format!(
        "{:<id_width$} {:<status_width$} {:<harness_width$} {:<state_width$} {}",
        plain_table_cell(&session.id),
        plain_table_cell(&session.status),
        plain_table_cell(&session.harness),
        session_ritual(session),
        // Fit first, then escape: `…` then means the stored title really was
        // longer than the column, rather than merely containing a control
        // character whose escape expansion crowded the tail out.
        plain_table_cell(&fit_chars(&session.title, PLAIN_SESSION_TITLE_COLUMN_WIDTH)),
        id_width = widths.id,
        status_width = widths.status,
        harness_width = widths.harness,
        state_width = widths.state
    )
}

pub(crate) fn render_sessions_json(sessions: &[store::SessionRecord]) -> Result<String> {
    #[derive(Serialize)]
    struct SessionsJson<'a> {
        sessions: &'a [store::SessionRecord],
    }

    serde_json::to_string_pretty(&SessionsJson { sessions })
        .context("failed to serialize sessions as JSON")
}

#[cfg(test)]
pub(crate) fn render_browser_frame_plain_for_test(
    sessions: &[store::SessionRecord],
    selected_session: usize,
    selected_action: usize,
) -> String {
    render_session_browser_frame_plain(sessions, selected_session, selected_action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sacrifice_action_qualifies_adoption_retention() {
        let session = store::SessionRecord {
            id: "session-1".to_string(),
            project_root: "/repo".to_string(),
            harness: "codex".to_string(),
            title: "Done".to_string(),
            status: "completed".to_string(),
            exit_code: Some(0),
            archived_at: None,
            created_at: "2026-08-16T00:00:00Z".to_string(),
            updated_at: "2026-08-16T00:00:00Z".to_string(),
            conversation_id: None,
            familiar_id: None,
            execution_binding: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
            external: false,
            transcript_path: None,
        };

        let sacrifice = session_browser_actions(&session)
            .into_iter()
            .find(|action| action.kind == SessionBrowserActionKind::Sacrifice)
            .expect("completed sessions expose sacrifice");

        assert_eq!(
            sacrifice.help,
            "Delete eligible non-running; adopted/reserved sessions are retained"
        );
    }

    fn plain_table_session(id: &str, harness: &str, title: &str) -> store::SessionRecord {
        store::SessionRecord {
            id: id.to_string(),
            project_root: "/repo".to_string(),
            harness: harness.to_string(),
            title: title.to_string(),
            status: "completed".to_string(),
            exit_code: Some(0),
            archived_at: None,
            created_at: "2026-08-16T00:00:00Z".to_string(),
            updated_at: "2026-08-16T00:00:00Z".to_string(),
            conversation_id: None,
            familiar_id: None,
            execution_binding: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
            external: false,
            transcript_path: None,
        }
    }

    /// Character offsets at which each space-delimited field of a table line
    /// starts. Only the first five are column starts — the title column is last
    /// and may itself contain spaces.
    fn column_offsets(line: &str) -> Vec<usize> {
        let mut offsets = Vec::new();
        let mut previous_was_space = true;
        for (index, character) in line.chars().enumerate() {
            if character == ' ' {
                previous_was_space = true;
            } else {
                if previous_was_space {
                    offsets.push(index);
                }
                previous_was_space = false;
            }
        }
        offsets
    }

    const PLAIN_TABLE_COLUMN_COUNT: usize = 5;

    fn assert_columns_aligned(table: &str) {
        let mut lines = table.lines();
        let header = lines.next().expect("table has a header row");
        let expected = column_offsets(header);
        assert_eq!(
            expected.len(),
            PLAIN_TABLE_COLUMN_COUNT,
            "header should declare exactly five columns: {header:?}"
        );
        for line in lines {
            let offsets = column_offsets(line);
            assert!(
                offsets.len() >= PLAIN_TABLE_COLUMN_COUNT,
                "row is missing columns: {line:?}"
            );
            assert_eq!(
                &offsets[..PLAIN_TABLE_COLUMN_COUNT],
                &expected[..],
                "row columns drifted from the header: {line:?}"
            );
        }
    }

    #[test]
    fn plain_session_table_stays_aligned_when_a_harness_id_outgrows_the_header() {
        let sessions = vec![
            plain_table_session(
                "78f5627d-0000-4000-8000-000000000001",
                "coven-code",
                "howdy",
            ),
            plain_table_session("a6a93bbf-0000-4000-8000-000000000002", "copilot", "wand"),
        ];

        let table = render_plain_session_table(&sessions);

        assert_columns_aligned(&table);
        // The widened harness column must be measured from the rows, not from
        // the header: `coven-code` is ten characters.
        let harness_column = column_offsets(table.lines().next().expect("header"))[2];
        let state_column = column_offsets(table.lines().next().expect("header"))[3];
        assert_eq!(state_column - harness_column, "coven-code".len() + 1);
    }

    #[test]
    fn plain_session_table_keeps_a_control_character_title_on_one_row() {
        let sessions = vec![plain_table_session(
            "5135804f-0000-4000-8000-000000000003",
            "copilot",
            "patrch this\nCount\r\u{1b}[31mred",
        )];

        let table = render_plain_session_table(&sessions);

        // Header, separator, and exactly one row — the newline in the title
        // must not open a second line.
        assert_eq!(table.lines().count(), 3, "table was split: {table:?}");
        assert_columns_aligned(&table);
        let row = table.lines().nth(2).expect("session row");
        assert!(row.contains("\\u{a}"), "newline not escaped: {row:?}");
        assert!(
            row.contains("\\u{d}"),
            "carriage return not escaped: {row:?}"
        );
        assert!(
            !row.contains('\u{1b}'),
            "escape character passed through: {row:?}"
        );
        // Escaping renders the controls visibly; it must not cost the title any
        // of its own characters.
        assert!(row.ends_with("red"), "title tail was eaten: {row:?}");
        assert!(
            !row.contains('…'),
            "a title within the column was marked truncated: {row:?}"
        );
    }

    #[test]
    fn plain_session_table_marks_a_truncated_title_with_an_ellipsis() {
        let long_title = "t".repeat(PLAIN_SESSION_TITLE_COLUMN_WIDTH * 2);
        let sessions = vec![plain_table_session(
            "6d1b0a2c-0000-4000-8000-000000000004",
            "codex",
            &long_title,
        )];

        let table = render_plain_session_table(&sessions);
        let row = table.lines().nth(2).expect("session row");
        let title_start = column_offsets(row)[4];
        let title: String = row.chars().skip(title_start).collect();

        assert!(
            row.ends_with('…'),
            "truncated title lost its marker: {row:?}"
        );
        assert_eq!(title.chars().count(), PLAIN_SESSION_TITLE_COLUMN_WIDTH);
    }

    #[test]
    fn plain_session_table_leaves_a_short_title_whole() {
        let sessions = vec![plain_table_session(
            "6d1b0a2c-0000-4000-8000-000000000005",
            "codex",
            "A useful session",
        )];

        let table = render_plain_session_table(&sessions);
        let row = table.lines().nth(2).expect("session row");

        assert!(row.ends_with("A useful session"), "{row:?}");
    }
}
