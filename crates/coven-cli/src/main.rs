use std::collections::HashSet;
use std::ffi::OsString;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::{SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use crossterm::{
    cursor::MoveTo,
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
};
use uuid::Uuid;

mod api;
mod daemon;
mod harness;
mod openclaw_repo;
mod patch;
mod project;
mod pty_runner;
mod store;
mod verification;

const DEFAULT_COVEN_HOME_DIR: &str = ".coven";
const STORE_FILE_NAME: &str = "coven.sqlite3";
const DEFAULT_SESSION_STATUS: &str = "created";
const RUNNING_SESSION_STATUS: &str = "running";
const FAILED_SESSION_STATUS: &str = "failed";
const DEFAULT_TITLE_CHARS: usize = 48;

#[derive(Parser, Debug)]
#[command(name = "coven")]
#[command(about = "Run project-scoped coding agents without memorizing harness commands")]
#[command(
    long_about = "Coven runs Codex, Claude Code, and future harnesses inside a local, project-scoped session ledger. Run `coven` with no arguments for a beginner-friendly menu."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[command(about = "Open the slash-command TUI")]
    Tui,
    #[command(about = "Check local setup and print next steps")]
    Doctor,
    #[command(about = "Manage the local Coven daemon")]
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    #[command(about = "Launch a project-scoped harness session")]
    Run {
        #[arg(help = "Harness to run: codex or claude")]
        harness: String,
        #[arg(help = "Task for the harness", required = true, num_args = 1..)]
        prompt: Vec<String>,
        #[arg(long, help = "Working directory inside the current project")]
        cwd: Option<PathBuf>,
        #[arg(long, help = "Readable title for `coven sessions`")]
        title: Option<String>,
        #[arg(long, help = "Create the session record without launching the harness")]
        detach: bool,
    },
    #[command(about = "List recent Coven sessions")]
    Sessions {
        #[arg(long, help = "Include archived sessions")]
        all: bool,
        #[arg(long, help = "Open the interactive session action browser")]
        manage: bool,
        #[arg(long, help = "Print a plain table instead of the session browser")]
        plain: bool,
    },
    #[command(about = "Replay/follow a session and forward input to live daemon sessions")]
    Attach { session_id: String },
    #[command(about = "Summon an archived session back, then replay/follow it")]
    Summon { session_id: String },
    #[command(about = "Archive a completed session without deleting its events")]
    Archive { session_id: String },
    #[command(about = "Permanently delete a non-running session and its events")]
    Sacrifice {
        session_id: String,
        #[arg(long, help = "Confirm permanent deletion")]
        yes: bool,
    },
    #[command(about = "Guided repair flows")]
    Patch {
        #[command(subcommand)]
        command: PatchCommand,
    },
}

#[derive(Subcommand, Debug)]
enum PatchCommand {
    #[command(name = "openclaw")]
    OpenClaw {
        #[arg(num_args = 0..)]
        issue: Vec<String>,
        #[arg(long)]
        repo: Option<PathBuf>,
        #[arg(long)]
        harness: Option<String>,
        #[arg(long)]
        verify: Option<String>,
        #[arg(long)]
        non_interactive: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        keep_session: bool,
    },
}

