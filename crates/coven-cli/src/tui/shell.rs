use std::io::{self, IsTerminal, Write};

use anyhow::{anyhow, Context, Result};
use crossterm::{
    cursor::MoveTo,
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
};

use super::{is_key_press, sessions};
use crate::{
    archive_session_command, attach_session, default_harness_id, prompt_for_optional_line,
    prompt_for_required_line, run_daemon_command, run_doctor, run_patch_openclaw, run_session,
    sacrifice_session_command, summon_session_command, theme, DaemonCommand,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MagicalTuiAction {
    StartHere,
    Help,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MagicalTuiRequest {
    Action(MagicalTuiAction),
    NaturalPrompt(String),
    HarnessPrompt { harness: String, prompt: String },
    AttachSession(String),
    SummonSession(String),
    ArchiveSession(String),
    SacrificeSession(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MagicalTuiMove {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MagicalTuiItem {
    pub(crate) key: &'static str,
    pub(crate) slash: &'static str,
    pub(crate) label: &'static str,
    pub(crate) description: &'static str,
    pub(crate) command: &'static str,
    pub(crate) action: MagicalTuiAction,
}

const MAGICAL_TUI_DEFAULT_INNER_WIDTH: usize = 76;
pub(crate) const MAGICAL_TUI_MAX_INNER_WIDTH: usize = 96;
const MAGICAL_TUI_MIN_INNER_WIDTH: usize = 40;

/// Restores terminal raw mode on all exit paths for the slash-command launcher.
struct RawModeGuard {
    enabled: bool,
}

impl RawModeGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("failed to enter Coven's magical terminal mode")?;
        Ok(Self { enabled: true })
    }

    fn restore(&mut self) -> Result<()> {
        if self.enabled {
            disable_raw_mode().context("failed to leave Coven's magical terminal mode")?;
            self.enabled = false;
        }
        Ok(())
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = disable_raw_mode();
        }
    }
}

pub(crate) fn magical_tui_items() -> &'static [MagicalTuiItem] {
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
            key: "h",
            slash: "/help",
            label: "Help",
            description: "Show natural-language and slash-command examples",
            command: "type a task or /run codex <task>",
            action: MagicalTuiAction::Help,
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

pub(crate) fn run() -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        println!("{}", render_magical_tui_frame_plain(0));
        println!("\nTip: run `coven tui` in a terminal, type a task, then press Enter.");
        return Ok(());
    }

    let mut selection = 0;
    let mut input = String::new();
    let mut raw_mode = RawModeGuard::enter()?;
    let request = loop {
        execute!(io::stdout(), Clear(ClearType::All), MoveTo(0, 0))
            .context("failed to redraw Coven menu")?;
        print!(
            "{}",
            render_magical_tui_frame_for_raw_terminal(selection, &input)
        );
        io::stdout().flush().context("failed to flush Coven menu")?;

        if let Event::Key(key) = event::read().context("failed to read Coven menu input")? {
            if !is_key_press(key.kind) {
                continue;
            }
            match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Ok(MagicalTuiRequest::Action(MagicalTuiAction::Quit));
                }
                KeyCode::Up => {
                    selection = move_magical_tui_selection(selection, MagicalTuiMove::Up);
                }
                KeyCode::Down => {
                    selection = move_magical_tui_selection(selection, MagicalTuiMove::Down);
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.clear();
                }
                KeyCode::Enter => {
                    if input.trim().is_empty() {
                        break Ok(MagicalTuiRequest::Action(
                            magical_tui_items()[selection].action,
                        ));
                    }
                    break parse_magical_tui_input(&input);
                }
                KeyCode::Char(value) => {
                    input.push(value);
                }
                KeyCode::Esc => break Ok(MagicalTuiRequest::Action(MagicalTuiAction::Quit)),
                _ => {}
            }
        }
    };
    raw_mode.restore()?;
    println!();

    run_magical_tui_request(request?)
}

