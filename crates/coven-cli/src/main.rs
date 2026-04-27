use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::{SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use uuid::Uuid;

mod harness;
mod project;
mod store;

const DEFAULT_COVEN_HOME_DIR: &str = ".coven";
const STORE_FILE_NAME: &str = "coven.sqlite3";
const DEFAULT_SESSION_STATUS: &str = "created";
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
    Run {
        harness: String,
        #[arg(required = true, num_args = 1..)]
        prompt: Vec<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        title: Option<String>,
    },
    Sessions,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Doctor => run_doctor(),
        Command::Run {
            harness,
            prompt,
            cwd,
            title,
        } => run_session(&harness, &prompt, cwd.as_deref(), title.as_deref()),
        Command::Sessions => list_sessions(),
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

fn run_session(
    harness_id: &str,
    prompt_args: &[String],
    cwd: Option<&Path>,
    title: Option<&str>,
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
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let record = store::SessionRecord {
        id: Uuid::new_v4().to_string(),
        project_root: project_root.to_string_lossy().into_owned(),
        harness: selected_harness.id.to_string(),
        title: session_title(title, &prompt),
        status: DEFAULT_SESSION_STATUS.to_string(),
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
    Ok(())
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
    let home = coven_home_from_env(std::env::var_os("COVEN_HOME"), std::env::var_os("HOME"))?;
    std::fs::create_dir_all(&home)
        .with_context(|| format!("failed to create Coven home directory {}", home.display()))?;
    Ok(home.join(STORE_FILE_NAME))
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
    fn format_session_line_prints_id_status_harness_and_title() {
        let session = store::SessionRecord {
            id: "session-id".to_string(),
            project_root: "/tmp/project".to_string(),
            harness: "codex".to_string(),
            title: "A useful session".to_string(),
            status: "created".to_string(),
            created_at: "2026-04-27T06:00:00Z".to_string(),
            updated_at: "2026-04-27T06:00:00Z".to_string(),
        };

        assert_eq!(
            format_session_line(&session),
            "session-id created codex A useful session"
        );
    }
}