#[derive(Subcommand, Debug)]
enum DaemonCommand {
    Start,
    Restart,
    Status,
    Stop,
    #[command(hide = true)]
    Serve,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => run_magical_tui(),
        Some(Command::Tui) => run_magical_tui(),
        Some(Command::Doctor) => run_doctor(),
        Some(Command::Daemon { command }) => run_daemon_command(command),
        Some(Command::Run {
            harness,
            prompt,
            cwd,
            title,
            detach,
        }) => run_session(&harness, &prompt, cwd.as_deref(), title.as_deref(), detach),
        Some(Command::Sessions { all, manage, plain }) => run_sessions_command(all, manage, plain),
        Some(Command::Attach { session_id }) => attach_session(&session_id),
        Some(Command::Summon { session_id }) => summon_session_command(&session_id),
        Some(Command::Archive { session_id }) => archive_session_command(&session_id),
        Some(Command::Sacrifice { session_id, yes }) => sacrifice_session_command(&session_id, yes),
        Some(Command::Patch { command }) => run_patch_command(command),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MagicalTuiAction {
    StartHere,
    OpenTui,
    Doctor,
    DaemonStatus,
    RunHarness,
    PatchOpenClaw,
    Sessions,
    AllSessions,
    AttachSession,
    SummonSession,
    ArchiveSession,
    SacrificeSession,
    Quit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MagicalTuiMove {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionBrowserMove {
    Up,
    Down,
    PreviousAction,
    NextAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionBrowserActionKind {
    Attach,
    Summon,
    Archive,
    Sacrifice,
    Back,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionsCommandMode {
    Browser,
    List,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SessionBrowserAction {
    key: &'static str,
    label: &'static str,
    help: &'static str,
    kind: SessionBrowserActionKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MagicalTuiItem {
    key: &'static str,
    slash: &'static str,
    label: &'static str,
    description: &'static str,
    command: &'static str,
    action: MagicalTuiAction,
}

const PURPLE: &str = "\x1b[38;5;141m";
const GOLD: &str = "\x1b[38;5;220m";
const ROSE: &str = "\x1b[38;5;218m";
const MOON: &str = "\x1b[38;5;117m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const MAGICAL_TUI_DEFAULT_INNER_WIDTH: usize = 24;
const MAGICAL_TUI_MAX_INNER_WIDTH: usize = 24;
const MAGICAL_TUI_MIN_INNER_WIDTH: usize = 24;
const SESSION_BROWSER_FIRST_SESSION_ROW: usize = 5;
const SESSION_BROWSER_MAX_VISIBLE_SESSIONS: usize = 8;

fn magical_tui_items() -> &'static [MagicalTuiItem] {
    &[
        MagicalTuiItem {
            key: "1",
            slash: "/start",
            label: "Start here",
            description: "Setup check and a safe first command",
            command: "coven doctor",
            action: MagicalTuiAction::StartHere,
        },
        MagicalTuiItem {
            key: "0",
            slash: "/tui",
            label: "Open TUI",
            description: "Launch this slash-command palette explicitly",
            command: "coven tui",
            action: MagicalTuiAction::OpenTui,
        },
        MagicalTuiItem {
            key: "2",
            slash: "/doctor",
            label: "Doctor",
            description: "Check store, project, and harness readiness",
            command: "coven doctor",
            action: MagicalTuiAction::Doctor,
        },
        MagicalTuiItem {
            key: "3",
            slash: "/daemon",
            label: "Daemon status",
            description: "See whether the local Coven daemon is awake",
            command: "coven daemon status",
            action: MagicalTuiAction::DaemonStatus,
        },
        MagicalTuiItem {
            key: "4",
            slash: "/run",
            label: "Run an agent",
            description: "Launch Codex or Claude Code inside this project",
            command: "coven run codex \"fix the failing tests\"",
            action: MagicalTuiAction::RunHarness,
        },
        MagicalTuiItem {
            key: "5",
            slash: "/patch",
            label: "Patch OpenClaw",
            description: "Guided repair room for a local OpenClaw checkout",
            command: "coven patch openclaw",
            action: MagicalTuiAction::PatchOpenClaw,
        },
        MagicalTuiItem {
            key: "6",
            slash: "/sessions",
            label: "Active sessions",
            description: "List live, non-archived Coven sessions",
            command: "coven sessions --manage",
            action: MagicalTuiAction::Sessions,
        },
        MagicalTuiItem {
            key: "7",
            slash: "/all",
            label: "All sessions",
            description: "List active and archived sessions together",
            command: "coven sessions --all --manage",
            action: MagicalTuiAction::AllSessions,
        },
        MagicalTuiItem {
            key: "8",
            slash: "/attach",
            label: "Attach session",
            description: "Replay/follow a session by id",
            command: "coven attach <session-id>",
            action: MagicalTuiAction::AttachSession,
        },
        MagicalTuiItem {
            key: "9",
            slash: "/summon",
            label: "Summon session",
            description: "Restore an archived session, then follow it",
            command: "coven summon <session-id>",
            action: MagicalTuiAction::SummonSession,
        },
        MagicalTuiItem {
            key: "a",
            slash: "/archive",
            label: "Archive session",
            description: "Hide completed work without deleting events",
            command: "coven archive <session-id>",
            action: MagicalTuiAction::ArchiveSession,
        },
        MagicalTuiItem {
            key: "x",
            slash: "/sacrifice",
            label: "Sacrifice session",
            description: "Permanently delete a non-running session",
            command: "coven sacrifice <session-id> --yes",
            action: MagicalTuiAction::SacrificeSession,
        },
        MagicalTuiItem {
            key: "q",
            slash: "/quit",
            label: "Quit",
            description: "Exit without changing anything",
            command: "q",
            action: MagicalTuiAction::Quit,
        },
    ]
}

fn run_magical_tui() -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        println!("{}", render_magical_tui_frame_plain(0));
        println!(
            "\nTip: run `coven doctor` first, then `coven run codex \"fix the failing tests\"`."
        );
        return Ok(());
    }

    let mut selection = 0;
    enable_raw_mode().context("failed to enter Coven's magical terminal mode")?;
    let action = loop {
        execute!(io::stdout(), Clear(ClearType::All), MoveTo(0, 0))
            .context("failed to redraw Coven menu")?;
        print!("{}", render_magical_tui_frame_for_raw_terminal(selection));
        io::stdout().flush().context("failed to flush Coven menu")?;

        if let Event::Key(key) = event::read().context("failed to read Coven menu input")? {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selection = move_magical_tui_selection(selection, MagicalTuiMove::Up);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selection = move_magical_tui_selection(selection, MagicalTuiMove::Down);
                }
                KeyCode::Enter => break magical_tui_items()[selection].action,
                KeyCode::Char(value) => {
                    if let Some(item) = magical_tui_items()
                        .iter()
                        .find(|item| item.key.chars().eq(std::iter::once(value)))
                    {
                        break item.action;
                    }
                }
                KeyCode::Esc => break MagicalTuiAction::Quit,
                _ => {}
            }
        }
    };
    disable_raw_mode().context("failed to leave Coven's magical terminal mode")?;
    println!();

    run_magical_tui_action(action)
}

fn run_magical_tui_action(action: MagicalTuiAction) -> Result<()> {
    match action {
        MagicalTuiAction::StartHere => run_new_user_start_here(),
        MagicalTuiAction::OpenTui => run_magical_tui(),
        MagicalTuiAction::Doctor => run_doctor(),
        MagicalTuiAction::DaemonStatus => run_daemon_command(DaemonCommand::Status),
        MagicalTuiAction::RunHarness => run_guided_harness_session(),
        MagicalTuiAction::PatchOpenClaw => {
            run_patch_openclaw(vec![], None, None, None, false, false, true)
        }
        MagicalTuiAction::Sessions => run_session_browser(false),
        MagicalTuiAction::AllSessions => run_session_browser(true),
        MagicalTuiAction::AttachSession
        | MagicalTuiAction::SummonSession
        | MagicalTuiAction::ArchiveSession
        | MagicalTuiAction::SacrificeSession => run_session_browser(true),
        MagicalTuiAction::Quit => {
            println!("{PURPLE}The circle fades. Nothing changed.{RESET}");
            Ok(())
        }
    }
}

fn run_new_user_start_here() -> Result<()> {
    println!("{GOLD}Coven quick start{RESET}");
    println!("Coven is a safe room for coding agents. It keeps each run tied to this project,");
    println!("records the session, and lets other tools list or attach to the work later.\n");
    println!("Recommended first run:");
    println!("  1. coven doctor");
    println!("  2. coven run codex \"explain this repo in 5 bullets\"");
    println!("  3. coven sessions");
    println!("\nSetup check:\n");
    run_doctor()
}

fn run_guided_harness_session() -> Result<()> {
    println!("{GOLD}Run an agent in this project{RESET}");
    println!("Coven will create a session record, validate the project root, then attach to the harness.\n");
    let default_harness = default_harness_id().unwrap_or("codex");
    let harness_prompt = format!("Harness [default: {default_harness}; options: codex, claude]: ");
    let harness =
        prompt_for_optional_line(&harness_prompt)?.unwrap_or_else(|| default_harness.to_string());
    let prompt = prompt_for_required_line("Task for the agent: ")?;
    let title = prompt_for_optional_line("Optional session title [enter to skip]: ")?;
    run_session(&harness, &[prompt], None, title.as_deref(), false)
}

fn run_sessions_command(include_archived: bool, manage: bool, plain: bool) -> Result<()> {
    match sessions_command_mode(
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
        manage,
        plain,
    ) {
        SessionsCommandMode::Browser => run_session_browser(include_archived),
        SessionsCommandMode::List => list_sessions(include_archived),
    }
}

fn sessions_command_mode(
    stdin_terminal: bool,
    stdout_terminal: bool,
    manage: bool,
    plain: bool,
) -> SessionsCommandMode {
    if plain {
        SessionsCommandMode::List
    } else if manage || (stdin_terminal && stdout_terminal) {
        SessionsCommandMode::Browser
    } else {
        SessionsCommandMode::List
    }
}

fn run_session_browser(include_archived: bool) -> Result<()> {
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
    enable_raw_mode().context("failed to enter Coven session browser")?;
    execute!(io::stdout(), EnableMouseCapture)
        .context("failed to enable Coven session browser mouse support")?;
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
            Event::Key(key) => match key.code {
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
    execute!(io::stdout(), DisableMouseCapture)
        .context("failed to disable Coven session browser mouse support")?;
    disable_raw_mode().context("failed to leave Coven session browser")?;
    println!();

    if let Some((session, action)) = selected {
        run_session_browser_action(&session, action)
    } else {
        println!("{PURPLE}Closed session browser. Nothing changed.{RESET}");
        Ok(())
    }
}

fn run_session_browser_action(
    session: &store::SessionRecord,
    action: SessionBrowserActionKind,
) -> Result<()> {
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
                println!("{PURPLE}Sacrifice cancelled. Nothing changed.{RESET}");
                return Ok(());
            }
            sacrifice_session_command(&session.id, true)
        }
        SessionBrowserActionKind::Back => {
            println!("{PURPLE}Closed session browser. Nothing changed.{RESET}");
            Ok(())
        }
    }
}