fn run_magical_tui_request(request: MagicalTuiRequest) -> Result<()> {
    match request {
        MagicalTuiRequest::Action(action) => run_magical_tui_action(action),
        MagicalTuiRequest::NaturalPrompt(prompt) => run_default_prompt_session(&prompt),
        MagicalTuiRequest::HarnessPrompt { harness, prompt } => {
            run_session(&harness, &[prompt], None, None, false)
        }
        MagicalTuiRequest::AttachSession(session_id) => attach_session(&session_id),
        MagicalTuiRequest::SummonSession(session_id) => summon_session_command(&session_id),
        MagicalTuiRequest::ArchiveSession(session_id) => archive_session_command(&session_id),
        MagicalTuiRequest::SacrificeSession(session_id) => {
            sacrifice_session_command(&session_id, false)
        }
    }
}

fn run_magical_tui_action(action: MagicalTuiAction) -> Result<()> {
    match action {
        MagicalTuiAction::StartHere => run_new_user_start_here(),
        MagicalTuiAction::Help => run_tui_help(),
        MagicalTuiAction::OpenTui => run(),
        MagicalTuiAction::Doctor => run_doctor(),
        MagicalTuiAction::DaemonStatus => run_daemon_command(DaemonCommand::Status),
        MagicalTuiAction::RunHarness => run_guided_harness_session(),
        MagicalTuiAction::PatchOpenClaw => {
            run_patch_openclaw(vec![], None, None, None, false, false, true)
        }
        MagicalTuiAction::Sessions => sessions::run_browser(false),
        MagicalTuiAction::AllSessions => sessions::run_browser(true),
        MagicalTuiAction::AttachSession
        | MagicalTuiAction::SummonSession
        | MagicalTuiAction::ArchiveSession
        | MagicalTuiAction::SacrificeSession => sessions::run_browser(true),
        MagicalTuiAction::Quit => {
            let primary = theme::fg(theme::PRIMARY);
            let reset = theme::reset();
            println!("{primary}The circle fades. Nothing changed.{reset}");
            Ok(())
        }
    }
}

pub(crate) fn parse_magical_tui_input(input: &str) -> Result<MagicalTuiRequest> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(MagicalTuiRequest::Action(MagicalTuiAction::OpenTui));
    }
    if !input.starts_with('/') {
        return Ok(MagicalTuiRequest::NaturalPrompt(input.to_string()));
    }

    let (command, rest) = split_command(input);
    match command {
        "/start" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::StartHere)),
        "/help" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::Help)),
        "/tui" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::OpenTui)),
        "/doctor" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::Doctor)),
        "/daemon" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::DaemonStatus)),
        "/patch" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::PatchOpenClaw)),
        "/sessions" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::Sessions)),
        "/all" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::AllSessions)),
        "/run" => parse_run_slash_command(rest),
        "/codex" => parse_harness_slash_command("codex", rest),
        "/claude" => parse_harness_slash_command("claude", rest),
        "/attach" => parse_session_slash_command(rest, MagicalTuiRequest::AttachSession),
        "/summon" => parse_session_slash_command(rest, MagicalTuiRequest::SummonSession),
        "/archive" => parse_session_slash_command(rest, MagicalTuiRequest::ArchiveSession),
        "/sacrifice" => parse_session_slash_command(rest, MagicalTuiRequest::SacrificeSession),
        "/quit" | "/exit" => Ok(MagicalTuiRequest::Action(MagicalTuiAction::Quit)),
        _ => anyhow::bail!(
            "unknown Coven slash command `{command}`. Type `/help` to see available commands"
        ),
    }
}

fn split_command(input: &str) -> (&str, &str) {
    if let Some(index) = input.find(char::is_whitespace) {
        (&input[..index], input[index..].trim())
    } else {
        (input, "")
    }
}

