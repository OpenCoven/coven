use std::collections::HashSet;
use std::ffi::OsString;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::{SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use uuid::Uuid;

mod api;
mod control_plane;
mod daemon;
mod harness;
mod openclaw_repo;
mod patch;
mod pc;
mod project;
mod pty_runner;
mod store;
mod theme;
mod tui;
mod verification;

const DEFAULT_COVEN_HOME_DIR: &str = ".coven";
pub(crate) const STORE_FILE_NAME: &str = "coven.sqlite3";
const DEFAULT_SESSION_STATUS: &str = "created";
const RUNNING_SESSION_STATUS: &str = "running";
const FAILED_SESSION_STATUS: &str = "failed";
const DEFAULT_TITLE_CHARS: usize = 48;
const OPENCLAW_REPOSITORY_ID: &str = "openclaw";

#[derive(Parser, Debug)]
#[command(name = "coven")]
#[command(version = env!("COVEN_VERSION_DESC"))]
#[command(about = "Run project-scoped coding agents without memorizing harness commands")]
#[command(
    long_about = "Coven runs Codex, Claude Code, and future harnesses inside a local, project-scoped session ledger. Run `coven` with no arguments for a beginner-friendly menu."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(
        value_name = "PROMPT",
        num_args = 0..,
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "Task to run through Cast when no subcommand is provided"
    )]
    prompt: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[command(about = "Interactive chat with Coven agents")]
    Chat,
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
        #[arg(long, help = "Print sessions as JSON for clients such as comux")]
        json: bool,
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
    #[command(about = "Diagnose and relieve macOS system pressure")]
    Pc {
        #[command(subcommand)]
        command: Option<pc::PcCommand>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InteractiveShellRoute {
    Chat,
    PlainCast,
}

fn main() -> Result<()> {
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
        Some(Command::Daemon { command }) => run_daemon_command(command),
        Some(Command::Run {
            harness,
            prompt,
            cwd,
            title,
            detach,
        }) => run_session(&harness, &prompt, cwd.as_deref(), title.as_deref(), detach),
        Some(Command::Sessions {
            all,
            manage,
            plain,
            json,
        }) => tui::sessions::run_command(all, manage, plain, json),
        Some(Command::Attach { session_id }) => attach_session(&session_id),
        Some(Command::Summon { session_id }) => summon_session_command(&session_id),
        Some(Command::Archive { session_id }) => archive_session_command(&session_id),
        Some(Command::Sacrifice { session_id, yes }) => sacrifice_session_command(&session_id, yes),
        Some(Command::Patch { command }) => run_patch_command(command),
        Some(Command::Pc { command }) => pc::run_pc_command(command),
    }
}

fn run_bare_prompt(prompt: &[String]) -> Result<()> {
    let prompt = joined_prompt(prompt)?;
    tui::shell::run_cast_spell(&prompt)
}

fn run_shared_interactive_shell() -> Result<()> {
    match interactive_shell_route(None, io::stdin().is_terminal(), io::stdout().is_terminal()) {
        InteractiveShellRoute::Chat => tui::chat::run_chat(),
        InteractiveShellRoute::PlainCast => tui::shell::run(),
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
    let stored_repo = stored_repository_path(OPENCLAW_REPOSITORY_ID)?;
    let detected_repo = openclaw_repo::detect_openclaw_repo_with_stored(
        repo.as_deref(),
        &start_dir,
        stored_repo.as_deref(),
    )?;
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
    remember_openclaw_repo_location(&request.repo)?;
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

fn remember_openclaw_repo_location(repo: &openclaw_repo::OpenClawRepo) -> Result<()> {
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let now = current_timestamp();
    let existing = store::get_repository(&conn, OPENCLAW_REPOSITORY_ID)?;
    store::upsert_repository(
        &conn,
        &store::RepositoryRecord {
            id: OPENCLAW_REPOSITORY_ID.to_string(),
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
    let Some(session) = store::get_session(&conn, session_id)? else {
        anyhow::bail!("session `{session_id}` not found");
    };

    if session.archived_at.is_some() {
        store::summon_session(&conn, session_id, &current_timestamp())?;
        eprintln!("summoned session from the archive");
        let Some(session) = store::get_session(&conn, session_id)? else {
            anyhow::bail!("session `{session_id}` not found");
        };
        return Ok(session);
    }

    Ok(session)
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
    let Some(session) = store::get_session(&conn, session_id)? else {
        anyhow::bail!("session `{session_id}` not found");
    };

    eprintln!(
        "attached to session status={} harness={} title={} ",
        session.status, session.harness, session.title
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
            }) => {
                assert!(all);
                assert!(!manage);
                assert!(!plain);
                assert!(!json);
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
            id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
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
        }
    }
}
