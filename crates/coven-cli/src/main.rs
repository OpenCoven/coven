use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
#[cfg(unix)]
use std::io::Read;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::{SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use uuid::Uuid;

mod api;
mod capabilities;
mod cockpit_sources;
mod control_plane;
mod coven_calls;
mod daemon;
mod encrypted_artifacts;
mod eval_loop;
mod familiar_identity;
mod harness;
mod openclaw_repo;
mod parallel_protocol;
mod patch;
mod pc;
mod privacy;
mod project;
mod prompt_refs;
mod pty_runner;
mod repos_config;
mod settings;
mod store;
mod stream_json;
mod theme;
mod tui;
mod verification;

pub(crate) const DEFAULT_COVEN_HOME_DIR: &str = ".coven";
pub(crate) const STORE_FILE_NAME: &str = "coven.sqlite3";
const DEFAULT_SESSION_STATUS: &str = "created";
const RUNNING_SESSION_STATUS: &str = "running";
const FAILED_SESSION_STATUS: &str = "failed";
const DEFAULT_TITLE_CHARS: usize = 48;

#[derive(Parser, Debug)]
#[command(name = "coven")]
#[command(version = env!("COVEN_VERSION_DESC"))]
#[command(about = "Run project-scoped coding agents without memorizing harness commands")]
#[command(
    long_about = "Coven runs Codex, Claude Code, and future harnesses inside a local, project-scoped session ledger. Run `coven` with no arguments to open the interactive Coven UI (requires the coven-code front-end), or pass a free-text task to plan and run it directly."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(
        value_name = "PROMPT",
        num_args = 0..,
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "Free-text task to cast when no subcommand is given: Coven plans it, asks you to confirm, then runs it in a session"
    )]
    prompt: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[command(about = "Open the interactive Coven UI (requires coven-code)")]
    Chat,
    #[command(about = "Open the interactive Coven UI (same as `coven chat`)")]
    Tui,
    #[command(about = "Check local setup and print next steps")]
    Doctor,
    #[command(
        name = "adapter",
        alias = "adapters",
        about = "List and diagnose harness adapters"
    )]
    Adapter {
        #[command(subcommand)]
        command: AdapterCommand,
    },
    #[command(about = "Manage the local Coven daemon")]
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    #[command(about = "Launch a project-scoped harness session")]
    Run {
        #[arg(help = "Harness to run: codex or claude")]
        harness: String,
        #[arg(help = "Task for the harness", required = false, num_args = 0..)]
        prompt: Vec<String>,
        #[arg(long, help = "Working directory inside the current project")]
        cwd: Option<PathBuf>,
        #[arg(long, help = "Readable title for `coven sessions`")]
        title: Option<String>,
        #[arg(
            long,
            conflicts_with = "continue_session",
            help = "Create the session record without launching the harness"
        )]
        detach: bool,
        #[arg(
            long = "continue",
            value_name = "ID",
            num_args = 0..=1,
            default_missing_value = "",
            help = "Resume session by id; omit value to resume the latest active session for this project"
        )]
        continue_session: Option<String>,
        #[arg(
            long,
            value_delimiter = ',',
            help = "Comma-separated labels for the new session (ignored when resuming)"
        )]
        labels: Vec<String>,
        #[arg(
            long,
            value_parser = ["private", "workspace", "shared"],
            help = "Visibility for the new session: private (default), workspace, shared (ignored when resuming)"
        )]
        visibility: Option<String>,
        #[arg(long, help = "Archive the session after the run completes")]
        archive: bool,
        #[arg(
            long,
            value_name = "ID",
            help = "Familiar id to inject as identity context (e.g. charm). The identity preamble is injected via each harness's preferred mechanism (--system-prompt flag or prompt prefix)."
        )]
        familiar: Option<String>,
        #[arg(
            long,
            value_name = "ID",
            help = "Model to run the harness on. Accepts a namespaced id (e.g. openai/gpt-5.5, anthropic/claude-...); Coven strips the provider/ prefix and forwards the bare id to the harness's native model flag (codex/claude --model). Adapters that declare no model mechanism warn and continue. Echoed back in the stream-json system.init `model` field."
        )]
        model: Option<String>,
        #[arg(
            long,
            help = "Request deeper reasoning when the harness supports it. Unsupported harnesses warn and continue."
        )]
        think: bool,
        #[arg(
            long,
            value_name = "LEVEL",
            value_parser = ["fast", "balanced", "thorough"],
            help = "Latency/reasoning hint: fast, balanced, or thorough. Unsupported harnesses warn and continue."
        )]
        speed: Option<String>,
        #[arg(
            long,
            value_name = "MODE",
            value_parser = ["full", "read-only"],
            help = "Sandbox/permission policy for the harness: full (default) or read-only. Maps to each harness's native flag (codex --sandbox, claude --permission-mode). Harnesses with no sandbox mechanism warn and continue."
        )]
        permission: Option<String>,
        #[arg(
            long,
            help = "Emit JSONL events on stdout (system.init / user / assistant / tool_result / result)"
        )]
        stream_json: bool,
        #[arg(
            long,
            requires = "stream_json",
            help = "Read JSONL user messages from stdin (claude harness only; requires --stream-json)"
        )]
        stream_json_input: bool,
    },
    #[command(about = "List or search recent Coven sessions", alias = "session")]
    Sessions {
        #[command(subcommand)]
        command: Option<SessionsCommand>,
        #[arg(long, help = "Include archived sessions (list mode only)")]
        all: bool,
        #[arg(long, conflicts_with_all = ["plain", "json"], help = "Open the interactive session action browser")]
        manage: bool,
        #[arg(long, conflicts_with_all = ["manage", "json"], help = "Print a plain table instead of the session browser")]
        plain: bool,
        #[arg(long, conflicts_with_all = ["manage", "plain"], help = "Print sessions as JSON (machine-readable)")]
        json: bool,
    },
    #[command(about = "Manage local log retention")]
    Logs {
        #[command(subcommand)]
        command: LogsCommand,
    },
    #[command(
        about = "Create, list, diagnose, and prune Coven worktrees",
        alias = "worktree",
        alias = "worktrees"
    )]
    Wt {
        #[arg(
            help = "Branch to create or enter in the sibling <repo>.wt directory",
            conflicts_with_all = ["list", "doctor", "prune_merged", "prune_stale"],
            required_unless_present_any = ["list", "doctor", "prune_merged", "prune_stale"]
        )]
        branch: Option<String>,
        #[arg(long, conflicts_with_all = ["doctor", "prune_merged", "prune_stale"], help = "List worktrees with claim and dirty state")]
        list: bool,
        #[arg(long, conflicts_with_all = ["list", "prune_merged", "prune_stale"], help = "Report protocol layout and hook issues")]
        doctor: bool,
        #[arg(long, conflicts_with_all = ["list", "doctor", "prune_stale"], help = "Remove clean worktrees whose branches are merged into the primary branch")]
        prune_merged: bool,
        #[arg(long, value_name = "DAYS", conflicts_with_all = ["list", "doctor", "prune_merged"], help = "Remove clean worktrees not modified for DAYS")]
        prune_stale: Option<u64>,
    },
    #[command(about = "Manage TTL-bounded agent branch claims")]
    Claim {
        #[command(subcommand)]
        command: ClaimCommand,
    },
    #[command(about = "Install Coven Parallel Work Protocol git hooks")]
    Hooks {
        #[command(subcommand)]
        command: HooksCommand,
    },
    #[command(about = "Replay/follow a session and forward input to live daemon sessions")]
    Attach {
        #[arg(help = "Session id, or a unique prefix of one (list ids with `coven sessions`)")]
        session_id: String,
    },
    #[command(about = "Summon an archived session back, then replay/follow it")]
    Summon {
        #[arg(
            help = "Session id, or a unique prefix of one (list ids with `coven sessions --all`)"
        )]
        session_id: String,
    },
    #[command(about = "Archive a completed session without deleting its events")]
    Archive {
        #[arg(help = "Session id, or a unique prefix of one (list ids with `coven sessions`)")]
        session_id: String,
    },
    #[command(about = "Permanently delete a non-running session and its events")]
    Sacrifice {
        #[arg(
            help = "Session id, or a unique prefix of one (list ids with `coven sessions --all`)"
        )]
        session_id: String,
        #[arg(long, help = "Confirm permanent deletion")]
        yes: bool,
    },
    #[command(about = "Guided repair flow for a registered repo")]
    Patch {
        #[arg(help = "Registered repo name (default: from ~/.coven/repos.toml, else `openclaw`)")]
        name: Option<String>,
        #[arg(num_args = 0.., help = "Issue text describing what is broken")]
        issue: Vec<String>,
        #[arg(long, help = "Override the repo path for this run")]
        repo: Option<PathBuf>,
        #[arg(long, help = "Harness to use: codex or claude")]
        harness: Option<String>,
        #[arg(
            long,
            value_name = "PROFILE",
            value_parser = ["auto", "pnpm-check", "targeted-test", "diff-only"],
            help = "Verification profile to run after the harness finishes"
        )]
        verify: Option<String>,
        #[arg(
            long,
            help = "Never prompt; requires issue text and --harness, and fails instead of asking"
        )]
        non_interactive: bool,
        #[arg(
            long,
            help = "Print the plan and repair brief without launching the harness"
        )]
        dry_run: bool,
        // Accepted for compatibility; patch sessions are always kept today.
        #[arg(long, hide = true)]
        keep_session: bool,
    },
    #[command(about = "Diagnose and relieve macOS system pressure")]
    Pc {
        #[command(subcommand)]
        command: Option<pc::PcCommand>,
    },
}

#[derive(Subcommand, Debug)]
enum SessionsCommand {
    #[command(about = "Full-text search session event payloads")]
    Search {
        #[arg(help = "Full-text query (FTS5 syntax, e.g. `phoenix OR rises`)")]
        query: String,
        #[arg(long, help = "Print search hits as JSON (machine-readable)")]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum AdapterCommand {
    #[command(about = "List configured harness adapters")]
    List {
        #[arg(long, help = "Print adapter reports as JSON")]
        json: bool,
    },
    #[command(about = "Diagnose all adapters, or one adapter id")]
    Doctor {
        #[arg(help = "Adapter id to diagnose")]
        adapter: Option<String>,
    },
    #[command(about = "Install a trusted local adapter recipe")]
    Install {
        #[arg(help = "Adapter recipe to install, e.g. hermes")]
        adapter: String,
    },
}

#[derive(Subcommand, Debug)]
enum DaemonCommand {
    #[command(about = "Start the background daemon that hosts live sessions")]
    Start,
    #[command(about = "Restart the background daemon")]
    Restart,
    #[command(about = "Show whether the daemon is running")]
    Status,
    #[command(about = "Stop the background daemon")]
    Stop,
    #[command(hide = true)]
    Serve {
        #[arg(
            long,
            value_name = "ADDR",
            help = "Also bind an HTTP TCP listener at ADDR (e.g. 127.0.0.1:3000). \
                    The API is unauthenticated — bind only to loopback for local \
                    dev (e.g. cockpit via Vite proxy). Do not expose to non-loopback \
                    interfaces or untrusted networks."
        )]
        tcp: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum LogsCommand {
    #[command(about = "Prune expired raw artifacts and old redacted event logs")]
    Prune {
        #[arg(long, help = "Report what would be pruned without deleting rows")]
        dry_run: bool,
        #[arg(
            long,
            value_name = "DAYS",
            help = "Override raw artifact retention days"
        )]
        raw_days: Option<u64>,
        #[arg(
            long,
            value_name = "DAYS",
            help = "Override redacted event retention days"
        )]
        event_days: Option<u64>,
    },
}