fn parse_run_slash_command(rest: &str) -> Result<MagicalTuiRequest> {
    if rest.trim().is_empty() {
        return Ok(MagicalTuiRequest::Action(MagicalTuiAction::RunHarness));
    }
    let (first, remaining) = split_command(rest);
    if matches!(first, "codex" | "claude") {
        if remaining.is_empty() {
            anyhow::bail!("`/run {first}` needs a task, for example `/run {first} fix tests`");
        }
        return Ok(MagicalTuiRequest::HarnessPrompt {
            harness: first.to_string(),
            prompt: remaining.to_string(),
        });
    }
    Ok(MagicalTuiRequest::NaturalPrompt(rest.trim().to_string()))
}

fn parse_harness_slash_command(harness: &str, rest: &str) -> Result<MagicalTuiRequest> {
    let prompt = rest.trim();
    if prompt.is_empty() {
        anyhow::bail!("`/{harness}` needs a task, for example `/{harness} fix tests`");
    }
    Ok(MagicalTuiRequest::HarnessPrompt {
        harness: harness.to_string(),
        prompt: prompt.to_string(),
    })
}

fn parse_session_slash_command(
    rest: &str,
    build: fn(String) -> MagicalTuiRequest,
) -> Result<MagicalTuiRequest> {
    let session_id = rest.trim();
    if session_id.is_empty() {
        anyhow::bail!("this slash command needs a session id");
    }
    Ok(build(session_id.to_string()))
}

fn run_default_prompt_session(prompt: &str) -> Result<()> {
    let harness = default_harness_id()
        .ok_or_else(|| anyhow!("no supported harness is available; run `coven doctor` first"))?;
    run_session(harness, &[prompt.to_string()], None, None, false)
}

fn run_tui_help() -> Result<()> {
    let primary_strong = theme::fg(theme::PRIMARY_STRONG);
    let reset = theme::reset();
    println!("{primary_strong}Coven TUI{reset}");
    println!("Type a plain-language task and press Enter to launch your default harness.");
    println!("Use slash commands when you want a specific route. Examples:");
    println!("  fix the failing tests");
    println!("  /run codex explain this repo");
    println!("  /claude review the latest diff");
    println!("  /sessions");
    println!("  /attach <session-id>");
    println!("  /doctor");
    Ok(())
}

fn run_new_user_start_here() -> Result<()> {
    let primary_strong = theme::fg(theme::PRIMARY_STRONG);
    let reset = theme::reset();
    println!("{primary_strong}Coven quick start{reset}");
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
    let primary_strong = theme::fg(theme::PRIMARY_STRONG);
    let reset = theme::reset();
    println!("{primary_strong}Run an agent in this project{reset}");
    println!("Coven will create a session record, validate the project root, then attach to the harness.\n");
    let default_harness = default_harness_id().unwrap_or("codex");
    let harness_prompt = format!("Harness [default: {default_harness}; options: codex, claude]: ");
    let harness =
        prompt_for_optional_line(&harness_prompt)?.unwrap_or_else(|| default_harness.to_string());
    let prompt = prompt_for_required_line("Task for the agent: ")?;
    let title = prompt_for_optional_line("Optional session title [enter to skip]: ")?;
    run_session(&harness, &[prompt], None, title.as_deref(), false)
}

fn render_magical_tui_frame(selection: usize, input: &str) -> String {
    render_magical_tui_frame_with_mode_and_width(
        selection,
        input,
        theme::mode(),
        magical_tui_inner_width(),
    )
}

pub(crate) fn render_magical_tui_frame_for_raw_terminal(selection: usize, input: &str) -> String {
    render_magical_tui_frame(selection, input).replace('\n', "\r\n")
}

pub(crate) fn render_magical_tui_frame_plain(selection: usize) -> String {
    render_magical_tui_frame_with_mode_and_width(
        selection,
        "",
        theme::TerminalMode::NoColor,
        MAGICAL_TUI_DEFAULT_INNER_WIDTH,
    )
}