fn session_browser_actions(session: &store::SessionRecord) -> Vec<SessionBrowserAction> {
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

fn render_session_browser_frame_plain(
    sessions: &[store::SessionRecord],
    selected_session: usize,
    selected_action: usize,
) -> String {
    render_session_browser_frame_with_color(sessions, selected_session, selected_action, false)
}

fn render_session_browser_frame_for_raw_terminal(
    sessions: &[store::SessionRecord],
    selected_session: usize,
    selected_action: usize,
) -> String {
    render_session_browser_frame_with_color(sessions, selected_session, selected_action, true)
        .replace('\n', "\r\n")
}

fn render_session_browser_frame_with_color(
    sessions: &[store::SessionRecord],
    selected_session: usize,
    selected_action: usize,
    color_enabled: bool,
) -> String {
    let purple = ansi(color_enabled, PURPLE);
    let gold = ansi(color_enabled, GOLD);
    let rose = ansi(color_enabled, ROSE);
    let moon = ansi(color_enabled, MOON);
    let dim = ansi(color_enabled, DIM);
    let reset = ansi(color_enabled, RESET);
    let selected_session = selected_session.min(sessions.len().saturating_sub(1));
    let selected = &sessions[selected_session];
    let actions = session_browser_actions(selected);
    let selected_action = selected_action.min(actions.len().saturating_sub(1));
    let mut frame = String::new();

    frame.push_str(&format!("{gold}Session browser{reset}\n"));
    frame.push_str(&format!(
        "{moon}Select work, then choose an action. No IDs to copy.{reset}\n\n"
    ));
    frame.push_str(&format!(
        "{gold}Sessions{reset} {dim}(title | state | harness){reset}\n"
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
            gold
        } else {
            purple
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

    frame.push_str(&format!("\n{gold}Selected{reset}\n"));
    frame.push_str(&format!(
        "{rose}Title:{reset} {}\n",
        fit_chars(&selected.title, 50)
    ));
    frame.push_str(&format!(
        "{rose}Internal ID:{reset} {}  {rose}Runtime:{reset} {}  {rose}Harness:{reset} {}\n",
        first_chars(&selected.id, 18),
        selected.status,
        selected.harness
    ));
    frame.push_str(&format!(
        "{rose}Project:{reset} {}\n",
        fit_chars(&selected.project_root, 58)
    ));
    frame.push_str(&format!(
        "{rose}Updated:{reset} {}  {rose}State:{reset} {}\n",
        selected.updated_at,
        session_browser_status(selected)
    ));

    frame.push_str(&format!("\n{gold}Actions{reset}\n"));
    for (index, action) in actions.iter().enumerate() {
        let pointer = if index == selected_action { ">" } else { " " };
        let color = if index == selected_action {
            gold
        } else {
            purple
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

fn move_session_browser_selection(
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

fn session_browser_session_row_to_index(
    row: usize,
    displayed_count: usize,
    total_count: usize,
) -> Option<usize> {
    let index = row.checked_sub(SESSION_BROWSER_FIRST_SESSION_ROW)?;
    (index < displayed_count && index < total_count).then_some(index)
}

fn session_browser_action_row_to_index(
    row: usize,
    displayed_count: usize,
    has_more_sessions: bool,
    action_count: usize,
) -> Option<usize> {
    let extra_rows = usize::from(has_more_sessions);
    let first_action_row = SESSION_BROWSER_FIRST_SESSION_ROW + displayed_count + extra_rows + 8;
    let index = row.checked_sub(first_action_row)?;
    (index < action_count).then_some(index)
}

fn render_magical_tui_frame(selection: usize) -> String {
    render_magical_tui_frame_with_color_and_width(selection, true, magical_tui_inner_width())
}

fn render_magical_tui_frame_for_raw_terminal(selection: usize) -> String {
    render_magical_tui_frame(selection).replace('\n', "\r\n")
}

fn render_magical_tui_frame_plain(selection: usize) -> String {
    render_magical_tui_frame_with_color_and_width(selection, false, MAGICAL_TUI_DEFAULT_INNER_WIDTH)
}

#[cfg(test)]
fn render_magical_tui_frame_plain_with_width(selection: usize, inner_width: usize) -> String {
    render_magical_tui_frame_with_color_and_width(selection, false, inner_width)
}

fn render_magical_tui_frame_with_color_and_width(
    selection: usize,
    color_enabled: bool,
    inner_width: usize,
) -> String {
    let inner_width = normalized_magical_tui_inner_width(inner_width);
    let purple = ansi(color_enabled, PURPLE);
    let gold = ansi(color_enabled, GOLD);
    let rose = ansi(color_enabled, ROSE);
    let moon = ansi(color_enabled, MOON);
    let dim = ansi(color_enabled, DIM);
    let reset = ansi(color_enabled, RESET);
    let mut frame = String::new();
    frame.push_str(&magical_tui_line(
        "* Coven /command",
        gold,
        reset,
        inner_width,
    ));
    frame.push_str(&magical_tui_line(
        "Project sessions",
        rose,
        reset,
        inner_width,
    ));
    frame.push_str(&magical_tui_line(
        "Pick one. Enter casts.",
        moon,
        reset,
        inner_width,
    ));
    frame.push('\n');
    frame.push_str(&magical_tui_line("Commands", gold, reset, inner_width));
    frame.push_str(&magical_tui_line(
        "1-9/a/x/q + Enter",
        dim,
        reset,
        inner_width,
    ));
    frame.push('\n');

    for (index, item) in magical_tui_items().iter().enumerate() {
        let pointer = if index == selection { ">" } else { " " };
        let content = magical_tui_command_row(pointer, item, inner_width);
        let color = if index == selection { gold } else { purple };
        frame.push_str(&magical_tui_line(&content, color, reset, inner_width));
    }

    let selected = magical_tui_items()[selection.min(magical_tui_items().len() - 1)];
    frame.push('\n');
    frame.push_str(&magical_tui_line("Preview", gold, reset, inner_width));
    frame.push_str(&magical_tui_line(
        selected.description,
        moon,
        reset,
        inner_width,
    ));
    frame.push_str(&magical_tui_line(
        &format!("{} → {}", selected.slash, selected.command),
        gold,
        reset,
        inner_width,
    ));
    frame.push_str(&magical_tui_line(
        "Store: ~/.coven",
        dim,
        reset,
        inner_width,
    ));
    frame
}

fn magical_tui_line(content: &str, text_color: &str, reset: &str, inner_width: usize) -> String {
    format!("{text_color}{}{reset}\n", fit_chars(content, inner_width))
}

fn magical_tui_command_row(pointer: &str, item: &MagicalTuiItem, inner_width: usize) -> String {
    let row = format!("{pointer} {:<10} {}", item.slash, item.label);
    fit_chars(&row, inner_width)
}

fn magical_tui_inner_width() -> usize {
    crossterm::terminal::size()
        .map(|(columns, _)| magical_tui_inner_width_for_columns(columns as usize))
        .unwrap_or(MAGICAL_TUI_DEFAULT_INNER_WIDTH)
}

fn magical_tui_inner_width_for_columns(columns: usize) -> usize {
    let available = columns.saturating_sub(2);
    if available < MAGICAL_TUI_MIN_INNER_WIDTH {
        available.max(18)
    } else {
        available.min(MAGICAL_TUI_MAX_INNER_WIDTH)
    }
}

fn normalized_magical_tui_inner_width(inner_width: usize) -> usize {
    inner_width.clamp(18, MAGICAL_TUI_MAX_INNER_WIDTH)
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

fn ansi(enabled: bool, code: &'static str) -> &'static str {
    if enabled {
        code
    } else {
        ""
    }
}

fn move_magical_tui_selection(current: usize, direction: MagicalTuiMove) -> usize {
    let item_count = magical_tui_items().len();
    match direction {
        MagicalTuiMove::Up => current.checked_sub(1).unwrap_or(item_count - 1),
        MagicalTuiMove::Down => (current + 1) % item_count,
    }
}

fn run_doctor() -> Result<()> {
    println!("Coven doctor");
    println!("Store: {}", coven_home_dir()?.display());
    match std::env::current_dir()
        .ok()
        .and_then(|cwd| project::canonical_project_root(&cwd).ok())
    {
        Some(root) => println!("Project: {}", root.display()),
        None => println!("Project: not inside a git/project root yet"),
    }

    println!("\nHarnesses:");
    let harnesses = harness::built_in_harnesses();
    for harness in &harnesses {
        let status = if harness.available {
            "ready"
        } else {
            "missing"
        };
        let marker = if harness.available { "OK" } else { "!!" };
        println!(
            "  [{marker}] {:<11} `{}` is {status}",
            harness.label, harness.executable
        );
        if !harness.available {
            println!("       {}", harness.install_hint);
        }
    }

    println!("\nNext steps:");
    if let Some(default) = default_harness_id() {
        println!("  coven run {default} \"explain this repo in 5 bullets\"");
        println!("  coven sessions");
    } else {
        println!("  Install or authenticate Codex/Claude Code, then run `coven doctor` again.");
    }
    Ok(())
}

fn run_patch_command(command: PatchCommand) -> Result<()> {
    match command {
        PatchCommand::OpenClaw {
            issue,
            repo,
            harness,
            verify,
            non_interactive,
            dry_run,
            keep_session,
        } => run_patch_openclaw(
            issue,
            repo,
            harness,
            verify,
            non_interactive,
            dry_run,
            keep_session,
        ),
    }
}

fn run_patch_openclaw(
    issue: Vec<String>,
    repo: Option<PathBuf>,
    harness: Option<String>,
    verify: Option<String>,
    non_interactive: bool,
    dry_run: bool,
    _keep_session: bool,
) -> Result<()> {
    let start_dir = std::env::current_dir().context("failed to read current directory")?;
    let detected_repo = openclaw_repo::detect_openclaw_repo(repo.as_deref(), &start_dir)?;
    let git_state = openclaw_repo::inspect_git_state(&detected_repo.root)?;
    let issue = match joined_optional_issue(issue)? {
        Some(issue) => issue,
        None if non_interactive => anyhow::bail!("issue text is required with --non-interactive"),
        None => prompt_for_required_line("What is broken in OpenClaw? ")?,
    };
    let harness_id = match harness {
        Some(harness) => patch::HarnessId::parse(&harness)?,
        None if non_interactive => anyhow::bail!("--harness is required with --non-interactive"),
        None => choose_default_harness()?,
    };
    let verification_profile = patch::VerificationProfile::parse(verify.as_deref())?;

    let request = patch::PatchOpenClawRequest {
        repo: detected_repo,
        git_state,
        issue,
        harness_id,
        verification_profile,
        non_interactive,
        dry_run,
        keep_session: _keep_session,
    };

    println!("{}", patch::summarize_patch_plan(&request));
    if dry_run {
        println!("\nRepair brief:\n{}", patch::build_repair_brief(&request));
        return Ok(());
    }

    if request.git_state.is_dirty() && !request.non_interactive {
        println!("\nExisting changes were detected. Coven will not stash or overwrite them.");
        if !confirm_yes("Continue and ask the harness to preserve existing changes? [y/N] ")? {
            anyhow::bail!("cancelled before harness launch");
        }
    }

    if !request.non_interactive && !confirm_yes("Launch the harness now? [y/N] ")? {
        anyhow::bail!("cancelled before harness launch");
    }

    let session_id = launch_patch_session(&request)?;
    let verification_results =
        verification::run_verification(&request.repo.root, &request.verification_profile)?;
    let verification_lines = verification_results
        .into_iter()
        .map(|result| match result.status {
            verification::VerificationStatus::Passed => format!("{} passed", result.command),
            verification::VerificationStatus::Failed(code) => {
                format!("{} failed with exit code {}", result.command, code)
            }
        })
        .collect::<Vec<_>>();
    let changed_files = openclaw_repo::changed_files(&request.repo.root)?;
    let status = if verification_lines
        .iter()
        .any(|line| line.contains(" failed "))
    {
        "verification failed"
    } else if changed_files.is_empty() {
        "blocked"
    } else {
        "patched"
    };

    println!(
        "{}",
        patch::summarize_patch_report(&patch::PatchOpenClawReport {
            status: status.to_string(),
            session_id,
            changed_files,
            verification: verification_lines,
        })
    );
    Ok(())
}

fn joined_optional_issue(issue: Vec<String>) -> Result<Option<String>> {
    if issue.is_empty() {
        return Ok(None);
    }
    let joined = issue.join(" ").trim().to_string();
    if joined.is_empty() {
        anyhow::bail!("issue text must not be empty when provided");
    }
    Ok(Some(joined))
}

fn prompt_for_required_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush().context("failed to flush prompt")?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("failed to read input")?;
    let line = line.trim().to_string();
    if line.is_empty() {
        anyhow::bail!("a response is required");
    }
    Ok(line)
}

fn prompt_for_optional_line(prompt: &str) -> Result<Option<String>> {
    print!("{prompt}");
    io::stdout().flush().context("failed to flush prompt")?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("failed to read input")?;
    let line = line.trim().to_string();
    Ok((!line.is_empty()).then_some(line))
}

fn confirm_yes(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush().context("failed to flush prompt")?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("failed to read input")?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "YES" | "Yes"))
}

fn choose_default_harness() -> Result<patch::HarnessId> {
    let harnesses = harness::built_in_harnesses();
    if harnesses.iter().any(|h| h.id == "codex" && h.available) {
        return Ok(patch::HarnessId::Codex);
    }
    if harnesses.iter().any(|h| h.id == "claude" && h.available) {
        return Ok(patch::HarnessId::ClaudeCode);
    }
    anyhow::bail!("no supported harness is available; run `coven doctor` for setup guidance")
}

fn default_harness_id() -> Option<&'static str> {
    let harnesses = harness::built_in_harnesses();
    harnesses
        .iter()
        .find(|h| h.id == "codex" && h.available)
        .or_else(|| harnesses.iter().find(|h| h.id == "claude" && h.available))
        .map(|h| h.id)
}

fn launch_patch_session(request: &patch::PatchOpenClawRequest) -> Result<String> {
    let selected_harness = selected_available_harness(request.harness_id.as_str())?;
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let now = current_timestamp();
    let brief = patch::build_repair_brief(request);
    let record = store::SessionRecord {
        id: Uuid::new_v4().to_string(),
        project_root: request.repo.root.to_string_lossy().into_owned(),
        harness: selected_harness.id.to_string(),
        title: session_title(Some("Patch OpenClaw"), &brief),
        status: DEFAULT_SESSION_STATUS.to_string(),
        exit_code: None,
        archived_at: None,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    store::insert_session(&conn, &record)?;
    store::insert_json_event(
        &conn,
        &record.id,
        "patch_metadata",
        &serde_json::json!({
            "patchTarget": "openclaw",
            "repoRoot": request.repo.root,
            "issue": request.issue,
            "harnessId": request.harness_id.as_str(),
            "verificationProfile": request.verification_profile.as_str(),
            "status": "running"
        }),
        &now,
    )?;

    store::update_session_status(
        &conn,
        &record.id,
        RUNNING_SESSION_STATUS,
        None,
        &current_timestamp(),
    )?;
    let command = pty_runner::build_harness_command(
        selected_harness.id,
        &brief,
        &request.repo.root,
        harness_launch_mode_for_stdio(),
    )?;
    let result = pty_runner::run_attached(&command)?;
    store::update_session_status(
        &conn,
        &record.id,
        result.status,
        result.exit_code,
        &current_timestamp(),
    )?;
    Ok(record.id)
}

fn run_daemon_command(command: DaemonCommand) -> Result<()> {
    let home = coven_home_dir()?;
    match command {
        DaemonCommand::Start => {
            let current_exe =
                std::env::current_exe().context("failed to resolve current executable")?;
            let status = daemon::start_background_server(&home, &current_exe, current_timestamp())?;
            println!(
                "coven daemon status=running pid={} socket={}",
                status.pid, status.socket
            );
        }
        DaemonCommand::Restart => {
            let was_running = daemon::stop_background_server(&home)?;
            let current_exe =
                std::env::current_exe().context("failed to resolve current executable")?;
            let status = daemon::start_background_server(&home, &current_exe, current_timestamp())?;
            if was_running {
                println!(
                    "coven daemon status=restarted pid={} socket={}",
                    status.pid, status.socket
                );
            } else {
                println!(
                    "coven daemon status=running pid={} socket={}",
                    status.pid, status.socket
                );
            }
        }
        DaemonCommand::Status => match daemon::read_status(&home)? {
            Some(status) => {
                let health = api::health_response(Some(status.clone()));
                println!(
                    "coven daemon status=running ok={} pid={} socket={}",
                    health.ok, status.pid, status.socket
                );
            }
            None => println!("coven daemon status=stopped"),
        },
        DaemonCommand::Stop => {
            if daemon::stop_background_server(&home)? {
                println!("coven daemon status=stopped");
            } else {
                println!("coven daemon was not running");
            }
        }
        DaemonCommand::Serve => {
            #[cfg(unix)]
            {
                daemon::serve_forever(&home, current_timestamp())?;
            }
            #[cfg(not(unix))]
            {
                anyhow::bail!(
                    "coven daemon server is only implemented on Unix-like systems for now"
                );
            }
        }
    }
    Ok(())
}

fn harness_launch_mode_for_stdio() -> harness::HarnessLaunchMode {
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        harness::HarnessLaunchMode::Interactive
    } else {
        harness::HarnessLaunchMode::NonInteractive
    }
}

fn run_session(
    harness_id: &str,
    prompt_args: &[String],
    cwd: Option<&Path>,
    title: Option<&str>,
    detach: bool,
) -> Result<()> {
    let prompt = joined_prompt(prompt_args)?;
    let selected_harness = selected_available_harness(harness_id)?;
    let current_dir = std::env::current_dir().context("failed to read current directory")?;
    let project_root = project::canonical_project_root(&current_dir).with_context(|| {
        format!(
            "failed to resolve project root from {}",
            current_dir.display()
        )
    })?;
    let cwd = project::resolve_inside_root(&project_root, cwd).context("failed to resolve cwd")?;
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
    let record = store::SessionRecord {
        id: Uuid::new_v4().to_string(),
        project_root: project_root.to_string_lossy().into_owned(),
        harness: selected_harness.id.to_string(),
        title: session_title(title, &prompt),
        status: DEFAULT_SESSION_STATUS.to_string(),
        exit_code: None,
        archived_at: None,
        created_at: now.clone(),
        updated_at: now,
    };

    store::insert_session(&conn, &record)?;

    println!("Coven session created");
    println!("  id:      {}", record.id);
    println!("  harness: {}", record.harness);
    println!("  cwd:     {}", cwd.display());
    println!("  title:   {}", record.title);

    if detach {
        println!("\nDetached mode: session was recorded but the harness was not spawned.");
        println!("View it later with `coven sessions`.");
        return Ok(());
    }

    store::update_session_status(
        &conn,
        &record.id,
        RUNNING_SESSION_STATUS,
        None,
        &current_timestamp(),
    )?;

    let command = pty_runner::build_harness_command(
        selected_harness.id,
        &prompt,
        &cwd,
        harness_launch_mode_for_stdio(),
    )?;
    match pty_runner::run_attached(&command) {
        Ok(result) => {
            store::update_session_status(
                &conn,
                &record.id,
                result.status,
                result.exit_code,
                &current_timestamp(),
            )?;
            Ok(())
        }
        Err(error) => {
            store::update_session_status(
                &conn,
                &record.id,
                FAILED_SESSION_STATUS,
                None,
                &current_timestamp(),
            )?;
            Err(error)
        }
    }
}

fn list_sessions(include_archived: bool) -> Result<()> {
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
            "{:<12} {:<10} {:<8} {:<8} TITLE",
            "SESSION", "STATUS", "HARNESS", "RITUAL"
        );
        println!(
            "{:<12} {:<10} {:<8} {:<8} -----",
            "-------", "------", "-------", "------"
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

fn archive_session_command(session_id: &str) -> Result<()> {
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let Some(session) = store::get_session(&conn, session_id)? else {
        anyhow::bail!("session `{session_id}` not found");
    };
    if session.status == RUNNING_SESSION_STATUS {
        anyhow::bail!("session `{session_id}` is still running; stop it before archiving");
    }

    store::archive_session(&conn, session_id, &current_timestamp())?;
    println!("archived session {session_id}");
    println!(
        "Summon it later with `coven summon {session_id}` or view it with `coven sessions --all`."
    );
    Ok(())
}

fn summon_session_command(session_id: &str) -> Result<()> {
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let Some(session) = store::get_session(&conn, session_id)? else {
        anyhow::bail!("session `{session_id}` not found");
    };

    if session.archived_at.is_some() {
        store::summon_session(&conn, session_id, &current_timestamp())?;
        eprintln!("summoned session {session_id} from the archive");
    }

    attach_session(session_id)
}

fn sacrifice_session_command(session_id: &str, yes: bool) -> Result<()> {
    confirm_sacrifice(session_id, yes)?;
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let Some(session) = store::get_session(&conn, session_id)? else {
        anyhow::bail!("session `{session_id}` not found");
    };
    if session.status == RUNNING_SESSION_STATUS {
        anyhow::bail!("session `{session_id}` is still running; do not sacrifice live work");
    }

    store::sacrifice_session(&conn, session_id)?;
    println!("sacrificed session {session_id}; its event log was permanently deleted");
    Ok(())
}

fn confirm_sacrifice(session_id: &str, yes: bool) -> Result<()> {
    if yes {
        Ok(())
    } else {
        anyhow::bail!(
            "sacrifice permanently deletes session `{session_id}` and its events; rerun with --yes to confirm"
        )
    }
}

fn attach_session(session_id: &str) -> Result<()> {
    let home = coven_home_dir()?;
    let store_path = home.join(STORE_FILE_NAME);
    let conn = store::open_store(&store_path)?;
    let Some(session) = store::get_session(&conn, session_id)? else {
        anyhow::bail!("session `{session_id}` not found");
    };

    eprintln!(
        "attached to session {} status={} harness={} title={} ",
        session.id, session.status, session.harness, session.title
    );

    maybe_spawn_input_forwarder(home.clone(), session_id.to_string());

    let mut seen = HashSet::new();
    loop {
        let events = store::list_events(&conn, session_id)?;
        for event in printable_new_events(&events, &mut seen) {
            print!("{event}");
            io::stdout()
                .flush()
                .context("failed to flush session output")?;
        }

        let status = store::get_session(&conn, session_id)?
            .map(|session| session.status)
            .unwrap_or_else(|| "missing".to_string());
        if status != RUNNING_SESSION_STATUS {
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }

    Ok(())
}

fn printable_new_events(events: &[store::EventRecord], seen: &mut HashSet<String>) -> Vec<String> {
    events
        .iter()
        .filter(|event| seen.insert(event.id.clone()))
        .filter_map(printable_event_text)
        .collect()
}

fn printable_event_text(event: &store::EventRecord) -> Option<String> {
    match event.kind.as_str() {
        "output" => serde_json::from_str::<serde_json::Value>(&event.payload_json)
            .ok()?
            .get("data")?
            .as_str()
            .map(ToOwned::to_owned),
        "exit" => {
            let payload = serde_json::from_str::<serde_json::Value>(&event.payload_json).ok()?;
            let status = payload.get("status")?.as_str()?;
            let exit_code = payload
                .get("exitCode")
                .and_then(serde_json::Value::as_i64)
                .map(|code| format!(" exitCode={code}"))
                .unwrap_or_default();
            Some(format!("\n[coven session {status}{exit_code}]\n"))
        }
        _ => None,
    }
}

fn maybe_spawn_input_forwarder(coven_home: PathBuf, session_id: String) {
    if !io::stdin().is_terminal() {
        return;
    }

    thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let Ok(mut line) = line else {
                break;
            };
            line.push('\n');
            let _ = send_session_input(&coven_home, &session_id, &line);
        }
    });
}

#[cfg(unix)]
fn send_session_input(coven_home: &Path, session_id: &str, data: &str) -> Result<()> {
    use std::os::unix::net::UnixStream;

    let socket = daemon::daemon_socket_path(coven_home);
    let body = serde_json::json!({ "data": data }).to_string();
    let request = format!(
        "POST /sessions/{session_id}/input HTTP/1.1\r\nHost: coven\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let mut stream = UnixStream::connect(&socket).with_context(|| {
        format!(
            "failed to connect to Coven daemon socket {}",
            socket.display()
        )
    })?;
    stream
        .write_all(request.as_bytes())
        .context("failed to write Coven input request")?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to finish Coven input request")?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .context("failed to read Coven input response")?;
    ensure_successful_http_response(&response)
}

#[cfg(not(unix))]
fn send_session_input(_coven_home: &Path, _session_id: &str, _data: &str) -> Result<()> {
    anyhow::bail!("Coven attach input forwarding is only implemented on Unix-like systems for now")
}

fn ensure_successful_http_response(response: &str) -> Result<()> {
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .context("invalid Coven daemon response")?;
    if (200..300).contains(&status) {
        Ok(())
    } else {
        anyhow::bail!("Coven daemon rejected input with HTTP {status}")
    }
}

fn selected_available_harness(harness_id: &str) -> Result<harness::HarnessSummary> {
    let harnesses = harness::built_in_harnesses();
    let known_harnesses = harnesses
        .iter()
        .map(|harness| harness.id)
        .collect::<Vec<_>>()
        .join(", ");
    let selected = harnesses
        .into_iter()
        .find(|harness| harness.id == harness_id);

    match selected {
        Some(harness) if harness.available => Ok(harness),
        Some(harness) => Err(anyhow!(
            "harness `{}` is not available. {}",
            harness.id,
            harness.install_hint
        )),
        None => Err(anyhow!(
            "unknown harness `{harness_id}`. Built-in harnesses: {known_harnesses}"
        )),
    }
}

fn joined_prompt(prompt_args: &[String]) -> Result<String> {
    let prompt = prompt_args.join(" ");
    let prompt = prompt.trim();
    if prompt.is_empty() {
        anyhow::bail!("prompt must not be empty");
    }
    Ok(prompt.to_string())
}

fn current_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn session_title(title: Option<&str>, prompt: &str) -> String {
    title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| first_chars(prompt, DEFAULT_TITLE_CHARS))
}

fn first_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn coven_store_path() -> Result<PathBuf> {
    let home = coven_home_dir()?;
    std::fs::create_dir_all(&home)
        .with_context(|| format!("failed to create Coven home directory {}", home.display()))?;
    Ok(home.join(STORE_FILE_NAME))
}

fn coven_home_dir() -> Result<PathBuf> {
    coven_home_from_env(std::env::var_os("COVEN_HOME"), std::env::var_os("HOME"))
}

fn coven_home_from_env(coven_home: Option<OsString>, home: Option<OsString>) -> Result<PathBuf> {
    if let Some(coven_home) = coven_home.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(coven_home));
    }

    let home =
        home.ok_or_else(|| anyhow!("HOME is not set; set COVEN_HOME to choose a store path"))?;
    Ok(PathBuf::from(home).join(DEFAULT_COVEN_HOME_DIR))
}

