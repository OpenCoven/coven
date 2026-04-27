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
mod project;
mod pty_runner;
mod store;

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