#[cfg(test)]
pub(crate) fn render_magical_tui_frame_plain_with_width(
    selection: usize,
    inner_width: usize,
) -> String {
    render_magical_tui_frame_with_mode_and_width(
        selection,
        "",
        theme::TerminalMode::NoColor,
        inner_width,
    )
}

#[cfg(test)]
pub(crate) fn render_magical_tui_frame_plain_with_input(
    selection: usize,
    input: &str,
    inner_width: usize,
) -> String {
    render_magical_tui_frame_with_mode_and_width(
        selection,
        input,
        theme::TerminalMode::NoColor,
        inner_width,
    )
}

fn render_magical_tui_frame_with_mode_and_width(
    selection: usize,
    input: &str,
    mode: theme::TerminalMode,
    inner_width: usize,
) -> String {
    let inner_width = normalized_magical_tui_inner_width(inner_width);
    let primary = theme::Fg::with_mode(theme::PRIMARY, mode);
    let primary_strong = theme::Fg::with_mode(theme::PRIMARY_STRONG, mode);
    let field_label = theme::Fg::with_mode(theme::FIELD_LABEL, mode);
    let user_label = theme::Fg::with_mode(theme::USER_LABEL, mode);
    let dim = theme::Fg::with_mode(theme::DIM, mode);
    let reset = theme::Reset::with_mode(mode);
    let mut frame = String::new();
    frame.push_str(&magical_tui_line(
        "CovenCLI",
        primary_strong,
        reset,
        inner_width,
    ));
    frame.push_str(&magical_tui_line(
        "Welcome back to the Coven.",
        field_label,
        reset,
        inner_width,
    ));
    frame.push_str(&magical_tui_line(
        "OpenCoven terminal home for local agent work.",
        user_label,
        reset,
        inner_width,
    ));
    frame.push('\n');
    for line in magical_tui_graph_lines() {
        frame.push_str(&magical_tui_line(line, primary, reset, inner_width));
    }
    frame.push('\n');
    frame.push_str(&magical_tui_line(
        "Status",
        primary_strong,
        reset,
        inner_width,
    ));
    for line in magical_tui_status_lines() {
        frame.push_str(&magical_tui_line(line, field_label, reset, inner_width));
    }
    frame.push('\n');
    frame.push_str(&magical_tui_line(
        "Task inbox",
        primary_strong,
        reset,
        inner_width,
    ));
    for line in magical_tui_task_inbox_lines() {
        frame.push_str(&magical_tui_line(line, primary, reset, inner_width));
    }
    frame.push('\n');
    for line in magical_tui_input_box_lines(input, inner_width) {
        frame.push_str(&magical_tui_line(&line, user_label, reset, inner_width));
    }
    frame.push('\n');

    frame.push_str(&magical_tui_line(
        "Slash commands",
        primary_strong,
        reset,
        inner_width,
    ));
    for (index, item) in magical_tui_items().iter().enumerate() {
        let pointer = if index == selection { ">" } else { " " };
        let content = magical_tui_command_row(pointer, item, inner_width);
        let color = if index == selection {
            primary_strong
        } else {
            primary
        };
        frame.push_str(&magical_tui_line(&content, color, reset, inner_width));
    }

    let selected = magical_tui_items()[selection.min(magical_tui_items().len() - 1)];
    frame.push('\n');
    frame.push_str(&magical_tui_line(
        "Selected command",
        primary_strong,
        reset,
        inner_width,
    ));
    frame.push_str(&magical_tui_line(
        selected.description,
        user_label,
        reset,
        inner_width,
    ));
    frame.push_str(&magical_tui_line(
        &format!("{} => {}", selected.slash, selected.command),
        primary_strong,
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

fn magical_tui_graph_lines() -> &'static [&'static str] {
    &[
        "+-------------------------- Workspace map -----------------------------+",
        "| workspace: current repo            branch: local checkout            |",
        "| harness shelf: Codex | Claude Code | local adapters                  |",
        "|                                                                      |",
        "|       [nova] ------ [coven] ------ [cody]                            |",
        r"|          |            /   \           |                              |",
        r"|          |           /     \          |                              |",
        "| [memory] -- [coven] -- [sessions] -- [review]                        |",
        r"|          |                              \                            |",
        "|     [gateway]                     local daemon                       |",
        "|                                                                      |",
        "| prompt floor: ask | slash | attach | summon | archive | sacrifice    |",
        "+----------------------------------------------------------------------+",
    ]
}

fn magical_tui_status_lines() -> &'static [&'static str] {
    &[
        "System snapshot   local-first session ledger | ~/.coven",
        "Model lane        Codex ready | Claude Code ready | PTY guarded",
        "Context           repo, docs, memory, sessions, and slash palette",
        "Approvals         asks before secrets, deletes, pushes, or public moves",
        "Release notes     CovenCLI now opens as a rich terminal home",
        "Tips              type a task, /run <harness>, or choose below",
    ]
}

fn magical_tui_task_inbox_lines() -> &'static [&'static str] {
    &[
        "[ ] inspect repo      [ ] launch harness      [ ] attach session",
        "[ ] review diff       [ ] export trace        [ ] archive work",
        "Claude Code style: welcome, status, context, prompt, command rail",
    ]
}