fn format_session_line(session: &store::SessionRecord) -> String {
    let short_id = first_chars(&session.id, 12);
    let ritual = if session.archived_at.is_some() {
        "archived"
    } else {
        "active"
    };
    format!(
        "{:<12} {:<10} {:<8} {:<8} {}",
        short_id, session.status, session.harness, ritual, session.title
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joined_prompt_rejects_empty_prompt() {
        let error = joined_prompt(&[" ".to_string(), "\t".to_string()]).unwrap_err();

        assert!(
            error.to_string().contains("prompt must not be empty"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn joined_prompt_joins_prompt_args_with_spaces() -> Result<()> {
        assert_eq!(
            joined_prompt(&["hello".to_string(), "from".to_string(), "coven".to_string()])?,
            "hello from coven"
        );
        Ok(())
    }

    #[test]
    fn session_title_uses_provided_title_when_present() {
        assert_eq!(
            session_title(Some(" Custom title "), "prompt text"),
            "Custom title"
        );
    }

    #[test]
    fn session_title_uses_first_48_prompt_chars_by_default() {
        let prompt = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

        assert_eq!(
            session_title(None, prompt),
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUV"
        );
    }

    #[test]
    fn coven_home_from_env_respects_coven_home() -> Result<()> {
        let path = coven_home_from_env(
            Some(OsString::from("/tmp/custom-coven-home")),
            Some(OsString::from("/tmp/ignored-home")),
        )?;

        assert_eq!(path, PathBuf::from("/tmp/custom-coven-home"));
        Ok(())
    }

    #[test]
    fn coven_home_from_env_defaults_under_home() -> Result<()> {
        let path = coven_home_from_env(None, Some(OsString::from("/tmp/user-home")))?;

        assert_eq!(path, PathBuf::from("/tmp/user-home").join(".coven"));
        Ok(())
    }

    #[test]
    fn cli_defaults_to_magical_tui_when_no_subcommand_is_provided() {
        let cli = Cli::parse_from(["coven"]);

        assert!(cli.command.is_none());
    }

    #[test]
    fn cli_accepts_explicit_tui_command() {
        let cli = Cli::parse_from(["coven", "tui"]);

        match cli.command {
            Some(Command::Tui) => {}
            other => panic!("expected tui command, got {other:?}"),
        }
    }

    #[test]
    fn magical_tui_frame_uses_purple_gold_branding_and_lists_core_actions() {
        let frame = render_magical_tui_frame_plain(1);

        assert!(frame.contains("Coven"));
        assert!(frame.contains("/command"));
        assert!(frame.contains("/start"));
        assert!(frame.contains("/run"));
        assert!(frame.contains("/patch"));
        assert!(frame.contains("/doctor"));
        assert!(frame.contains(">"));
    }

    #[test]
    fn magical_tui_lists_full_slash_command_suite() {
        let slashes = magical_tui_items()
            .iter()
            .map(|item| item.slash)
            .collect::<Vec<_>>();

        assert_eq!(
            slashes,
            vec![
                "/start",
                "/tui",
                "/doctor",
                "/daemon",
                "/run",
                "/patch",
                "/sessions",
                "/all",
                "/attach",
                "/summon",
                "/archive",
                "/sacrifice",
                "/quit",
            ]
        );
    }

    #[test]
    fn magical_tui_selection_wraps_around() {
        assert_eq!(
            move_magical_tui_selection(0, MagicalTuiMove::Up),
            magical_tui_items().len() - 1
        );
        assert_eq!(
            move_magical_tui_selection(magical_tui_items().len() - 1, MagicalTuiMove::Down),
            0
        );
    }

    #[test]
    fn magical_tui_frame_previews_selected_spell_command() {
        let frame = render_magical_tui_frame_plain(0);

        assert!(frame.contains("Preview"));
        assert!(frame.contains("/start"));
        assert!(frame.contains("coven doctor"));
        assert!(frame.contains("~/.coven"));
    }

    #[test]
    fn magical_tui_frame_is_newcomer_friendly() {
        let frame = render_magical_tui_frame_plain(4);

        assert!(frame.contains("Project sessions"));
        assert!(frame.contains("Enter casts"));
        assert!(frame.contains("Commands"));
        assert!(frame.contains("Launch Codex"));
        assert!(frame.contains("coven run codex"));
    }

    #[test]
    fn magical_tui_width_tracks_terminal_columns_without_overflowing() {
        assert_eq!(
            magical_tui_inner_width_for_columns(120),
            MAGICAL_TUI_MAX_INNER_WIDTH
        );
        assert_eq!(
            magical_tui_inner_width_for_columns(80),
            MAGICAL_TUI_MAX_INNER_WIDTH
        );
        assert_eq!(magical_tui_inner_width_for_columns(36), 24);
    }

    #[test]
    fn magical_tui_frame_truncates_narrow_rows_with_ellipsis() {
        let frame = render_magical_tui_frame_plain_with_width(1, 34);

        assert!(frame.contains("/run"));
        assert!(frame.contains('…'));
        for line in frame.lines() {
            assert!(
                line.chars().count() <= 36,
                "line exceeded requested narrow frame: {line}"
            );
        }
    }

    #[test]
    fn magical_tui_raw_terminal_frame_uses_crlf_to_avoid_stair_step_rendering() {
        let frame = render_magical_tui_frame_for_raw_terminal(0);

        assert!(frame.contains("\r\n"));
        assert!(!frame.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn cli_accepts_daemon_start_status_stop_restart_and_hidden_serve_commands() {
        for subcommand in ["start", "status", "stop", "restart", "serve"] {
            let cli = Cli::parse_from(["coven", "daemon", subcommand]);
            match cli.command {
                Some(Command::Daemon { .. }) => {}
                other => panic!("expected daemon command, got {other:?}"),
            }
        }
    }

    #[test]
    fn cli_run_defaults_to_attached_and_accepts_detach() {
        let attached = Cli::parse_from(["coven", "run", "codex", "hello"]);
        let detached = Cli::parse_from(["coven", "run", "codex", "hello", "--detach"]);

        match attached.command {
            Some(Command::Run { detach, .. }) => assert!(!detach),
            other => panic!("expected run command, got {other:?}"),
        }

        match detached.command {
            Some(Command::Run { detach, .. }) => assert!(detach),
            other => panic!("expected run command, got {other:?}"),
        }
    }

    #[test]
    fn cli_accepts_attach_command() {
        let cli = Cli::parse_from(["coven", "attach", "session-1"]);

        match cli.command {
            Some(Command::Attach { session_id }) => assert_eq!(session_id, "session-1"),
            other => panic!("expected attach command, got {other:?}"),
        }
    }

    #[test]
    fn cli_accepts_coven_session_ritual_verbs() {
        let sessions = Cli::parse_from(["coven", "sessions", "--all"]);
        match sessions.command {
            Some(Command::Sessions { all, manage, plain }) => {
                assert!(all);
                assert!(!manage);
                assert!(!plain);
            }
            other => panic!("expected sessions command, got {other:?}"),
        }

        let managed = Cli::parse_from(["coven", "sessions", "--manage", "--plain"]);
        match managed.command {
            Some(Command::Sessions { manage, plain, .. }) => {
                assert!(manage);
                assert!(plain);
            }
            other => panic!("expected sessions command, got {other:?}"),
        }

        let summon = Cli::parse_from(["coven", "summon", "session-1"]);
        match summon.command {
            Some(Command::Summon { session_id }) => assert_eq!(session_id, "session-1"),
            other => panic!("expected summon command, got {other:?}"),
        }

        let archive = Cli::parse_from(["coven", "archive", "session-1"]);
        match archive.command {
            Some(Command::Archive { session_id }) => assert_eq!(session_id, "session-1"),
            other => panic!("expected archive command, got {other:?}"),
        }

        let sacrifice = Cli::parse_from(["coven", "sacrifice", "session-1", "--yes"]);
        match sacrifice.command {
            Some(Command::Sacrifice { session_id, yes }) => {
                assert_eq!(session_id, "session-1");
                assert!(yes);
            }
            other => panic!("expected sacrifice command, got {other:?}"),
        }
    }

    #[test]
    fn sessions_command_opens_browser_for_humans_and_plain_tables_for_scripts() {
        assert_eq!(
            sessions_command_mode(true, true, false, false),
            SessionsCommandMode::Browser
        );
        assert_eq!(
            sessions_command_mode(false, true, false, false),
            SessionsCommandMode::List
        );
        assert_eq!(
            sessions_command_mode(false, false, true, false),
            SessionsCommandMode::Browser
        );
        assert_eq!(
            sessions_command_mode(true, true, true, true),
            SessionsCommandMode::List
        );
    }

    #[test]
    fn sacrifice_requires_explicit_yes() {
        assert!(confirm_sacrifice("session-1", false).is_err());
        assert!(confirm_sacrifice("session-1", true).is_ok());
    }

    #[test]
    fn printable_event_text_extracts_output_payload() {
        let event = store::EventRecord {
            id: "event-1".to_string(),
            session_id: "session-1".to_string(),
            kind: "output".to_string(),
            payload_json: r#"{"data":"hello\n"}"#.to_string(),
            created_at: "2026-04-27T10:00:00Z".to_string(),
        };

        assert_eq!(printable_event_text(&event).as_deref(), Some("hello\n"));
    }

    #[test]
    fn printable_event_text_formats_exit_payload() {
        let event = store::EventRecord {
            id: "event-1".to_string(),
            session_id: "session-1".to_string(),
            kind: "exit".to_string(),
            payload_json: r#"{"status":"completed","exitCode":0}"#.to_string(),
            created_at: "2026-04-27T10:00:00Z".to_string(),
        };

        assert_eq!(
            printable_event_text(&event).as_deref(),
            Some("\n[coven session completed exitCode=0]\n")
        );
    }

    #[test]
    fn successful_http_response_accepts_2xx_only() {
        assert!(ensure_successful_http_response("HTTP/1.1 202 Accepted\r\n\r\n{}").is_ok());
        assert!(ensure_successful_http_response("HTTP/1.1 409 Conflict\r\n\r\n{}").is_err());
    }

    #[test]
    fn cli_accepts_patch_openclaw_guided_command() {
        let cli = Cli::parse_from(["coven", "patch", "openclaw"]);

        match cli.command {
            Some(Command::Patch {
                command:
                    PatchCommand::OpenClaw {
                        issue,
                        repo,
                        harness,
                        verify,
                        non_interactive,
                        dry_run,
                        keep_session,
                    },
            }) => {
                assert!(issue.is_empty());
                assert!(repo.is_none());
                assert!(harness.is_none());
                assert!(verify.is_none());
                assert!(!non_interactive);
                assert!(!dry_run);
                assert!(!keep_session);
            }
            other => panic!("expected patch openclaw command, got {other:?}"),
        }
    }

    #[test]
    fn cli_accepts_patch_openclaw_fast_command() {
        let cli = Cli::parse_from([
            "coven",
            "patch",
            "openclaw",
            "fix auth order",
            "--repo",
            "/repo/openclaw",
            "--harness",
            "codex",
            "--verify",
            "pnpm-check",
            "--non-interactive",
            "--dry-run",
            "--keep-session",
        ]);

        match cli.command {
            Some(Command::Patch {
                command:
                    PatchCommand::OpenClaw {
                        issue,
                        repo,
                        harness,
                        verify,
                        non_interactive,
                        dry_run,
                        keep_session,
                    },
            }) => {
                assert_eq!(issue, vec!["fix auth order".to_string()]);
                assert_eq!(repo.as_deref(), Some(Path::new("/repo/openclaw")));
                assert_eq!(harness.as_deref(), Some("codex"));
                assert_eq!(verify.as_deref(), Some("pnpm-check"));
                assert!(non_interactive);
                assert!(dry_run);
                assert!(keep_session);
            }
            other => panic!("expected patch openclaw command, got {other:?}"),
        }
    }

    #[test]
    fn joined_optional_issue_returns_none_for_guided_mode() -> Result<()> {
        assert_eq!(joined_optional_issue(vec![])?, None);
        Ok(())
    }

    #[test]
    fn joined_optional_issue_joins_fast_issue_text() -> Result<()> {
        assert_eq!(
            joined_optional_issue(vec!["fix".to_string(), "auth".to_string()])?,
            Some("fix auth".to_string())
        );
        Ok(())
    }

    #[test]
    fn format_session_line_prints_id_status_harness_and_title() {
        let session = store::SessionRecord {
            id: "session-id".to_string(),
            project_root: "/tmp/project".to_string(),
            harness: "codex".to_string(),
            title: "A useful session".to_string(),
            status: "created".to_string(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-04-27T06:00:00Z".to_string(),
            updated_at: "2026-04-27T06:00:00Z".to_string(),
        };

        assert_eq!(
            format_session_line(&session),
            "session-id   created    codex    active   A useful session"
        );
    }

    #[test]
    fn session_browser_frame_shows_context_actions_and_no_copy_paste_id_prompt() {
        let sessions = vec![test_session_record(
            "session-alpha-1234567890",
            "completed",
            "codex",
            "Fix the failing tests before demo",
            None,
        )];

        let frame = render_session_browser_frame_plain(&sessions, 0, 0);

        assert!(frame.contains("Session browser"));
        assert!(frame.contains("Fix the failing tests"));
        assert!(frame.contains("completed"));
        assert!(frame.contains("codex"));
        assert!(frame.contains("Actions"));
        assert!(frame.contains("View Log"));
        assert!(frame.contains("Archive"));
        assert!(frame.contains("Sacrifice"));
        assert!(frame.contains("session-alpha"));
        assert!(!frame.contains("<session-id>"));
    }

    #[test]
    fn session_browser_actions_are_contextual_for_archived_sessions() {
        let sessions = [test_session_record(
            "archived-session-123456",
            "completed",
            "claude",
            "Polish the UI",
            Some("2026-05-08T07:00:00Z"),
        )];

        let actions = session_browser_actions(&sessions[0]);

        assert!(actions.iter().any(|action| action.label == "Summon"));
        assert!(!actions.iter().any(|action| action.label == "Archive"));
    }

    #[test]
    fn session_browser_primary_action_uses_human_labels() {
        let running = test_session_record(
            "running-session-123",
            RUNNING_SESSION_STATUS,
            "codex",
            "Live agent",
            None,
        );
        let completed = test_session_record(
            "completed-session-123",
            "completed",
            "codex",
            "Past agent",
            None,
        );

        assert_eq!(session_browser_actions(&running)[0].label, "Rejoin");
        assert_eq!(session_browser_actions(&completed)[0].label, "View Log");
    }

    #[test]
    fn session_browser_maps_click_rows_to_sessions_and_actions() {
        assert_eq!(
            session_browser_session_row_to_index(SESSION_BROWSER_FIRST_SESSION_ROW, 3, 3),
            Some(0)
        );
        assert_eq!(
            session_browser_session_row_to_index(SESSION_BROWSER_FIRST_SESSION_ROW + 2, 3, 3),
            Some(2)
        );
        assert_eq!(
            session_browser_action_row_to_index(
                SESSION_BROWSER_FIRST_SESSION_ROW + 3 + 8,
                3,
                false,
                4,
            ),
            Some(0)
        );
    }

    fn test_session_record(
        id: &str,
        status: &str,
        harness: &str,
        title: &str,
        archived_at: Option<&str>,
    ) -> store::SessionRecord {
        store::SessionRecord {
            id: id.to_string(),
            project_root: "/tmp/project".to_string(),
            harness: harness.to_string(),
            title: title.to_string(),
            status: status.to_string(),
            exit_code: None,
            archived_at: archived_at.map(ToOwned::to_owned),
            created_at: "2026-05-08T07:00:00Z".to_string(),
            updated_at: "2026-05-08T07:05:00Z".to_string(),
        }
    }
}