#[derive(Subcommand, Debug)]
enum ClaimCommand {
    #[command(about = "Claim a branch for the current agent")]
    Acquire {
        #[arg(help = "Branch to claim")]
        branch: String,
    },
    #[command(about = "Release this agent's claim for a branch")]
    Release {
        #[arg(help = "Branch to release")]
        branch: String,
    },
    #[command(about = "Extend this agent's claim TTL for a branch")]
    Heartbeat {
        #[arg(help = "Branch whose claim should be extended")]
        branch: String,
    },
    #[command(about = "Record the current HEAD for later hook canary checks")]
    Canary {
        #[arg(help = "Branch to associate with the current HEAD snapshot")]
        branch: String,
    },
    #[command(about = "Show active and expired claims for this repository")]
    Status,
}

#[derive(Subcommand, Debug)]
enum HooksCommand {
    #[command(about = "Install pre-commit and pre-push protocol hooks")]
    Install,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InteractiveShellRoute {
    Chat,
    PlainCast,
}

fn main() -> Result<()> {
    let loaded =
        settings::user_settings_path().as_deref().and_then(|path| {
            match settings::load_from(path) {
                Ok(s) => s,
                Err(err) => {
                    eprintln!("coven: ignoring settings ({}): {err:#}", path.display());
                    None
                }
            }
        });
    settings::init_cached(loaded);

    let cli = Cli::parse();
    run_cli(cli)
}

fn run_cli(cli: Cli) -> Result<()> {
    if cli.command.is_none() && !cli.prompt.is_empty() {
        return run_bare_prompt(&cli.prompt);
    }

    match cli.command {
        None | Some(Command::Chat) | Some(Command::Tui) => run_shared_interactive_shell(),
        Some(Command::Doctor) => run_doctor(),
        Some(Command::Adapter { command }) => run_adapter_command(command),
        Some(Command::Daemon { command }) => run_daemon_command(command),
        Some(Command::Run {
            harness,
            prompt,
            cwd,
            title,
            detach,
            continue_session,
            labels,
            visibility,
            archive,
            familiar,
            model,
            think,
            speed,
            permission,
            stream_json,
            stream_json_input,
        }) => run_session(
            &harness,
            &prompt,
            cwd.as_deref(),
            title.as_deref(),
            detach,
            continue_session.as_deref(),
            labels,
            visibility.as_deref(),
            archive,
            familiar.as_deref(),
            model.as_deref(),
            think,
            speed.as_deref(),
            permission.as_deref(),
            stream_json,
            stream_json_input,
        ),
        Some(Command::Sessions {
            command,
            all,
            manage,
            plain,
            json,
        }) => match command {
            Some(SessionsCommand::Search {
                query,
                json: search_json,
            }) => run_sessions_search(&query, search_json),
            None => tui::sessions::run_command(all, manage, plain, json),
        },
        Some(Command::Logs { command }) => run_logs_command(command),
        Some(Command::Wt {
            branch,
            list,
            doctor,
            prune_merged,
            prune_stale,
        }) => parallel_protocol::run_wt_command(
            branch.as_deref(),
            list,
            doctor,
            prune_merged,
            prune_stale,
        ),
        Some(Command::Claim { command }) => match command {
            ClaimCommand::Acquire { branch } => parallel_protocol::claim_acquire(&branch),
            ClaimCommand::Release { branch } => parallel_protocol::claim_release(&branch),
            ClaimCommand::Heartbeat { branch } => parallel_protocol::claim_heartbeat(&branch),
            ClaimCommand::Canary { branch } => parallel_protocol::claim_canary(&branch),
            ClaimCommand::Status => parallel_protocol::claim_status(),
        },
        Some(Command::Hooks { command }) => match command {
            HooksCommand::Install => parallel_protocol::hooks_install(),
        },
        Some(Command::Attach { session_id }) => attach_session(&session_id),
        Some(Command::Summon { session_id }) => summon_session_command(&session_id),
        Some(Command::Archive { session_id }) => archive_session_command(&session_id),
        Some(Command::Sacrifice { session_id, yes }) => sacrifice_session_command(&session_id, yes),
        Some(Command::Patch {
            name,
            issue,
            repo,
            harness,
            verify,
            non_interactive,
            dry_run,
            keep_session,
        }) => run_patch(
            name,
            issue,
            repo,
            harness,
            verify,
            non_interactive,
            dry_run,
            keep_session,
        ),
        Some(Command::Pc { command }) => pc::run_pc_command(command),
    }
}

fn run_bare_prompt(prompt: &[String]) -> Result<()> {
    // The bare-prompt catch-all swallows anything clap doesn't recognize, so
    // it has to do its own front-door validation: reject stray flags, catch
    // near-miss subcommand typos, and refuse to launch a harness when nobody
    // is at the terminal to confirm or cancel the cast.
    if let Some(first) = prompt.first() {
        if first.starts_with('-') {
            anyhow::bail!(
                "unrecognized flag `{first}`; run `coven --help` to see available flags and commands"
            );
        }
    }
    if let [token] = prompt {
        if let Some(suggestion) = near_miss_subcommand(token) {
            anyhow::bail!(
                "unrecognized subcommand `{token}`; did you mean `coven {suggestion}`? \
                 (to run `{token}` as a task instead, use `coven run <harness> \"{token}\"`)"
            );
        }
    }
    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "refusing to cast without an interactive terminal to confirm the plan; \
             use `coven run <harness> \"<task>\"` for scripted runs"
        );
    }
    let prompt = joined_prompt(prompt)?;
    tui::shell::run_cast_spell(&prompt)
}