fn magical_tui_prompt_row(input: &str, inner_width: usize) -> String {
    let value = if input.is_empty() {
        "fix the failing tests  |  /run codex plan the refactor"
    } else {
        input
    };
    fit_chars(&format!("> {value}"), inner_width)
}

fn magical_tui_input_box_lines(input: &str, inner_width: usize) -> Vec<String> {
    let width = normalized_magical_tui_inner_width(inner_width);
    let content_width = width.saturating_sub(4).max(1);
    let prompt = magical_tui_prompt_row(input, content_width);
    let hint = fit_chars(
        "Enter sends. Empty Enter runs selected slash. Ctrl+U clears. Esc quits.",
        content_width,
    );
    vec![
        magical_tui_input_box_top(width),
        magical_tui_input_box_row(&prompt, width),
        magical_tui_input_box_row(&hint, width),
        magical_tui_input_box_bottom(width),
    ]
}

fn magical_tui_input_box_top(width: usize) -> String {
    let label = "+-- Ask anything ";
    if width <= 2 {
        return fit_chars(label, width);
    }
    if width <= label.chars().count() + 1 {
        return fit_chars(label, width);
    }
    let fill = width - label.chars().count() - 1;
    format!("{label}{}+", "-".repeat(fill))
}

fn magical_tui_input_box_bottom(width: usize) -> String {
    if width <= 2 {
        return "-".repeat(width);
    }
    format!("+{}+", "-".repeat(width - 2))
}

fn magical_tui_input_box_row(content: &str, width: usize) -> String {
    if width <= 2 {
        return fit_chars(content, width);
    }
    let content_width = width.saturating_sub(4).max(1);
    let fitted = fit_chars(content, content_width);
    let padding = content_width.saturating_sub(fitted.chars().count());
    format!("| {fitted}{} |", " ".repeat(padding))
}

fn magical_tui_line(
    content: &str,
    text_color: impl std::fmt::Display,
    reset: impl std::fmt::Display,
    inner_width: usize,
) -> String {
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

pub(crate) fn magical_tui_inner_width_for_columns(columns: usize) -> usize {
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

pub(crate) fn move_magical_tui_selection(current: usize, direction: MagicalTuiMove) -> usize {
    let item_count = magical_tui_items().len();
    match direction {
        MagicalTuiMove::Up => current.checked_sub(1).unwrap_or(item_count - 1),
        MagicalTuiMove::Down => (current + 1) % item_count,
    }
}

#[cfg(test)]
pub(crate) fn render_frame_plain_for_test(selection: usize) -> String {
    render_magical_tui_frame_plain(selection)
}
