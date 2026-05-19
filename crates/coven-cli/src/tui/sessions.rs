use std::io::{self, IsTerminal, Write};

use anyhow::{Context, Result};
use crossterm::{
    cursor::MoveTo,
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
};

use super::is_key_press;
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
        println!("Start or inspect with:");
        println!("  coven doctor");
        println!("  coven run codex \"explain this repo in 5 bullets\"");
        println!("  coven sessions --all");
    } else {
        println!(
            "{:<id_width$} {:<10} {:<8} {:<8} TITLE",
            "SESSION",
            "STATUS",
            "HARNESS",
            "RITUAL",
            id_width = PLAIN_SESSION_ID_COLUMN_WIDTH
        );
        println!(
            "{:<id_width$} {:<10} {:<8} {:<8} -----",
            "-------",
            "------",
            "-------",
            "------",
            id_width = PLAIN_SESSION_ID_COLUMN_WIDTH
        );
        for session in sessions {
            println!("{}", format_session_line(&session));
        }
        println!("\nRituals:");
        println!(
            "  coven summon <session-id>       # restore archived session, then replay/follow"
        );
        println!("  coven archive <session-id>      # hide from active list, keep events");
        println!("  coven sacrifice <session-id> --yes  # permanently delete non-running session");
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
            let confirmation = prompt_for_required_line(&format!(
                "Type `sacrifice` to permanently delete `{}` and its events: ",
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
            help: "Permanent delete after typed confirm",
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
pub(crate) fn format_session_line(session: &store::SessionRecord) -> String {
    let ritual = if session.archived_at.is_some() {
        "archived"
    } else {
        "active"
    };
    format!(
        "{:<id_width$} {:<10} {:<8} {:<8} {}",
        session.id,
        session.status,
        session.harness,
        ritual,
        session.title,
        id_width = PLAIN_SESSION_ID_COLUMN_WIDTH
    )
}

pub(crate) fn render_sessions_json(sessions: &[store::SessionRecord]) -> Result<String> {
    serde_json::to_string_pretty(&serde_json::json!({ "sessions": sessions }))
        .context("failed to serialize sessions as JSON")
}

fn fit_chars(value: &str, limit: usize) -> String {
    let count = value.chars().count();
    if count <= limit {
        return value.to_string();
    }
    if limit == 0 {
        return String::new();
    }
    if limit == 1 {
        return "…".to_string();
    }

    let mut fitted = value.chars().take(limit - 1).collect::<String>();
    fitted.push('…');
    fitted
}

#[cfg(test)]
pub(crate) fn render_browser_frame_plain_for_test(
    sessions: &[store::SessionRecord],
    selected_session: usize,
    selected_action: usize,
) -> String {
    render_session_browser_frame_plain(sessions, selected_session, selected_action)
}