/// Suggest a subcommand (or alias) within a small edit distance of the given
/// word. Guards the bare-prompt catch-all: without this, `coven sesions`
/// would be cast as an AI task instead of surfacing a typo.
fn near_miss_subcommand(word: &str) -> Option<String> {
    use clap::CommandFactory;
    let needle = word.to_ascii_lowercase();
    let mut best: Option<(usize, String)> = None;
    for subcommand in Cli::command().get_subcommands() {
        let names = std::iter::once(subcommand.get_name().to_string())
            .chain(subcommand.get_all_aliases().map(str::to_string));
        for name in names {
            let threshold = if name.len() <= 4 { 1 } else { 2 };
            let distance = edit_distance(&needle, &name);
            if distance > 0
                && distance <= threshold
                && best.as_ref().is_none_or(|(d, _)| distance < *d)
            {
                best = Some((distance, name));
            }
        }
    }
    best.map(|(_, name)| name)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, &ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, &cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current.push(substitution.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

fn run_sessions_search(query: &str, json: bool) -> Result<()> {
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let hits = store::search_events(&conn, query)?;

    if json {
        // Serialize the Vec<SearchHit> directly — SearchHit derives Serialize.
        let serialized = serde_json::to_string(&hits).context("failed to serialize search hits")?;
        println!("{serialized}");
        return Ok(());
    }

    if hits.is_empty() {
        println!("No matches for `{query}`.");
        return Ok(());
    }

    for hit in &hits {
        println!(
            "{}  {}  [{}]  {}",
            hit.created_at, hit.session_id, hit.kind, hit.snippet
        );
    }
    Ok(())
}

fn run_shared_interactive_shell() -> Result<()> {
    // coven-code is the canonical interactive front-end. The only escape
    // hatch is an explicit `COVEN_LEGACY_TUI=1`, which keeps the legacy
    // in-process tui::shell available during the transition.
    if legacy_tui_opted_in() {
        eprintln!("{}", legacy_tui_warning_message(target_shell()));
        return match interactive_shell_route(
            None,
            io::stdin().is_terminal(),
            io::stdout().is_terminal(),
        ) {
            InteractiveShellRoute::Chat => tui::chat::run_chat(),
            InteractiveShellRoute::PlainCast => tui::shell::run(),
        };
    }

    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        anyhow::bail!(
            "the interactive Coven UI needs a real terminal (stdin/stdout are not TTYs).\n\
             Try instead: coven doctor · coven sessions --plain · coven run <harness> \"<task>\""
        );
    }

    match coven_code_binary() {
        Some(binary) => try_delegate_to_coven_code(&binary),
        None => Err(missing_coven_code_error()),
    }
}

/// Build a single, user-actionable error for the missing-coven-code case.
fn missing_coven_code_error() -> anyhow::Error {
    anyhow!(missing_coven_code_error_message(target_shell()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetShell {
    Posix,
    PowerShell,
}

fn target_shell() -> TargetShell {
    if cfg!(windows) {
        TargetShell::PowerShell
    } else {
        TargetShell::Posix
    }
}

fn missing_coven_code_error_message(shell: TargetShell) -> String {
    format!(
        "coven-code is required for the interactive Coven UI but was not found on PATH \
         or under ~/.coven-code/bin.\n\n\
         Install it with:\n\
         {install}\n\n\
         If you need the legacy slash shell temporarily, run:\n\
         {legacy}\n\
         (the legacy shell will be removed in a future release.)",
        install = coven_code_install_instructions(shell),
        legacy = legacy_tui_instructions(shell),
    )
}

fn legacy_tui_warning_message(shell: TargetShell) -> String {
    format!(
        "coven: warning — COVEN_LEGACY_TUI is set; falling back to the legacy slash shell.\n\
         coven: the legacy shell is deprecated and will be removed in a future release.\n\
         coven: install coven-code to use the supported interactive UI:\n\
         {install}",
        install = coven_code_install_instructions(shell)
            .lines()
            .map(|line| format!("coven: {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn coven_code_install_instructions(shell: TargetShell) -> &'static str {
    match shell {
        TargetShell::Posix => {
            "  npm install -g @opencoven/coven-code\n  curl -fsSL https://github.com/OpenCoven/coven-code/releases/latest/download/install.sh | bash"
        }
        TargetShell::PowerShell => {
            "  npm install -g @opencoven/coven-code\n  irm https://github.com/OpenCoven/coven-code/releases/latest/download/install.ps1 | iex"
        }
    }
}

fn legacy_tui_instructions(shell: TargetShell) -> &'static str {
    match shell {
        TargetShell::Posix => "  COVEN_LEGACY_TUI=1 coven",
        TargetShell::PowerShell => {
            "  $env:COVEN_LEGACY_TUI = \"1\"\n  coven\n  Remove-Item Env:COVEN_LEGACY_TUI"
        }
    }
}

/// `COVEN_LEGACY_TUI=1` (or `=true`) opts back into the in-process tui::shell.
/// This is a transitional escape hatch, not the supported path.
fn legacy_tui_opted_in() -> bool {
    matches!(
        std::env::var("COVEN_LEGACY_TUI").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// Locate the `coven-code` binary on PATH or in `~/.coven-code/bin/`.
fn coven_code_binary() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH");
    let home = dirs_next::home_dir();
    coven_code_binary_from(path_var.as_deref(), home.as_deref())
}

fn coven_code_binary_from(path_var: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["coven-code.exe", "coven-code.cmd", "coven-code.bat"]
    } else {
        &["coven-code"]
    };
    coven_code_binary_from_names(path_var, home, names)
}

fn coven_code_binary_from_names(
    path_var: Option<&OsStr>,
    home: Option<&Path>,
    names: &[&str],
) -> Option<PathBuf> {
    if let Some(path_var) = path_var {
        for dir in std::env::split_paths(path_var) {
            for name in names {
                let candidate = dir.join(name);
                if is_executable_file(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }
    if let Some(home) = home {
        let bin_dir = home.join(".coven-code").join("bin");
        for name in names {
            let candidate = bin_dir.join(name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Exec `coven-code` in place of the current process. Returns only on failure;
/// on success the child takes over this PID and stdio.
fn try_delegate_to_coven_code(binary: &Path) -> Result<()> {
    let mut command = std::process::Command::new(binary);
    // Pass through every flag the user supplied to `coven`/`coven tui` so
    // any future coven-code substrate flags work end-to-end. We strip argv[0]
    // and the optional leading `tui` subcommand because coven-code expects
    // its own argv layout.
    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if matches!(
        args.first().and_then(|a| a.to_str()),
        Some("tui") | Some("chat")
    ) {
        args.remove(0);
    }
    command.args(args);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = command.exec();
        // exec only returns on failure.
        Err(anyhow!("failed to exec coven-code: {err}"))
    }

    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .map_err(|e| anyhow!("failed to launch coven-code: {e}"))?;
        // A signal-terminated child has no code; treat it as failure.
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn interactive_shell_route(
    _command: Option<&Command>,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> InteractiveShellRoute {
    if stdin_is_terminal && stdout_is_terminal {
        InteractiveShellRoute::Chat
    } else {
        InteractiveShellRoute::PlainCast
    }
}

fn run_doctor() -> Result<()> {
    let home = coven_home_dir()?;
    println!("Coven doctor");
    println!("Store: {}", home.display());
    match std::env::current_dir()
        .ok()
        .and_then(|cwd| project::canonical_project_root(&cwd).ok())
    {
        Some(root) => println!("Project: {}", root.display()),
        None => println!("Project: not inside a git/project root yet"),
    }

    println!("\nDaemon:");
    match daemon::background_server_status(&home)? {
        Some(daemon::DaemonStatusState::Running(status)) => {
            let health = api::health_response(Some(status.clone()));
            println!(
                "  status=running ok={} pid={} socket={}",
                health.ok, status.pid, status.socket
            );
        }
        Some(daemon::DaemonStatusState::Stale(status)) => {
            println!(
                "  status=stale ok=false pid={} socket={}",
                status.pid, status.socket
            );
        }
        None => println!("  status=stopped"),
    }

    let repos_config = repos_config::load_with_settings(&home, settings::cached())?;
    if !repos_config.is_empty() {
        println!("\nRepos ({}):", repos_config::config_path(&home).display());
        for (name, path) in repos_config.entries() {
            let ok = path.is_dir() && path.join(".git").exists();
            let marker = if ok { "OK" } else { "!!" };
            println!("  [{marker}] {name:<16} {}", path.display());
        }
    }

    println!("\nHarnesses:");
    let harnesses = harness::configured_harnesses()?;
    for harness in &harnesses {
        let status = if harness.available {
            "ready"
        } else {
            "missing"
        };
        let marker = if harness.available { "OK" } else { "!!" };
        println!(
            "  [{marker}] {:<18} `{}` is {status} ({})",
            harness.label,
            harness.executable,
            adapter_source_label(&harness.source)
        );
        if !harness.available {
            println!("       {}", harness.install_hint);
        }
    }

    println!("\nInteractive UI:");
    match coven_code_binary() {
        Some(path) => println!("  [OK] coven-code found at {}", path.display()),
        None => {
            println!("  [!!] coven-code is missing — `coven` and `coven chat` need it");
            for line in coven_code_install_instructions(target_shell()).lines() {
                println!("     {line}");
            }
        }
    }

    print_familiars_section(&home);

    println!("\nNext steps:");
    if let Some(default) = default_harness_id() {
        println!("  coven run {default} \"explain this repo in 5 bullets\"");
        println!("  coven sessions");
    } else {
        println!("  Install and authenticate at least one harness in this same shell.");
        println!("  Codex: npm install -g @openai/codex && codex login");
        println!("  Claude Code: npm install -g @anthropic-ai/claude-code && claude doctor");
        println!("  If PATH changed, open a new terminal and run `coven doctor` again.");
        println!("  Then run: coven daemon start");
        println!(
            "  Install docs: https://github.com/OpenCoven/coven/blob/main/docs/install/index.md"
        );
    }
    Ok(())
}

/// Human label for an adapter spec's `source` field. The raw value ("bundled")
/// reads as a contradiction next to a missing executable ("missing (bundled)").
fn adapter_source_label(source: &str) -> &str {
    if source == "bundled" {
        "built-in adapter"
    } else {
        source
    }
}

/// Surface the configured familiars so operators can confirm which identities
/// `coven run --familiar <id>` will resolve, and how fresh each one's memory is.
/// Identity is the product's spine, so doctor should make it as visible as the
/// daemon and harness state — without claiming anything the manifest doesn't say.
fn print_familiars_section(home: &Path) {
    let manifest = home.join("familiars.toml");
    let familiars = match cockpit_sources::read_familiars(home) {
        Ok(familiars) => familiars,
        Err(err) => {
            println!("\nFamiliars:");
            println!("  !! could not read {}: {err:#}", manifest.display());
            return;
        }
    };

    if familiars.is_empty() {
        println!("\nFamiliars:");
        println!("  none configured ({})", manifest.display());
        println!(
            "  Declare [[familiar]] entries there, then run with \
             `coven run <harness> --familiar <id> \"...\"`."
        );
        return;
    }

    println!("\nFamiliars ({}):", manifest.display());
    let id_width = familiars
        .iter()
        .map(|familiar| familiar.id.len())
        .max()
        .unwrap_or(0);
    for familiar in &familiars {
        let role = if familiar.role.is_empty() {
            String::new()
        } else {
            format!(" — {}", familiar.role)
        };
        println!(
            "  {:<id_width$} {}{}  (memory: {})",
            familiar.id, familiar.display_name, role, familiar.memory_freshness
        );
    }
}

fn run_adapter_command(command: AdapterCommand) -> Result<()> {
    match command {
        AdapterCommand::List { json } => run_adapter_list(json),
        AdapterCommand::Doctor { adapter } => run_adapter_doctor(adapter.as_deref()),
        AdapterCommand::Install { adapter } => run_adapter_install(&adapter),
    }
}

fn run_adapter_list(json: bool) -> Result<()> {
    let harnesses = harness::configured_harnesses()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&harnesses)?);
        return Ok(());
    }

    println!("Coven adapters");
    for harness in harnesses {
        let availability = if harness.available {
            "ready"
        } else {
            "missing"
        };
        let manifest = harness
            .manifest_path
            .as_deref()
            .map(|path| format!(" from {path}"))
            .unwrap_or_default();
        println!(
            "  {:<18} {:<10} `{}` {}{}",
            harness.id,
            availability,
            harness.executable,
            adapter_source_label(&harness.source),
            manifest
        );
    }
    Ok(())
}

fn run_adapter_doctor(adapter: Option<&str>) -> Result<()> {
    let harnesses = harness::configured_harnesses()?;
    let filtered: Vec<_> = match adapter {
        Some(id) => harnesses
            .into_iter()
            .filter(|harness| harness.id == id)
            .collect(),
        None => harnesses,
    };

    if let Some(id) = adapter {
        if filtered.is_empty() {
            anyhow::bail!(
                "unknown adapter `{id}`; run `coven adapter list` to see configured adapters"
            );
        }
    }

    println!("Coven adapter doctor");
    for harness in filtered {
        let marker = if harness.available { "OK" } else { "!!" };
        let status = if harness.available {
            "ready"
        } else {
            "missing"
        };
        println!(
            "  [{marker}] {:<18} `{}` is {status}",
            harness.label, harness.executable
        );
        if let Some(path) = harness.manifest_path.as_deref() {
            println!("       manifest: {path}");
        }
        if !harness.available {
            println!("       {}", harness.install_hint);
        }
    }
    Ok(())
}

fn run_adapter_install(adapter: &str) -> Result<()> {
    let manifest = harness::known_adapter_manifest(adapter).ok_or_else(|| {
        anyhow!(
            "unknown adapter recipe `{adapter}`. Known recipes: {}",
            harness::known_adapter_recipe_names().join(", ")
        )
    })?;
    let coven_home = coven_home_dir()?;
    let adapter_dir = harness::trusted_adapter_dir(&coven_home);
    let manifest_path = harness::trusted_adapter_manifest_path(&coven_home, adapter);

    std::fs::create_dir_all(&adapter_dir).with_context(|| {
        format!(
            "failed to create trusted adapter directory {}",
            adapter_dir.display()
        )
    })?;
    let installed = harness::trusted_adapter_manifest_matches_recipe(&manifest_path, adapter);
    if installed {
        println!(
            "Adapter `{adapter}` is already installed at {}",
            manifest_path.display()
        );
    } else {
        if let Ok(metadata) = manifest_path.symlink_metadata() {
            let remove_result =
                if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                    std::fs::remove_dir_all(&manifest_path)
                } else {
                    std::fs::remove_file(&manifest_path)
                };
            remove_result.with_context(|| {
                format!(
                    "failed to replace trusted adapter manifest {}",
                    manifest_path.display()
                )
            })?;
        }
        std::fs::write(&manifest_path, manifest).with_context(|| {
            format!(
                "failed to write trusted adapter manifest {}",
                manifest_path.display()
            )
        })?;
        println!(
            "Installed adapter `{adapter}` at {}",
            manifest_path.display()
        );
    }

    println!("Next steps:");
    println!("  coven adapter doctor {adapter}");
    println!("  coven run {adapter} \"what is in this project?\"");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_patch(
    name: Option<String>,
    issue: Vec<String>,
    repo: Option<PathBuf>,
    harness: Option<String>,
    verify: Option<String>,
    non_interactive: bool,
    dry_run: bool,
    keep_session: bool,
) -> Result<()> {
    let coven_home = coven_home_dir()?;
    let repos_config = repos_config::load_with_settings(&coven_home, settings::cached())?;
    let resolved_name = name
        .or_else(|| repos_config.default_name().map(str::to_string))
        .unwrap_or_else(|| openclaw_repo::OPENCLAW_REPO_NAME.to_string());

    let start_dir = std::env::current_dir().context("failed to read current directory")?;
    let mapped_repo = repos_config.resolve(&resolved_name);
    let stored_repo = stored_repository_path(&resolved_name)?;
    let detected_repo = openclaw_repo::detect_repo(
        &resolved_name,
        repo.as_deref(),
        mapped_repo.as_deref(),
        &start_dir,
        stored_repo.as_deref(),
    )?;
    let git_state = openclaw_repo::inspect_git_state(&detected_repo.root)?;
    let issue = match joined_optional_issue(issue)? {
        Some(issue) => issue,
        None if non_interactive => anyhow::bail!("issue text is required with --non-interactive"),
        None => {
            prompt_for_required_line(&format!("What is broken in {}? ", detected_repo.repo_name))?
        }
    };
    let harness_id = match harness {
        Some(harness) => patch::HarnessId::parse(&harness)?,
        None if non_interactive => anyhow::bail!("--harness is required with --non-interactive"),
        None => choose_default_harness()?,
    };
    let verification_profile = patch::VerificationProfile::parse(verify.as_deref())?;

    let request = patch::PatchRequest {
        repo: detected_repo,
        git_state,
        issue,
        harness_id,
        verification_profile,
        non_interactive,
        dry_run,
        keep_session,
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
    remember_repo_location(&request.repo)?;
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
        patch::summarize_patch_report(&patch::PatchReport {
            status: status.to_string(),
            session_id,
            changed_files,
            verification: verification_lines,
        })
    );
    Ok(())
}

fn stored_repository_path(repository_id: &str) -> Result<Option<PathBuf>> {
    let Some(store_path) = coven_store_path_if_exists()? else {
        return Ok(None);
    };
    let Some(conn) = store::open_existing_store_read_only(&store_path)? else {
        return Ok(None);
    };
    if !store::repositories_table_exists(&conn)? {
        return Ok(None);
    }
    Ok(store::get_repository(&conn, repository_id)?.map(|repo| PathBuf::from(repo.path)))
}

fn remember_repo_location(repo: &openclaw_repo::RepoHandle) -> Result<()> {
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let now = current_timestamp();
    let existing = store::get_repository(&conn, &repo.repo_name)?;
    store::upsert_repository(
        &conn,
        &store::RepositoryRecord {
            id: repo.repo_name.clone(),
            path: repo.root.to_string_lossy().into_owned(),
            package_name: repo.package_name.clone(),
            created_at: existing
                .map(|repo| repo.created_at)
                .unwrap_or_else(|| now.clone()),
            updated_at: now,
        },
    )
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

fn default_harness_id() -> Option<String> {
    let harnesses = harness::built_in_harnesses();
    harnesses
        .iter()
        .find(|h| h.id == "codex" && h.available)
        .or_else(|| harnesses.iter().find(|h| h.id == "claude" && h.available))
        .map(|h| h.id.clone())
}

fn launch_patch_session(request: &patch::PatchRequest) -> Result<String> {
    let selected_harness = selected_available_harness(request.harness_id.as_str())?;
    let coven_home = coven_home_dir()?;
    let store_path = coven_home.join(STORE_FILE_NAME);
    let conn = store::open_store(&store_path)?;
    let now = current_timestamp();
    let brief = patch::build_repair_brief(request);
    let record = store::SessionRecord {
        id: Uuid::new_v4().to_string(),
        project_root: request.repo.root.to_string_lossy().into_owned(),
        harness: selected_harness.id.to_string(),
        title: session_title(Some(&format!("Patch {}", request.repo.repo_name)), &brief),
        status: DEFAULT_SESSION_STATUS.to_string(),
        exit_code: None,
        archived_at: None,
        created_at: now.clone(),
        updated_at: now.clone(),
        conversation_id: None,
        familiar_id: None,
        labels: Vec::new(),
        visibility: "private".to_string(),
    };
    store::insert_session(&conn, &record)?;
    let metadata = serde_json::json!({
        "patchTarget": request.repo.repo_name,
        "repoRoot": request.repo.root,
        "issue": request.issue,
        "harnessId": request.harness_id.as_str(),
        "verificationProfile": request.verification_profile.as_str(),
        "status": "running"
    });
    store::insert_event_with_privacy(
        &conn,
        &coven_home,
        &store::EventRecord {
            seq: 0,
            id: Uuid::new_v4().to_string(),
            session_id: record.id.clone(),
            kind: "patch_metadata".to_string(),
            payload_json: metadata.to_string(),
            created_at: now,
        },
    )?;

    store::update_session_status(
        &conn,
        &record.id,
        RUNNING_SESSION_STATUS,
        None,
        &current_timestamp(),
    )?;
    let command = pty_runner::build_harness_command(
        &selected_harness.id,
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

fn run_logs_command(command: LogsCommand) -> Result<()> {
    match command {
        LogsCommand::Prune {
            dry_run,
            raw_days,
            event_days,
        } => prune_logs_command(dry_run, raw_days, event_days),
    }
}

fn prune_logs_command(dry_run: bool, raw_days: Option<u64>, event_days: Option<u64>) -> Result<()> {
    let home = coven_home_dir()?;
    let config = privacy::load_with_settings(&home, settings::cached()).unwrap_or_default();
    let raw_days = raw_days
        .unwrap_or(config.raw_artifact_retention_days)
        .max(1);
    let event_days = event_days.unwrap_or(config.log_retention_days).max(1);
    let conn = store::open_store(&home.join(STORE_FILE_NAME))?;
    let now = current_timestamp();
    let raw_cutoff = store::retention_cutoff(&now, raw_days);
    let event_cutoff = store::retention_cutoff(&now, event_days);
    let raw_count = store::count_prunable_sensitive_artifacts(&conn, &now, &raw_cutoff)?;
    let event_count = store::count_events_older_than(&conn, &event_cutoff)?;

    if dry_run {
        println!(
            "logs prune dryRun=true rawArtifacts={raw_count} events={event_count} rawDays={raw_days} eventDays={event_days}"
        );
        return Ok(());
    }

    let raw_pruned = store::prune_sensitive_artifacts(&conn, &now, &raw_cutoff)?;
    let events_pruned = store::prune_events_older_than(&conn, &event_cutoff)?;
    println!(
        "logs pruned rawArtifacts={raw_pruned} events={events_pruned} rawCutoff={raw_cutoff} eventCutoff={event_cutoff}"
    );
    Ok(())
}

fn run_daemon_command(command: DaemonCommand) -> Result<()> {
    let home = coven_home_dir()?;
    match command {
        DaemonCommand::Start => {
            let current_exe =
                std::env::current_exe().context("failed to resolve current executable")?;
            let status =
                daemon::ensure_background_server(&home, &current_exe, current_timestamp())?;
            println!(
                "coven daemon status=running pid={} socket={}",
                status.pid, status.socket
            );
        }
        DaemonCommand::Restart => {
            let current_exe =
                std::env::current_exe().context("failed to resolve current executable")?;
            let (was_running, status) =
                daemon::restart_background_server(&home, &current_exe, current_timestamp())?;
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
        DaemonCommand::Status => match daemon::background_server_status(&home)? {
            Some(daemon::DaemonStatusState::Running(status)) => {
                let health = api::health_response(Some(status.clone()));
                println!(
                    "coven daemon status=running ok={} pid={} socket={}",
                    health.ok, status.pid, status.socket
                );
            }
            Some(daemon::DaemonStatusState::Stale(status)) => println!(
                "coven daemon status=stale ok=false pid={} socket={}",
                status.pid, status.socket
            ),
            None => {
                println!("coven daemon status=stopped");
                eprintln!(
                    "start it with `coven daemon start` (optional — `coven run` works without it)"
                );
            }
        },
        DaemonCommand::Stop => {
            if daemon::stop_background_server(&home)? {
                println!("coven daemon status=stopped");
            } else {
                println!("coven daemon was not running");
            }
        }
        DaemonCommand::Serve { tcp } => {
            #[cfg(unix)]
            {
                daemon::serve_forever(&home, current_timestamp(), tcp.as_deref())?;
            }
            #[cfg(windows)]
            {
                daemon::serve_forever(&home, current_timestamp(), tcp.as_deref())?;
            }
            #[cfg(not(any(unix, windows)))]
            {
                let _ = tcp;
                anyhow::bail!(
                    "coven daemon server is only implemented on Unix-like systems and Windows for now"
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

/// Lock stdout, emit one stream-JSON frame, release. Per-frame locking keeps
/// us from holding the lock across `pty_runner::run_attached`, which writes
/// the harness's own stdout through the same handle.
fn emit_stream_event(event: &stream_json::Event) -> Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    stream_json::emit_event(&mut handle, event)?;
    Ok(())
}

fn should_synthesize_stream_user_event(
    stream_json: bool,
    expanded_prompt: &str,
    detach: bool,
    harness_id: &str,
) -> bool {
    stream_json && !expanded_prompt.is_empty() && (detach || harness_id != "claude")
}

/// Exit with the failed session's exit code so scripts can gate on
/// `coven run ... && next-step`. The ledger has already recorded the failure
/// and, on stream paths, the JSONL result event has already been emitted.
fn exit_with_session_code(exit_code: i32, stream_json: bool) -> ! {
    if !stream_json {
        eprintln!("session failed (exit code {exit_code})");
    }
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();
    std::process::exit(exit_code);
}

#[allow(clippy::too_many_arguments)]
fn run_session(
    harness_id: &str,
    prompt_args: &[String],
    cwd: Option<&Path>,
    title: Option<&str>,
    detach: bool,
    continue_session: Option<&str>,
    labels: Vec<String>,
    visibility: Option<&str>,
    archive: bool,
    familiar_id: Option<&str>,
    model: Option<&str>,
    think: bool,
    speed: Option<&str>,
    permission: Option<&str>,
    stream_json: bool,
    stream_json_input: bool,
) -> Result<()> {
    // `stream_json_input` is consumed by the claude pass-through in 4.4; for
    // non-stream harnesses it has no effect on this path.
    let prompt = if prompt_args.is_empty() {
        String::new()
    } else {
        joined_prompt(prompt_args)?
    };

    if prompt_args.is_empty() && continue_session.is_none() {
        anyhow::bail!("nothing to do: pass a prompt, or use --continue [ID] to resume a session");
    }

    let selected_harness = selected_available_harness(harness_id)?;
    let current_dir = std::env::current_dir().context("failed to read current directory")?;
    let project_root = project::canonical_project_root(&current_dir).with_context(|| {
        format!(
            "failed to resolve project root from {}",
            current_dir.display()
        )
    })?;
    let cwd = project::resolve_inside_root(&project_root, cwd).context("failed to resolve cwd")?;
    let coven_home = coven_home_dir()?;
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);

    // Expand @path / @T-<id> / @@search refs before dispatching to the harness.
    // Keep the original `prompt` for session title and human-facing output so titles
    // aren't blown out by inlined file content.
    let expanded_prompt = if prompt.is_empty() {
        String::new()
    } else {
        prompt_refs::expand_all(&cwd, &conn, &prompt)?
    };

    // Resolve familiar identity and build effective prompt.
    // For harnesses with a dedicated --system-prompt flag, identity is injected
    // via that flag in build_harness_command_with_conversation; the prompt stays
    // clean. For harnesses without one (Codex), we prepend a bracketed identity
    // preamble to the prompt here so the integration layer remains harness-agnostic.
    let familiar_ctx = familiar_identity::resolve_optional(&coven_home, familiar_id)?;
    let spec = harness::configured_harness_specs()?
        .into_iter()
        .find(|s| s.id == selected_harness.id);

    // Resolve the requested model. Cave sends a namespaced id; the harness arg
    // builders strip the provider/ prefix and forward the bare id to the native
    // model flag, while `system.init` echoes the requested id verbatim so Cave
    // can confirm acceptance with an exact match. Adapters that declare no model
    // mechanism warn (don't error) so a selection degrades gracefully. A blank
    // value is ignored.
    let requested_model: Option<&str> = model.map(str::trim).filter(|m| !m.is_empty());
    let requested_speed = speed.map(harness::HarnessSpeed::parse).transpose()?;
    // Resolve the requested sandbox/permission policy. It forwards to the
    // harness's native flag (codex `--sandbox`, claude `--permission-mode`) so
    // the composer's Access chip is enforced. Harnesses that declare no sandbox
    // mechanism warn (don't error) so a selection degrades gracefully. Absent
    // (`None`) leaves the harness at its default (equivalent to `full`).
    let requested_permission = permission.map(harness::Permission::parse).transpose()?;
    if let (Some(requested), Some(s)) = (requested_model, spec.as_ref()) {
        if !s.supports_model() {
            eprintln!(
                "warning: harness `{}` declares no model mechanism; --model {} is ignored \
                 (declare model_flag or model_arg_template in the adapter manifest to enable it)",
                s.id, requested
            );
        }
    }
    if think && !harness::harness_supports_think(&selected_harness.id) {
        eprintln!(
            "warning: harness `{}` does not support --think; ignoring the request",
            selected_harness.id
        );
    }
    if let Some(speed) = requested_speed {
        if !harness::harness_supports_speed(&selected_harness.id) {
            eprintln!(
                "warning: harness `{}` does not support --speed {}; ignoring the request",
                selected_harness.id,
                match speed {
                    harness::HarnessSpeed::Fast => "fast",
                    harness::HarnessSpeed::Balanced => "balanced",
                    harness::HarnessSpeed::Thorough => "thorough",
                }
            );
        }
    }
    if let (Some(requested), Some(s)) = (requested_permission, spec.as_ref()) {
        if !s.supports_permission() {
            eprintln!(
                "warning: harness `{}` declares no sandbox mechanism; --permission {} is ignored \
                 (declare a sandbox mapping in the adapter to enable it)",
                s.id,
                requested.as_str()
            );
        }
    }
    let launch_options = harness::HarnessLaunchOptions {
        model: requested_model,
        think,
        speed: requested_speed,
        permission: requested_permission,
    };

    let effective_prompt = match (&familiar_ctx, spec.as_ref()) {
        (Some(f), Some(s)) if s.system_prompt_flag.is_none() && !expanded_prompt.is_empty() => {
            format!(
                "{preamble}\n\n{prompt}",
                preamble = f.identity_preamble(),
                prompt = expanded_prompt
            )
        }
        _ => expanded_prompt.clone(),
    };

    // Resolve --continue: explicit id, "" (latest), or None (new session).
    let resumed_id: Option<String> = match continue_session {
        None => None,
        Some("") => {
            let latest =
                store::latest_active_for_project(&conn, project_root.to_str().unwrap_or(""))?;
            if latest.is_none() {
                anyhow::bail!(
                    "no active session to continue in {}; pass an explicit --continue <ID> or omit the flag",
                    project_root.display(),
                );
            }
            latest
        }
        Some(id) => Some(id.to_string()),
    };

    let (record, is_resume) = if let Some(ref id) = resumed_id {
        // Verify the session exists; reuse its row.
        let existing = store::list_sessions_including_archived(&conn)?
            .into_iter()
            .find(|s| &s.id == id);
        match existing {
            Some(mut r) => {
                // Mutate updated_at to now; keep labels/visibility/title from the original.
                r.updated_at = now.clone();
                (r, true)
            }
            None => anyhow::bail!("session {} not found in local store", id),
        }
    } else {
        let r = store::SessionRecord {
            id: Uuid::new_v4().to_string(),
            project_root: project_root.to_string_lossy().into_owned(),
            harness: selected_harness.id.to_string(),
            title: session_title(title, &prompt),
            status: DEFAULT_SESSION_STATUS.to_string(),
            exit_code: None,
            archived_at: None,
            created_at: now.clone(),
            updated_at: now,
            conversation_id: None,
            familiar_id: familiar_ctx.as_ref().map(|f| f.id.clone()),
            labels,
            visibility: visibility.unwrap_or("private").to_string(),
        };
        (r, false)
    };

    if !is_resume {
        store::insert_session(&conn, &record)?;
    }

    if !stream_json {
        println!(
            "Coven session {}",
            if is_resume { "resumed" } else { "created" }
        );
        println!("  id:      {}", record.id);
        println!("  harness: {}", record.harness);
        println!("  cwd:     {}", cwd.display());
        println!("  title:   {}", record.title);
    }

    if detach && is_resume {
        anyhow::bail!("--detach and --continue are mutually exclusive");
    }

    let stream_started = std::time::Instant::now();
    if stream_json {
        emit_stream_event(&stream_json::Event::System(stream_json::System {
            subtype: "init".into(),
            cwd: cwd.to_string_lossy().into_owned(),
            session_id: record.id.clone(),
            tools: Vec::new(),
            agent_mode: None,
            model: requested_model.map(str::to_string),
            permission: requested_permission.map(|p| p.as_str().to_string()),
        }))?;
    }

    // We synthesize the `user` event only on paths where the harness will
    // *not* emit it itself: detach (no harness runs) and codex / generic
    // non-stream harnesses. The claude pass-through skips this so we don't
    // duplicate the user message claude echoes through its native protocol.
    let synthesize_user_event = should_synthesize_stream_user_event(
        stream_json,
        &expanded_prompt,
        detach,
        &selected_harness.id,
    );

    if detach {
        if archive {
            eprintln!("warning: --archive ignored in --detach mode (session was never launched)");
        }
        if !stream_json {
            println!("\nDetached mode: session was recorded but the harness was not spawned.");
            println!("View it later with `coven sessions`.");
        } else {
            if synthesize_user_event {
                emit_stream_event(&stream_json::Event::User(stream_json::UserMessage {
                    message: stream_json::MessageBody {
                        role: "user".into(),
                        content: vec![stream_json::ContentBlock::Text {
                            text: expanded_prompt.clone(),
                        }],
                    },
                    session_id: record.id.clone(),
                    parent_tool_use_id: None,
                }))?;
            }
            emit_stream_event(&stream_json::Event::Result(stream_json::RunResult {
                subtype: "success".into(),
                duration_ms: stream_started.elapsed().as_millis() as u64,
                is_error: false,
                num_turns: 1,
                session_id: record.id.clone(),
                error: None,
            }))?;
        }
        return Ok(());
    }

    store::update_session_status(
        &conn,
        &record.id,
        RUNNING_SESSION_STATUS,
        None,
        &current_timestamp(),
    )?;

    // Claude's native stream-json: pipe its JSONL events through ours
    // between the init/result frames we already emit. The codex / generic
    // path below cannot do this because codex doesn't speak stream-json, so we
    // branch here after resolving the harness to claude.
    if stream_json && selected_harness.id == "claude" {
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        let claude_system_prompt: Option<String> =
            familiar_ctx.as_ref().map(|f| f.identity_preamble());
        let exit_code = pty_runner::stream_claude(
            &cwd,
            &record.id,
            is_resume,
            &effective_prompt,
            stream_json_input,
            claude_system_prompt.as_deref(),
            launch_options,
            &mut handle,
        );
        drop(handle);
        let exit_code = match exit_code {
            Ok(code) => code,
            Err(error) => {
                store::update_session_status(
                    &conn,
                    &record.id,
                    FAILED_SESSION_STATUS,
                    None,
                    &current_timestamp(),
                )?;
                emit_stream_event(&stream_json::Event::Result(stream_json::RunResult {
                    subtype: "error_during_execution".into(),
                    duration_ms: stream_started.elapsed().as_millis() as u64,
                    is_error: true,
                    num_turns: 1,
                    session_id: record.id.clone(),
                    error: Some(format!("{error:#}")),
                }))?;
                return Err(error);
            }
        };
        let is_error = exit_code != 0;
        let status = if is_error { "failed" } else { "completed" };
        store::update_session_status(
            &conn,
            &record.id,
            status,
            Some(exit_code),
            &current_timestamp(),
        )?;
        emit_stream_event(&stream_json::Event::Result(stream_json::RunResult {
            subtype: if is_error {
                "error_during_execution".into()
            } else {
                "success".into()
            },
            duration_ms: stream_started.elapsed().as_millis() as u64,
            is_error,
            num_turns: 1,
            session_id: record.id.clone(),
            error: None,
        }))?;
        if archive {
            let archived_at = current_timestamp();
            store::archive_session(&conn, &record.id, &archived_at)?;
        }
        if is_error {
            exit_with_session_code(exit_code, true);
        }
        return Ok(());
    }

    if synthesize_user_event {
        emit_stream_event(&stream_json::Event::User(stream_json::UserMessage {
            message: stream_json::MessageBody {
                role: "user".into(),
                content: vec![stream_json::ContentBlock::Text {
                    text: expanded_prompt.clone(),
                }],
            },
            session_id: record.id.clone(),
            parent_tool_use_id: None,
        }))?;
    }

    let conversation_hint = if is_resume {
        Some(harness::ConversationHint::Resume {
            id: record.id.clone(),
        })
    } else {
        None
    };
    // Only pass familiar_ctx to the arg builder for harnesses that have a
    // system_prompt_flag (e.g. Claude). For harnesses without one (e.g. Codex)
    // the preamble is already embedded in effective_prompt — passing ctx here
    // too would produce a double-injection.
    let familiar_for_args = spec
        .as_ref()
        .filter(|s| s.system_prompt_flag.is_some())
        .and(familiar_ctx.as_ref());
    let command = pty_runner::build_harness_command_with_conversation(
        &selected_harness.id,
        &effective_prompt,
        &cwd,
        harness_launch_mode_for_stdio(),
        conversation_hint.as_ref(),
        familiar_for_args,
        launch_options,
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
            if stream_json {
                let is_error = result.exit_code.is_some_and(|c| c != 0);
                emit_stream_event(&stream_json::Event::Result(stream_json::RunResult {
                    subtype: if is_error {
                        "error_during_execution".into()
                    } else {
                        "success".into()
                    },
                    duration_ms: stream_started.elapsed().as_millis() as u64,
                    is_error,
                    num_turns: 1,
                    session_id: record.id.clone(),
                    error: None,
                }))?;
            }
            if archive {
                let archived_at = current_timestamp();
                store::archive_session(&conn, &record.id, &archived_at)?;
                if !stream_json {
                    println!("\nArchived session {} at {archived_at}", record.id);
                }
            }
            if let Some(code) = result.exit_code.filter(|code| *code != 0) {
                exit_with_session_code(code, stream_json);
            }
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
            if stream_json {
                emit_stream_event(&stream_json::Event::Result(stream_json::RunResult {
                    subtype: "error_during_execution".into(),
                    duration_ms: stream_started.elapsed().as_millis() as u64,
                    is_error: true,
                    num_turns: 1,
                    session_id: record.id.clone(),
                    error: Some(format!("{error:#}")),
                }))?;
            }
            Err(error)
        }
    }
}

/// Resolve a full session id or a unique id prefix to a session record.
/// Exact matches win; otherwise a single prefix match is accepted, an
/// ambiguous prefix lists the candidates, and a miss points at `coven
/// sessions` so the user can find a real id.
fn resolve_session_ref(
    conn: &rusqlite::Connection,
    reference: &str,
) -> Result<store::SessionRecord> {
    if let Some(session) = store::get_session(conn, reference)? {
        return Ok(session);
    }
    if !reference.is_empty() {
        let matches: Vec<store::SessionRecord> = store::list_sessions_including_archived(conn)?
            .into_iter()
            .filter(|session| session.id.starts_with(reference))
            .collect();
        match matches.len() {
            0 => {}
            1 => return Ok(matches.into_iter().next().expect("one match")),
            _ => {
                let ids: Vec<String> = matches
                    .iter()
                    .take(5)
                    .map(|session| session.id.clone())
                    .collect();
                anyhow::bail!(
                    "session id prefix `{reference}` is ambiguous; it matches: {}",
                    ids.join(", ")
                );
            }
        }
    }
    anyhow::bail!("session `{reference}` not found; run `coven sessions --all` to list session ids")
}

/// Remedy line for the archive/sacrifice "still running" refusals: a session
/// whose process died externally keeps status=running until daemon startup
/// recovery marks it orphaned.
const STALE_RUNNING_HINT: &str =
    "if its process is already gone, run `coven daemon restart` to mark it orphaned, then retry";

fn archive_session_command(session_id: &str) -> Result<()> {
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let session = resolve_session_ref(&conn, session_id)?;
    let session_id = session.id.as_str();
    if session.status == RUNNING_SESSION_STATUS {
        anyhow::bail!(
            "session `{session_id}` is still running; stop it before archiving ({STALE_RUNNING_HINT})"
        );
    }
    if session.archived_at.is_some() {
        println!("session was already archived; nothing to do");
        return Ok(());
    }

    store::archive_session(&conn, session_id, &current_timestamp())?;
    println!("archived session");
    println!(
        "Summon it later with `coven summon SESSION_ID` (replace SESSION_ID with one from `coven sessions --all`)."
    );
    Ok(())
}

fn summon_session_command(session_id: &str) -> Result<()> {
    summon_only_command(session_id)?;
    attach_session(session_id)
}

/// Un-archive a session if needed, without attaching. Returns the session
/// record so callers (Cast's attach dispatcher) can decide what to do next.
/// Pulled out of `summon_session_command` so the Cast TUI path can summon
/// then re-enter through Cast's follower instead of the legacy attach loop.
pub(crate) fn summon_only_command(session_id: &str) -> Result<store::SessionRecord> {
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let session = resolve_session_ref(&conn, session_id)?;
    let session_id = session.id.clone();

    if session.archived_at.is_some() {
        store::summon_session(&conn, &session_id, &current_timestamp())?;
        eprintln!("summoned session from the archive");
        let Some(session) = store::get_session(&conn, &session_id)? else {
            anyhow::bail!("session `{session_id}` not found");
        };
        return Ok(session);
    }

    Ok(session)
}

fn sacrifice_session_command(session_id: &str, yes: bool) -> Result<()> {
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    // Resolve before asking for confirmation so a typo'd id fails immediately
    // instead of after the user has already agreed to a permanent delete.
    let session = resolve_session_ref(&conn, session_id)?;
    let session_id = session.id.as_str();
    if session.status == RUNNING_SESSION_STATUS {
        anyhow::bail!(
            "session `{session_id}` is still running; do not sacrifice live work ({STALE_RUNNING_HINT})"
        );
    }
    confirm_sacrifice(session_id, yes)?;

    store::sacrifice_session(&conn, session_id)?;
    println!("sacrificed session; its event log was permanently deleted");
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
    let session = resolve_session_ref(&conn, session_id)?;
    let session_id = session.id.as_str();

    eprintln!(
        "attached to session {} ({}, \"{}\", status: {})",
        session_id, session.harness, session.title, session.status
    );
    if session.status == RUNNING_SESSION_STATUS {
        eprintln!("following live output; Ctrl+C detaches (the session keeps running)");
    } else {
        eprintln!(
            "session is not running; replaying its recorded output (resume it with `coven run {} --continue {}`)",
            session.harness, session_id
        );
    }

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

#[cfg(unix)]
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
    let harnesses = harness::configured_harnesses()?;
    let configured_ids = harnesses
        .iter()
        .map(|harness| harness.id.as_str())
        .collect::<Vec<_>>();
    let selected = harnesses
        .iter()
        .find(|harness| harness.id == harness_id)
        .cloned();

    match selected {
        Some(harness) if harness.available => Ok(harness),
        Some(harness) => Err(anyhow!(
            "harness `{}` is not available. {}",
            harness.id,
            harness.install_hint
        )),
        None => Err(anyhow!(
            "{}",
            harness::unsupported_harness_message(harness_id, &configured_ids)
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

pub(crate) fn current_timestamp() -> String {
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

fn coven_store_path_if_exists() -> Result<Option<PathBuf>> {
    let store_path = coven_home_dir()?.join(STORE_FILE_NAME);
    Ok(store_path.exists().then_some(store_path))
}

fn coven_home_dir() -> Result<PathBuf> {
    coven_home_from_env(
        std::env::var_os("COVEN_HOME"),
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
        std::env::var_os("HOMEDRIVE"),
        std::env::var_os("HOMEPATH"),
        dirs_next::home_dir().map(OsString::from),
    )
}

fn coven_home_from_env(
    coven_home: Option<OsString>,
    home: Option<OsString>,
    user_profile: Option<OsString>,
    home_drive: Option<OsString>,
    home_path: Option<OsString>,
    platform_home: Option<OsString>,
) -> Result<PathBuf> {
    if let Some(coven_home) = coven_home.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(coven_home));
    }

    let home = home
        .filter(|value| !value.is_empty())
        .or_else(|| user_profile.filter(|value| !value.is_empty()))
        .or_else(|| windows_home_from_drive_and_path(home_drive, home_path))
        .or_else(|| platform_home.filter(|value| !value.is_empty()))
        .ok_or_else(|| {
            anyhow!(
                "could not find a home directory for Coven. Set COVEN_HOME to choose a store path, \
for example `COVEN_HOME=$HOME/.coven` on macOS/Linux or \
`$env:COVEN_HOME=\"$env:USERPROFILE\\.coven\"` in PowerShell."
            )
        })?;
    Ok(PathBuf::from(home).join(DEFAULT_COVEN_HOME_DIR))
}

fn windows_home_from_drive_and_path(
    home_drive: Option<OsString>,
    home_path: Option<OsString>,
) -> Option<OsString> {
    let drive = home_drive?.into_string().ok()?;
    let path = home_path?.into_string().ok()?;
    if drive.is_empty() || path.is_empty() {
        return None;
    }
    Some(OsString::from(format!("{drive}{path}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::cast::{
        build_plan, parse_spell, CastHarness, CastIntent, CastRisk, CastStepKind, SafetyDecision,
    };
    use crate::tui::is_key_press;
    use crate::tui::sessions::{
        format_session_line, render_session_browser_frame_plain, render_sessions_json,
        session_browser_action_row_to_index, session_browser_actions,
        session_browser_session_row_to_index, sessions_command_mode, SessionsCommandMode,
        SESSION_BROWSER_FIRST_SESSION_ROW,
    };
    use crate::tui::shell::{
        cast_non_interactive_frame_for_test, magical_tui_inner_width_for_columns,
        magical_tui_items, move_magical_tui_selection, parse_magical_tui_input,
        render_magical_tui_frame_for_raw_terminal, render_magical_tui_frame_plain,
        render_magical_tui_frame_plain_with_input, render_magical_tui_frame_plain_with_width,
        MagicalTuiMove, MagicalTuiRequest, MAGICAL_TUI_MAX_INNER_WIDTH,
    };
    use crossterm::event::KeyEventKind;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn restore_env_var(name: &str, previous: Option<OsString>) {
        match previous {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }

    struct EnvVarGuard {
        name: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(name: &'static str, value: impl AsRef<OsStr>) -> Self {
            let previous = std::env::var_os(name);
            std::env::set_var(name, value);
            Self { name, previous }
        }

        fn remove(name: &'static str) -> Self {
            let previous = std::env::var_os(name);
            std::env::remove_var(name);
            Self { name, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            restore_env_var(self.name, self.previous.clone());
        }
    }

    #[test]
    fn tui_launcher_and_session_browser_are_owned_by_tui_modules() {
        let shell_frame = tui::shell::render_frame_plain_for_test(0);
        assert!(shell_frame.contains("Cast"));

        let sessions = [test_session_record(
            "session-alpha-1234567890",
            "completed",
            "codex",
            "Fix the failing tests before demo",
            None,
        )];
        let browser_frame = tui::sessions::render_browser_frame_plain_for_test(&sessions, 0, 0);
        assert!(browser_frame.contains("Session browser"));
    }

    #[test]
    fn near_miss_subcommand_catches_common_typos() {
        assert_eq!(near_miss_subcommand("sesions").as_deref(), Some("sessions"));
        assert_eq!(near_miss_subcommand("sessons").as_deref(), Some("sessions"));
        assert_eq!(near_miss_subcommand("docter").as_deref(), Some("doctor"));
        assert_eq!(near_miss_subcommand("attch").as_deref(), Some("attach"));
    }

    #[test]
    fn near_miss_subcommand_ignores_ordinary_prompts() {
        assert_eq!(near_miss_subcommand("refactor"), None);
        assert_eq!(near_miss_subcommand("hello"), None);
        assert_eq!(near_miss_subcommand("explain"), None);
    }

    #[test]
    fn edit_distance_matches_expected_values() {
        assert_eq!(edit_distance("sessions", "sessions"), 0);
        assert_eq!(edit_distance("sesions", "sessions"), 1);
        assert_eq!(edit_distance("", "run"), 3);
        assert_eq!(edit_distance("chat", "wt"), 3);
    }

    #[test]
    fn resolve_session_ref_accepts_unique_prefix_and_rejects_ambiguity() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = store::open_store(&temp_dir.path().join("store.sqlite3"))?;
        let mut record = store::SessionRecord {
            id: "aaaa1111-0000-0000-0000-000000000000".to_string(),
            project_root: "/tmp/project".to_string(),
            harness: "codex".to_string(),
            title: "first".to_string(),
            status: "completed".to_string(),
            exit_code: Some(0),
            archived_at: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            conversation_id: None,
            familiar_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
        };
        store::insert_session(&conn, &record)?;
        record.id = "aaab2222-0000-0000-0000-000000000000".to_string();
        record.title = "second".to_string();
        store::insert_session(&conn, &record)?;

        // Unique prefix resolves; exact id resolves; ambiguous and unknown fail.
        assert_eq!(
            resolve_session_ref(&conn, "aaaa")?.id,
            "aaaa1111-0000-0000-0000-000000000000"
        );
        assert_eq!(
            resolve_session_ref(&conn, "aaab2222-0000-0000-0000-000000000000")?.id,
            "aaab2222-0000-0000-0000-000000000000"
        );
        let ambiguous = resolve_session_ref(&conn, "aaa").unwrap_err();
        assert!(ambiguous.to_string().contains("ambiguous"));
        let missing = resolve_session_ref(&conn, "zzzz").unwrap_err();
        assert!(missing.to_string().contains("coven sessions --all"));
        Ok(())
    }

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
    fn stream_json_user_event_synthesis_skips_live_claude_passthrough() {
        assert!(should_synthesize_stream_user_event(
            true, "hello", true, "claude"
        ));
        assert!(should_synthesize_stream_user_event(
            true, "hello", false, "codex"
        ));
        assert!(!should_synthesize_stream_user_event(
            true, "hello", false, "claude"
        ));
        assert!(!should_synthesize_stream_user_event(
            false, "hello", false, "codex"
        ));
        assert!(!should_synthesize_stream_user_event(
            true, "", true, "codex"
        ));
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
            None,
            None,
            None,
            None,
        )?;

        assert_eq!(path, PathBuf::from("/tmp/custom-coven-home"));
        Ok(())
    }

    #[test]
    fn coven_home_from_env_defaults_under_home() -> Result<()> {
        let path = coven_home_from_env(
            None,
            Some(OsString::from("/tmp/user-home")),
            None,
            None,
            None,
            None,
        )?;

        assert_eq!(path, PathBuf::from("/tmp/user-home").join(".coven"));
        Ok(())
    }

    #[test]
    fn coven_home_from_env_uses_windows_drive_and_path_when_needed() -> Result<()> {
        let path = coven_home_from_env(
            None,
            None,
            None,
            Some(OsString::from("C:")),
            Some(OsString::from("\\Users\\hostname")),
            None,
        )?;

        assert_eq!(
            path,
            PathBuf::from("C:\\Users\\hostname").join(DEFAULT_COVEN_HOME_DIR)
        );
        Ok(())
    }

    #[test]
    fn cli_defaults_to_magical_tui_when_no_subcommand_is_provided() {
        let cli = Cli::parse_from(["coven"]);

        assert!(cli.command.is_none());
    }

    #[test]
    fn cli_accepts_bare_prompt_as_cast_spell() {
        let parsed = Cli::try_parse_from(["coven", "fix tests"])
            .expect("bare prompt should be accepted for script-friendly Cast input");

        assert!(parsed.command.is_none());
        assert_eq!(parsed.prompt, vec!["fix tests"]);
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
    fn cli_accepts_chat_command() {
        let cli = Cli::parse_from(["coven", "chat"]);

        match cli.command {
            Some(Command::Chat) => {}
            other => panic!("expected chat command, got {other:?}"),
        }
    }

    #[test]
    fn default_tui_and_chat_use_shared_chat_shell_for_interactive_terminals() {
        assert_eq!(
            interactive_shell_route(None, true, true),
            InteractiveShellRoute::Chat
        );
        assert_eq!(
            interactive_shell_route(Some(&Command::Tui), true, true),
            InteractiveShellRoute::Chat
        );
        assert_eq!(
            interactive_shell_route(Some(&Command::Chat), true, true),
            InteractiveShellRoute::Chat
        );
    }

    #[test]
    fn default_tui_and_chat_keep_plain_cast_output_for_pipes() {
        assert_eq!(
            interactive_shell_route(None, true, false),
            InteractiveShellRoute::PlainCast
        );
        assert_eq!(
            interactive_shell_route(Some(&Command::Tui), false, true),
            InteractiveShellRoute::PlainCast
        );
        assert_eq!(
            interactive_shell_route(Some(&Command::Chat), false, false),
            InteractiveShellRoute::PlainCast
        );
    }

    #[test]
    fn magical_tui_frame_opens_with_cast_identity_and_lists_core_commands() {
        let frame = render_magical_tui_frame_plain(1);

        // Identity line replaces the old "CovenCLI" header + "Welcome back" salute.
        assert!(frame.contains("Cast"));
        assert!(!frame.contains("CovenCLI"));
        assert!(!frame.contains("Welcome back"));
        // Core commands still render in the visible window (selection 1).
        assert!(frame.contains("/start"));
        assert!(frame.contains("/help"));
        assert!(frame.contains("/run"));
        // Selection arrow uses the thin guillemet (U+203A), not ASCII `>`.
        assert!(
            frame.contains('›'),
            "selected row should render with U+203A"
        );
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
                "/help",
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
    fn magical_tui_frame_previews_selected_action() {
        let frame = render_magical_tui_frame_plain(0);

        // The "Selected command" panel collapses to compact spell/detail rows.
        assert!(frame.contains("spell"));
        assert!(frame.contains("detail"));
        assert!(frame.contains("/start"));
        assert!(frame.contains("Setup check"));
        // The decorative "Store: ~/.coven" footer is gone per design contract.
        assert!(!frame.contains("Store:"));
    }

    #[test]
    fn magical_tui_frame_surfaces_command_rail_and_snapshot_for_newcomers() {
        let frame = render_magical_tui_frame_plain(5);

        // Two-lane body: left command rail + right snapshot lane.
        assert!(frame.contains("Commands"));
        assert!(frame.contains("Snapshot"));
        // Snapshot label column is rendered in lowercase per the contract.
        assert!(frame.contains("project"));
        assert!(frame.contains("harness"));
        assert!(frame.contains("daemon"));
        // /run is in the visible window when selection sits on it.
        assert!(frame.contains("/run"));
        assert!(frame.contains("Run an agent"));
        // Single-line footer hint, dot-separated.
        assert!(frame.contains("enter run"));
        assert!(frame.contains("esc quit"));
    }

    #[test]
    fn magical_tui_frame_renders_prompt_input() {
        let frame = render_magical_tui_frame_plain_with_input(0, "summarize the repo", 76);

        assert!(frame.contains("> summarize the repo"));
    }

    #[test]
    fn magical_tui_frame_wraps_prompt_in_thin_horizontal_rules() {
        let frame = render_magical_tui_frame_plain_with_input(0, "summarize the repo", 76);

        // No `+--+` corner art; single horizontal rule above and below the prompt.
        assert!(!frame.contains("+--"));
        assert!(!frame.contains("Ask anything"));
        assert!(
            frame.contains("─"),
            "prompt should be flanked by thin rules"
        );
        // The prompt itself is the bare `> input` line, no inner `|` bezels.
        assert!(frame.contains("> summarize the repo"));
        assert!(!frame.contains("| > summarize the repo"));
    }

    #[test]
    fn magical_tui_frame_drops_decorative_graph_and_task_inbox() {
        let frame = render_magical_tui_frame_plain(0);

        // Workspace map graph art, task inbox, and "Selected command" panel
        // are all removed per the Phase 1 design contract.
        assert!(!frame.contains("[memory]"));
        assert!(!frame.contains("[gateway]"));
        assert!(!frame.contains("[ ] inspect repo"));
        assert!(!frame.contains("Workspace map"));
        assert!(!frame.contains("Task inbox"));
        assert!(!frame.contains("Selected command"));
    }

    #[test]
    fn magical_tui_frame_avoids_emoji_and_decorative_ascii_chrome() {
        let frame = render_magical_tui_frame_plain(0);

        // No emoji or pictographs sneak in (BMP-only, no codepoints past U+2FFF
        // except whitelisted typography we use in the frame).
        for ch in frame.chars() {
            let code = ch as u32;
            let allowed = ch == '\n'
                || ch == '\r'
                || ch.is_ascii()
                || ch == '─'   // U+2500 thin horizontal rule
                || ch == '›'   // U+203A selected-row marker
                || ch == '·'   // U+00B7 separator
                || ch == '↑'
                || ch == '↓'
                || ch == '…'; // U+2026 truncation marker from fit_chars
            assert!(
                allowed,
                "unexpected glyph in launcher frame: {ch:?} (U+{code:04X})"
            );
        }
        // No ASCII corner-box chrome remains.
        assert!(!frame.contains("+--"));
        assert!(!frame.contains("--+"));
    }

    #[test]
    fn magical_tui_frame_follows_phase1_hierarchy() {
        let frame = render_magical_tui_frame_plain(0);

        // identity → prompt → commands + snapshot → action preview → footer
        assert!(frame.contains("Cast"));
        assert!(frame.contains("Commands"));
        assert!(frame.contains("Snapshot"));
        assert!(frame.contains("spell"));
        assert!(frame.contains("detail"));
        // Single-line dim footer, no `|` separators.
        assert!(frame.contains("enter run"));
        assert!(frame.contains("↑↓ select"));
        assert!(frame.contains("esc quit"));
        assert!(frame.contains("ctrl+u clear"));
        assert!(!frame.contains("Empty Enter"));
    }

    #[test]
    fn magical_tui_frame_keeps_blank_input_placeholder_dim() {
        let frame = render_magical_tui_frame_plain(0);
        // Empty prompt shows the placeholder copy; no `Ask anything` label.
        assert!(frame.contains("> type a task or /run codex"));
    }

    #[test]
    fn magical_tui_frame_windows_long_command_list_with_scroll_hint() {
        // Selection sits well past the visible window — scroll hint must
        // appear and the selected slash must still be in the rendered rail.
        let frame = render_magical_tui_frame_plain(12); // /sacrifice
        assert!(frame.contains("/sacrifice"));
        assert!(frame.contains("of 14"));
    }

    #[test]
    fn magical_tui_frame_stays_within_supported_screen_widths() {
        for inner_width in [18, 24, 34, 76, 96] {
            let frame = render_magical_tui_frame_plain_with_width(3, inner_width);

            for line in frame.lines() {
                assert!(
                    line.chars().count() <= inner_width,
                    "line exceeded width {inner_width}: {line}"
                );
            }
        }
    }

    #[test]
    fn magical_tui_width_tracks_terminal_columns_without_overflowing() {
        assert_eq!(
            magical_tui_inner_width_for_columns(120),
            MAGICAL_TUI_MAX_INNER_WIDTH
        );
        assert_eq!(magical_tui_inner_width_for_columns(80), 78);
        assert_eq!(magical_tui_inner_width_for_columns(36), 34);
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
        let frame = render_magical_tui_frame_for_raw_terminal(0, "");

        assert!(frame.contains("\r\n"));
        assert!(!frame.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn magical_tui_input_routes_plain_language_to_default_prompt() -> Result<()> {
        assert_eq!(
            parse_magical_tui_input("fix the failing tests")?,
            MagicalTuiRequest::NaturalPrompt("fix the failing tests".to_string())
        );
        Ok(())
    }

    #[test]
    fn magical_tui_input_routes_harness_slash_commands() -> Result<()> {
        assert_eq!(
            parse_magical_tui_input("/run codex explain this repo")?,
            MagicalTuiRequest::HarnessPrompt {
                harness: "codex".to_string(),
                prompt: "explain this repo".to_string(),
            }
        );
        assert_eq!(
            parse_magical_tui_input("/claude review the diff")?,
            MagicalTuiRequest::HarnessPrompt {
                harness: "claude".to_string(),
                prompt: "review the diff".to_string(),
            }
        );
        Ok(())
    }

    #[test]
    fn magical_tui_input_routes_session_slash_commands() -> Result<()> {
        assert_eq!(
            parse_magical_tui_input("/attach abc123")?,
            MagicalTuiRequest::AttachSession("abc123".to_string())
        );
        Ok(())
    }

    #[test]
    fn cast_non_interactive_frame_introduces_cast_and_shows_examples() {
        let project = PathBuf::from("/tmp/some-repo");
        let frame = cast_non_interactive_frame_for_test(Some(&project), Some("codex"));

        assert!(frame.contains("Cast"), "frame is missing the Cast salute");
        assert!(frame.contains("Coven familiar"));
        assert!(frame.contains("/tmp/some-repo"));
        assert!(frame.contains("codex"));
        assert!(frame.contains("fix the failing tests"));
        assert!(frame.contains("run claude polish the README"));
        assert!(frame.contains("/sessions"));
    }

    #[test]
    fn cast_parses_natural_text_as_default_harness_spell() {
        let intent = parse_spell("fix the failing tests").expect("parse");
        match intent {
            CastIntent::NaturalSpell { prompt } => assert_eq!(prompt, "fix the failing tests"),
            other => panic!("expected natural spell, got {other:?}"),
        }
    }

    #[test]
    fn cast_routes_run_claude_plain_language_to_claude() {
        let intent = parse_spell("run claude polish the README").expect("parse");
        match intent {
            CastIntent::HarnessSpell { harness, prompt } => {
                assert_eq!(harness, CastHarness::Claude);
                assert_eq!(prompt, "polish the README");
            }
            other => panic!("expected harness spell, got {other:?}"),
        }
    }

    #[test]
    fn cast_routes_sessions_keyword_to_session_browser() {
        let intent = parse_spell("sessions").expect("parse");
        assert!(matches!(intent, CastIntent::OpenSessions));
    }

    #[test]
    fn cast_plan_picks_safe_default_harness_for_natural_spell() {
        let plan = build_plan(parse_spell("fix the failing tests").unwrap(), || {
            Some(CastHarness::Codex)
        })
        .unwrap();

        assert_eq!(plan.risk(), CastRisk::Safe);
        let harness = plan.harness.expect("default harness should be resolved");
        assert_eq!(harness.harness, CastHarness::Codex);
        assert!(plan
            .steps
            .iter()
            .any(|step| step.kind == CastStepKind::LaunchSession));
    }

    #[test]
    fn cast_plan_marks_publish_spells_as_confirmation_required() {
        let plan = build_plan(
            parse_spell("publish the new crate to crates.io").unwrap(),
            || Some(CastHarness::Codex),
        )
        .unwrap();

        assert_eq!(plan.risk(), CastRisk::Confirm);
    }

    #[test]
    fn cast_plan_for_sacrifice_describes_typed_confirm_in_copy() {
        let plan = build_plan(parse_spell("/sacrifice abcdef123456").unwrap(), || {
            Some(CastHarness::Codex)
        })
        .unwrap();

        let inform_note = plan
            .steps
            .iter()
            .find(|step| matches!(step.kind, CastStepKind::Inform))
            .expect("sacrifice plan should include an inform step");
        assert!(
            inform_note.note.to_lowercase().contains("typed"),
            "sacrifice inform should describe typed-word confirm, got {:?}",
            inform_note.note
        );
        match plan.decision {
            SafetyDecision::Confirm { suggestion, .. } => {
                assert!(
                    suggestion.contains("`sacrifice`"),
                    "sacrifice suggestion should name the typed-confirm word, got {suggestion:?}"
                );
            }
            other => panic!("expected confirm, got {other:?}"),
        }
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
            Some(Command::Sessions {
                all,
                manage,
                plain,
                json,
                ..
            }) => {
                assert!(all);
                assert!(!manage);
                assert!(!plain);
                assert!(!json);
            }
            other => panic!("expected sessions command, got {other:?}"),
        }

        let managed = Cli::parse_from(["coven", "sessions", "--manage"]);
        match managed.command {
            Some(Command::Sessions { manage, plain, .. }) => {
                assert!(manage);
                assert!(!plain);
            }
            other => panic!("expected sessions command, got {other:?}"),
        }

        let json = Cli::parse_from(["coven", "sessions", "--json"]);
        match json.command {
            Some(Command::Sessions { json, .. }) => assert!(json),
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
    fn cli_accepts_logs_prune_options() {
        let cli = Cli::parse_from([
            "coven",
            "logs",
            "prune",
            "--dry-run",
            "--raw-days",
            "3",
            "--event-days",
            "14",
        ]);

        match cli.command {
            Some(Command::Logs {
                command:
                    LogsCommand::Prune {
                        dry_run,
                        raw_days,
                        event_days,
                    },
            }) => {
                assert!(dry_run);
                assert_eq!(raw_days, Some(3));
                assert_eq!(event_days, Some(14));
            }
            other => panic!("expected logs prune command, got {other:?}"),
        }
    }

    #[test]
    fn cli_accepts_adapter_list_and_doctor_commands() {
        let list = Cli::parse_from(["coven", "adapter", "list", "--json"]);
        match list.command {
            Some(Command::Adapter {
                command: AdapterCommand::List { json },
            }) => assert!(json),
            other => panic!("expected adapter list command, got {other:?}"),
        }

        let doctor_one = Cli::parse_from(["coven", "adapter", "doctor", "claude"]);
        match doctor_one.command {
            Some(Command::Adapter {
                command: AdapterCommand::Doctor { adapter },
            }) => assert_eq!(adapter.as_deref(), Some("claude")),
            other => panic!("expected adapter doctor command, got {other:?}"),
        }

        let install = Cli::parse_from(["coven", "adapter", "install", "hermes"]);
        match install.command {
            Some(Command::Adapter {
                command: AdapterCommand::Install { adapter },
            }) => assert_eq!(adapter, "hermes"),
            other => panic!("expected adapter install command, got {other:?}"),
        }
    }

    #[test]
    fn coven_home_uses_userprofile_when_home_is_missing() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let user_profile = temp_dir.path().join("windows-user");
        let _guard = env_lock().lock().unwrap();
        let _coven_home = EnvVarGuard::remove("COVEN_HOME");
        let _home = EnvVarGuard::remove("HOME");
        let _user_profile = EnvVarGuard::set("USERPROFILE", &user_profile);

        assert_eq!(coven_home_dir()?, user_profile.join(DEFAULT_COVEN_HOME_DIR));
        Ok(())
    }

    #[test]
    fn sessions_command_opens_browser_for_humans_and_plain_tables_for_scripts() {
        assert_eq!(
            sessions_command_mode(true, true, false, false, false),
            SessionsCommandMode::Browser
        );
        assert_eq!(
            sessions_command_mode(false, true, false, false, false),
            SessionsCommandMode::List
        );
        assert_eq!(
            sessions_command_mode(false, false, true, false, false),
            SessionsCommandMode::Browser
        );
        assert_eq!(
            sessions_command_mode(true, true, true, true, false),
            SessionsCommandMode::List
        );
        assert_eq!(
            sessions_command_mode(true, true, true, true, true),
            SessionsCommandMode::Json
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
            seq: 0,
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
            seq: 0,
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

    #[cfg(unix)]
    #[test]
    fn successful_http_response_accepts_2xx_only() {
        assert!(ensure_successful_http_response("HTTP/1.1 202 Accepted\r\n\r\n{}").is_ok());
        assert!(ensure_successful_http_response("HTTP/1.1 409 Conflict\r\n\r\n{}").is_err());
    }

    #[test]
    fn tui_key_handling_accepts_press_events_only() {
        assert!(is_key_press(KeyEventKind::Press));
        assert!(!is_key_press(KeyEventKind::Repeat));
        assert!(!is_key_press(KeyEventKind::Release));
    }

    #[test]
    fn cli_accepts_patch_with_only_name() {
        let cli = Cli::parse_from(["coven", "patch", "openclaw"]);

        match cli.command {
            Some(Command::Patch {
                name,
                issue,
                repo,
                harness,
                verify,
                non_interactive,
                dry_run,
                keep_session,
            }) => {
                assert_eq!(name.as_deref(), Some("openclaw"));
                assert!(issue.is_empty());
                assert!(repo.is_none());
                assert!(harness.is_none());
                assert!(verify.is_none());
                assert!(!non_interactive);
                assert!(!dry_run);
                assert!(!keep_session);
            }
            other => panic!("expected patch command, got {other:?}"),
        }
    }

    #[test]
    fn cli_accepts_patch_with_name_and_full_flag_set() {
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
                name,
                issue,
                repo,
                harness,
                verify,
                non_interactive,
                dry_run,
                keep_session,
            }) => {
                assert_eq!(name.as_deref(), Some("openclaw"));
                assert_eq!(issue, vec!["fix auth order".to_string()]);
                assert_eq!(repo.as_deref(), Some(Path::new("/repo/openclaw")));
                assert_eq!(harness.as_deref(), Some("codex"));
                assert_eq!(verify.as_deref(), Some("pnpm-check"));
                assert!(non_interactive);
                assert!(dry_run);
                assert!(keep_session);
            }
            other => panic!("expected patch command, got {other:?}"),
        }
    }

    #[test]
    fn cli_accepts_patch_with_no_name() {
        let cli = Cli::parse_from(["coven", "patch"]);

        match cli.command {
            Some(Command::Patch { name, issue, .. }) => {
                assert!(name.is_none());
                assert!(issue.is_empty());
            }
            other => panic!("expected patch command, got {other:?}"),
        }
    }

    #[test]
    fn cli_rejects_conflicting_sessions_output_modes() {
        for args in [
            ["coven", "sessions", "--json", "--plain"],
            ["coven", "sessions", "--json", "--manage"],
            ["coven", "sessions", "--plain", "--manage"],
        ] {
            assert!(
                Cli::try_parse_from(args).is_err(),
                "sessions output modes should be mutually exclusive: {args:?}"
            );
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
            id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            project_root: "/tmp/project".to_string(),
            harness: "codex".to_string(),
            title: "A useful session".to_string(),
            status: "created".to_string(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-04-27T06:00:00Z".to_string(),
            updated_at: "2026-04-27T06:00:00Z".to_string(),
            conversation_id: None,
            familiar_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
        };

        assert_eq!(
            format_session_line(&session),
            "550e8400-e29b-41d4-a716-446655440000 created    codex    active   A useful session"
        );
    }

    #[test]
    fn render_sessions_json_prints_client_friendly_session_array() -> Result<()> {
        let session = store::SessionRecord {
            id: "session-1".to_string(),
            project_root: "/tmp/project".to_string(),
            harness: "codex".to_string(),
            title: "Demo loop".to_string(),
            status: "running".to_string(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-05-14T07:00:00Z".to_string(),
            updated_at: "2026-05-14T07:00:01Z".to_string(),
            conversation_id: None,
            familiar_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
        };

        let rendered = render_sessions_json(&[session])?;
        let body: serde_json::Value = serde_json::from_str(&rendered)?;

        assert_eq!(body["sessions"][0]["id"], "session-1");
        assert_eq!(body["sessions"][0]["project_root"], "/tmp/project");
        assert_eq!(body["sessions"][0]["harness"], "codex");
        assert_eq!(body["sessions"][0]["status"], "running");
        Ok(())
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
            conversation_id: None,
            familiar_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
        }
    }

    #[test]
    fn legacy_tui_opt_in_respects_env_var() {
        // SAFETY: tests in this crate run on a single thread by default; if
        // that ever changes, gate this behind a serial mutex.
        let prev = std::env::var("COVEN_LEGACY_TUI").ok();
        std::env::set_var("COVEN_LEGACY_TUI", "1");
        assert!(legacy_tui_opted_in());
        std::env::set_var("COVEN_LEGACY_TUI", "true");
        assert!(legacy_tui_opted_in());
        std::env::set_var("COVEN_LEGACY_TUI", "0");
        assert!(!legacy_tui_opted_in());
        std::env::remove_var("COVEN_LEGACY_TUI");
        assert!(!legacy_tui_opted_in());
        if let Some(v) = prev {
            std::env::set_var("COVEN_LEGACY_TUI", v);
        }
    }

    #[test]
    fn missing_coven_code_error_includes_install_instructions() {
        // The error message is the primary onboarding surface when coven-code
        // is absent, so it must list at least one concrete install path and
        // mention the legacy escape hatch.
        let msg = missing_coven_code_error_message(TargetShell::Posix);
        assert!(msg.contains("npm install -g @opencoven/coven-code"));
        assert!(msg.contains("install.sh"));
        assert!(msg.contains("COVEN_LEGACY_TUI=1"));
    }

    #[test]
    fn missing_coven_code_error_includes_windows_powershell_instructions() {
        let msg = missing_coven_code_error_message(TargetShell::PowerShell);

        assert!(msg.contains("irm https://github.com/OpenCoven/coven-code/releases/latest/download/install.ps1 | iex"));
        assert!(msg.contains("$env:COVEN_LEGACY_TUI = \"1\""));
        assert!(msg.contains("Remove-Item Env:COVEN_LEGACY_TUI"));
        assert!(!msg.contains("install.sh | bash"));
    }

    #[test]
    fn coven_code_binary_lookup_returns_none_for_empty_path() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(coven_code_binary_from(Some(OsStr::new("")), Some(tmp.path())).is_none());
    }

    #[test]
    fn coven_code_binary_lookup_finds_windows_npm_cmd_shim() {
        let tmp = tempfile::tempdir().unwrap();
        let shim = tmp.path().join("coven-code.cmd");
        std::fs::write(&shim, "@echo off\r\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&shim).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&shim, permissions).unwrap();
        }
        let path_var = std::env::join_paths([tmp.path()]).unwrap();

        assert_eq!(
            coven_code_binary_from_names(
                Some(path_var.as_os_str()),
                None,
                &["coven-code.exe", "coven-code.cmd", "coven-code.bat"]
            ),
            Some(shim)
        );
    }
}
