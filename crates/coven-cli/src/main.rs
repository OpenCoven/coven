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
#[command(about = "Project-scoped harness substrate for agent sessions")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Doctor,
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    Run {
        harness: String,
        #[arg(required = true, num_args = 1..)]
        prompt: Vec<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        detach: bool,
    },
    Sessions,
    Attach {
        session_id: String,
    },
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
    Status,
    Stop,
    #[command(hide = true)]
    Serve,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Doctor => run_doctor(),
        Command::Daemon { command } => run_daemon_command(command),
        Command::Run {
            harness,
            prompt,
            cwd,
            title,
            detach,
        } => run_session(&harness, &prompt, cwd.as_deref(), title.as_deref(), detach),
        Command::Sessions => list_sessions(),
        Command::Attach { session_id } => attach_session(&session_id),
        Command::Patch { command } => run_patch_command(command),
    }
}

fn run_doctor() -> Result<()> {
    println!("coven doctor");
    for harness in harness::built_in_harnesses() {
        let status = if harness.available {
            "available"
        } else {
            "missing"
        };
        println!("- {} ({}): {status}", harness.label, harness.executable);
        if !harness.available {
            println!("  {}", harness.install_hint);
        }
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
    let command =
        pty_runner::build_harness_command(selected_harness.id, &brief, &request.repo.root)?;
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
        created_at: now.clone(),
        updated_at: now,
    };

    store::insert_session(&conn, &record)?;

    println!(
        "created session {} harness={} cwd={}",
        record.id,
        record.harness,
        cwd.display()
    );

    if detach {
        println!("detached session {}; harness was not spawned", record.id);
        return Ok(());
    }

    store::update_session_status(
        &conn,
        &record.id,
        RUNNING_SESSION_STATUS,
        None,
        &current_timestamp(),
    )?;

    let command = pty_runner::build_harness_command(selected_harness.id, &prompt, &cwd)?;
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

fn list_sessions() -> Result<()> {
    let store_path = coven_store_path()?;
    let conn = store::open_store(&store_path)?;
    let sessions = store::list_sessions(&conn)?;

    if sessions.is_empty() {
        println!("No Coven sessions yet. Create one with `coven run <harness> <prompt...>`.");
    } else {
        for session in sessions {
            println!("{}", format_session_line(&session));
        }
    }

    Ok(())
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
    format!(
        "{} {} {} {}",
        session.id, session.status, session.harness, session.title
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
    fn cli_accepts_daemon_start_status_stop_and_hidden_serve_commands() {
        for subcommand in ["start", "status", "stop", "serve"] {
            let cli = Cli::parse_from(["coven", "daemon", subcommand]);
            match cli.command {
                Command::Daemon { .. } => {}
                other => panic!("expected daemon command, got {other:?}"),
            }
        }
    }

    #[test]
    fn cli_run_defaults_to_attached_and_accepts_detach() {
        let attached = Cli::parse_from(["coven", "run", "codex", "hello"]);
        let detached = Cli::parse_from(["coven", "run", "codex", "hello", "--detach"]);

        match attached.command {
            Command::Run { detach, .. } => assert!(!detach),
            other => panic!("expected run command, got {other:?}"),
        }

        match detached.command {
            Command::Run { detach, .. } => assert!(detach),
            other => panic!("expected run command, got {other:?}"),
        }
    }

    #[test]
    fn cli_accepts_attach_command() {
        let cli = Cli::parse_from(["coven", "attach", "session-1"]);

        match cli.command {
            Command::Attach { session_id } => assert_eq!(session_id, "session-1"),
            other => panic!("expected attach command, got {other:?}"),
        }
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
            Command::Patch {
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
            } => {
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
            Command::Patch {
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
            } => {
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
            created_at: "2026-04-27T06:00:00Z".to_string(),
            updated_at: "2026-04-27T06:00:00Z".to_string(),
        };

        assert_eq!(
            format_session_line(&session),
            "session-id created codex A useful session"
        );
    }
}
