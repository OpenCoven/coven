use anyhow::Context;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const CURATED_COMMANDS: &[&str] = &[
    "doctor", "setup", "run", "sessions", "attach", "daemon", "status", "help",
];
const PUBLIC_COMMANDS: &[&str] = &[
    "doctor",
    "setup",
    "run",
    "sessions",
    "attach",
    "daemon",
    "status",
    "chat",
    "tui",
    "help",
    "config",
    "completions",
    "adapter",
    "engine",
    "auth",
    "models",
    "acp",
    "code",
    "summon",
    "archive",
    "sacrifice",
    "kill",
    "familiars",
    "skills",
    "memory",
    "research",
    "calls",
    "hub",
    "scheduler",
    "travel",
    "wt",
    "claim",
    "maintenance",
    "hooks",
    "logs",
    "vacuum",
    "reset",
    "patch",
    "pc",
    "ward",
    "executor",
];
const GROUP_IDS: &[&str] = &[
    "start-and-launch",
    "configure-and-extend",
    "session-lifecycle",
    "observe-your-coven",
    "coordinate-parallel-work",
    "repair-and-administer",
];

fn coven_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_coven"))
}

struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn new(name: &str) -> anyhow::Result<Self> {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir
            .parent()
            .expect("crate root has workspace parent")
            .parent()
            .expect("workspace root");
        let unique = format!(
            "{name}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        );
        let path = workspace_root
            .join("target")
            .join("help-disclosure-tests")
            .join(unique);
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn isolated_help_command(root: &Path, args: &[&str]) -> anyhow::Result<Command> {
    let current_dir = root.join("current");
    let user_home = root.join("user-home");
    let xdg_config = root.join("xdg-config");
    fs::create_dir_all(&current_dir)?;
    fs::create_dir_all(&user_home)?;
    fs::create_dir_all(&xdg_config)?;

    let mut command = Command::new(coven_bin());
    command
        .args(args)
        .current_dir(&current_dir)
        .env("COVEN_HOME", root.join("coven-home"))
        .env("HOME", &user_home)
        .env("USERPROFILE", &user_home)
        .env("XDG_CONFIG_HOME", &xdg_config)
        .env("PATH", OsString::new())
        .env_remove("COVEN_SETTINGS_PATH")
        .env_remove("COVEN_HARNESS_ADAPTER_DIRS")
        .env_remove("COVEN_HARNESS_ADAPTER_MANIFEST");
    Ok(command)
}

fn closed_stdout_help_output(root: &Path, args: &[&str]) -> anyhow::Result<Output> {
    let mut command = isolated_help_command(root, args)?;
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    drop(child.stdout.take());
    child.wait_with_output().map_err(Into::into)
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected success, got {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "help should keep stderr empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(output: &Output) {
    assert!(
        !output.status.success(),
        "expected failure, got success\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn listed_commands(help: &str) -> Vec<String> {
    help.lines()
        .filter_map(|line| {
            let trimmed = line.strip_prefix("  ")?;
            let name = trimmed.split_whitespace().next()?;
            let separator = &trimmed[name.len()..];
            if separator.starts_with("  ")
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
            {
                Some(name.to_owned())
            } else {
                None
            }
        })
        .collect()
}

fn command_from_json<'a>(json: &'a Value, name: &str) -> Option<&'a Value> {
    json["groups"]
        .as_array()?
        .iter()
        .flat_map(|group| group["commands"].as_array().into_iter().flatten())
        .find(|command| command["name"] == name)
}

#[test]
fn top_level_help_is_concise_and_help_subcommand_matches() -> anyhow::Result<()> {
    let scratch = ScratchDir::new("top-level-help")?;

    let short_help = isolated_help_command(scratch.path(), &["-h"])?.output()?;
    assert_success(&short_help);

    let top_level = isolated_help_command(scratch.path(), &["--help"])?.output()?;
    assert_success(&top_level);

    let explicit_help = isolated_help_command(scratch.path(), &["help"])?.output()?;
    assert_success(&explicit_help);

    assert_eq!(explicit_help.stdout, top_level.stdout);
    assert_eq!(
        listed_commands(&String::from_utf8(short_help.stdout)?),
        CURATED_COMMANDS
    );
    let stdout = String::from_utf8(top_level.stdout)?;
    assert!(stdout.contains("Run `coven` with no arguments to open the interactive Coven UI"));
    assert_eq!(listed_commands(&stdout), CURATED_COMMANDS);
    assert!(!stdout.contains("\n  chat  "));
    assert!(!stdout.contains("\n  config  "));
    assert!(
        !scratch.path().join("coven-home").exists(),
        "help should not initialize COVEN_HOME state"
    );
    Ok(())
}

#[test]
fn full_help_lists_each_public_command_once_and_hides_internal_commands() -> anyhow::Result<()> {
    let scratch = ScratchDir::new("full-help")?;

    let output = isolated_help_command(scratch.path(), &["help", "--all"])?.output()?;
    assert_success(&output);
    let stdout = String::from_utf8(output.stdout)?;

    assert_eq!(listed_commands(&stdout), PUBLIC_COMMANDS);
    assert!(!stdout.contains("process-supervisor"));
    assert!(!stdout.contains("daemon serve"));
    Ok(())
}

#[test]
fn help_subcommand_matches_direct_command_help() -> anyhow::Result<()> {
    let scratch = ScratchDir::new("command-help")?;

    let via_help = isolated_help_command(scratch.path(), &["help", "run"])?.output()?;
    let direct = isolated_help_command(scratch.path(), &["run", "--help"])?.output()?;
    assert_success(&via_help);
    assert_success(&direct);
    assert_eq!(via_help.stdout, direct.stdout);

    let nested_via_help =
        isolated_help_command(scratch.path(), &["help", "sessions", "show"])?.output()?;
    let nested_direct =
        isolated_help_command(scratch.path(), &["sessions", "show", "--help"])?.output()?;
    assert_success(&nested_via_help);
    assert_success(&nested_direct);
    assert_eq!(nested_via_help.stdout, nested_direct.stdout);

    let daemon_via_help = isolated_help_command(scratch.path(), &["help", "daemon"])?.output()?;
    let daemon_direct = isolated_help_command(scratch.path(), &["daemon", "--help"])?.output()?;
    assert_success(&daemon_via_help);
    assert_success(&daemon_direct);
    assert_eq!(daemon_via_help.stdout, daemon_direct.stdout);
    let daemon_stdout = String::from_utf8_lossy(&daemon_via_help.stdout);
    assert!(daemon_stdout.contains("background daemon process"));
    assert!(!daemon_stdout.contains("daemon serve"));

    assert!(
        !scratch.path().join("coven-home").exists(),
        "command-specific help should not initialize COVEN_HOME state"
    );

    let self_help = isolated_help_command(scratch.path(), &["help", "help"])?.output()?;
    let direct_self_help = isolated_help_command(scratch.path(), &["help", "--help"])?.output()?;
    assert_success(&self_help);
    assert_success(&direct_self_help);
    assert_eq!(self_help.stdout, direct_self_help.stdout);

    let via_help_with_color =
        isolated_help_command(scratch.path(), &["help", "run", "--color=always"])?.output()?;
    let direct_with_color =
        isolated_help_command(scratch.path(), &["run", "--help", "--color=always"])?.output()?;
    assert_success(&via_help_with_color);
    assert_success(&direct_with_color);
    assert_eq!(via_help_with_color.stdout, direct_with_color.stdout);

    for args in [
        &["daemon", "help", "--color=bogus"][..],
        &["daemon", "help", "--color", "bogus"][..],
    ] {
        let invalid_color = isolated_help_command(scratch.path(), args)?.output()?;
        assert_failure(&invalid_color);
        assert!(String::from_utf8(invalid_color.stderr)?.contains("invalid value 'bogus'"));
    }
    Ok(())
}

#[test]
fn nested_help_subcommands_remain_available() -> anyhow::Result<()> {
    let scratch = ScratchDir::new("nested-help")?;

    let daemon_help = isolated_help_command(scratch.path(), &["daemon", "help"])?.output()?;
    let daemon_direct = isolated_help_command(scratch.path(), &["daemon", "--help"])?.output()?;
    assert_success(&daemon_help);
    assert_success(&daemon_direct);
    assert_eq!(daemon_help.stdout, daemon_direct.stdout);

    let sessions_help =
        isolated_help_command(scratch.path(), &["sessions", "help", "show"])?.output()?;
    let sessions_direct =
        isolated_help_command(scratch.path(), &["sessions", "show", "--help"])?.output()?;
    assert_success(&sessions_help);
    assert_success(&sessions_direct);
    assert_eq!(sessions_help.stdout, sessions_direct.stdout);

    let hidden = isolated_help_command(scratch.path(), &["daemon", "help", "serve"])?.output()?;
    assert_failure(&hidden);
    assert_eq!(hidden.status.code(), Some(2));
    assert!(hidden.stdout.is_empty());
    let hidden_stderr = String::from_utf8(hidden.stderr)?;
    assert!(hidden_stderr.contains("unrecognized public command `serve`"));
    assert!(hidden_stderr.contains("Usage: coven daemon"));
    assert!(!hidden_stderr.contains("Usage: coven daemon serve"));

    assert!(
        !scratch.path().join("coven-home").exists(),
        "nested help should not initialize COVEN_HOME state"
    );
    Ok(())
}

#[test]
fn full_help_json_has_stable_schema_routes_and_no_ansi() -> anyhow::Result<()> {
    let scratch = ScratchDir::new("json-help")?;

    let first = isolated_help_command(
        scratch.path(),
        &["--color=always", "help", "--all", "--json"],
    )?
    .output()?;
    assert_success(&first);
    let second = isolated_help_command(scratch.path(), &["help", "--all", "--json"])?.output()?;
    assert_success(&second);
    assert_eq!(first.stdout, second.stdout);

    let stdout = String::from_utf8(first.stdout)?;
    assert!(!stdout.contains('\u{1b}'));
    assert!(!stdout.contains(&scratch.path().display().to_string()));
    assert!(!stdout.contains("process-supervisor"));
    assert!(!stdout.contains("daemon serve"));

    let json: Value = serde_json::from_str(&stdout)?;
    assert_eq!(json["schemaVersion"], 1);

    let group_ids: Vec<_> = json["groups"]
        .as_array()
        .expect("groups array")
        .iter()
        .map(|group| group["id"].as_str().expect("group id"))
        .collect();
    assert_eq!(group_ids, GROUP_IDS);

    let commands: Vec<_> = json["groups"]
        .as_array()
        .expect("groups array")
        .iter()
        .flat_map(|group| group["commands"].as_array().into_iter().flatten())
        .map(|command| command["name"].as_str().expect("command name"))
        .collect();
    assert_eq!(commands, PUBLIC_COMMANDS);

    let doctor = command_from_json(&json, "doctor").expect("doctor command");
    assert_eq!(
        doctor["docsUrl"],
        "https://docs.opencoven.ai/docs/cli/doctor"
    );
    assert_eq!(
        doctor["summary"],
        "Check local setup and print next steps (exits 1 when a blocking problem is found)"
    );
    let setup = command_from_json(&json, "setup").expect("setup command");
    assert_eq!(
        setup["docsUrl"],
        "https://docs.opencoven.ai/docs/reference/cli-setup"
    );

    let status = command_from_json(&json, "status").expect("status command");
    assert_eq!(
        status["docsUrl"],
        "https://docs.opencoven.ai/docs/cli/observe"
    );

    let help = command_from_json(&json, "help").expect("help command");
    assert_eq!(
        help["docsUrl"],
        "https://docs.opencoven.ai/docs/cli/interactive"
    );
    let config = command_from_json(&json, "config").expect("config command");
    assert_eq!(
        config["docsUrl"],
        "https://docs.opencoven.ai/docs/daemon/configuration"
    );
    let engine = command_from_json(&json, "engine").expect("engine command");
    assert_eq!(
        engine["docsUrl"],
        "https://docs.opencoven.ai/docs/cli/engine-auth"
    );
    let adapter = command_from_json(&json, "adapter").expect("adapter command");
    assert_eq!(
        adapter["docsUrl"],
        "https://docs.opencoven.ai/docs/cli/repo-workflow"
    );
    let memory = command_from_json(&json, "memory").expect("memory command");
    assert_eq!(
        memory["docsUrl"],
        "https://docs.opencoven.ai/docs/memory-models"
    );
    let patch = command_from_json(&json, "patch").expect("patch command");
    assert_eq!(
        patch["docsUrl"],
        "https://docs.opencoven.ai/docs/cli/patch-openclaw"
    );
    let pc = command_from_json(&json, "pc").expect("pc command");
    assert_eq!(pc["docsUrl"], "https://docs.opencoven.ai/docs/cli/pc");
    let executor = command_from_json(&json, "executor").expect("executor command");
    assert_eq!(
        executor["docsUrl"],
        "https://docs.opencoven.ai/docs/cli/hub-scheduler"
    );
    assert!(json["groups"]
        .as_array()
        .expect("groups array")
        .iter()
        .flat_map(|group| group["commands"].as_array().into_iter().flatten())
        .all(|command| command["summary"].is_string()
            && command["docsUrl"].as_str().is_some_and(|url| {
                url.starts_with("https://docs.opencoven.ai/docs/cli/")
                    || url.starts_with("https://docs.opencoven.ai/docs/daemon/")
                    || url.starts_with("https://docs.opencoven.ai/docs/memory-models")
                    || url == "https://docs.opencoven.ai/docs/reference/cli-setup"
                    || url == "https://docs.opencoven.ai/docs/reference/troubleshooting"
                    || url == "https://docs.opencoven.ai/docs/reference/support"
                    || url == "https://docs.opencoven.ai/docs/cli"
            })));
    Ok(())
}

#[test]
fn progressive_help_treats_broken_pipe_as_success() -> anyhow::Result<()> {
    let scratch = ScratchDir::new("broken-pipe-help")?;

    for (label, args) in [
        ("command help", vec!["help", "run"]),
        ("full help", vec!["help", "--all"]),
        ("json help", vec!["help", "--all", "--json"]),
    ] {
        let output = closed_stdout_help_output(scratch.path(), &args)
            .with_context(|| format!("failed running {label} with closed stdout"))?;
        assert_success(&output);
        assert!(
            output.stdout.is_empty(),
            "{label} should not retain closed-pipe stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    Ok(())
}
