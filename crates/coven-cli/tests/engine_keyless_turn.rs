//! A keyless turn through `coven run` against a real Coven Code engine (#1790).
//!
//! The engine contract tests in `src/main.rs` run the engine directly. These
//! run it the way a user does, `coven run coven-code`, and check the part of
//! the contract that only shows end to end: the engine hands the turn to the
//! Claude CLI, and Coven's `--permission` choice arrives there as the right
//! `--permission-mode`.
//!
//! They run only when `COVEN_ENGINE_BIN` points at an engine binary, as the
//! other contract tests do, and the same filter selects them:
//!
//!   COVEN_ENGINE_BIN=<engine> cargo test -p coven-cli --locked contract
//!
//! No model runs. The engine's keyless Claude transport is pointed at a
//! stand-in (`COVEN_CODE_CLAUDE_BIN`) that records its arguments and input
//! and replays one canned turn: a shell script on Unix, a `.cmd` file on
//! Windows. Home directories, the Coven home and the project are scratch
//! directories, and credential variables are removed from the child.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const PROMPT: &str = "keyless-contract-prompt";
const TURN_TIMEOUT: Duration = Duration::from_secs(120);

/// Variables that could let a real credential, provider or engine home reach
/// the turn.
const SCRUBBED_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "OPENAI_API_KEY",
    "COVEN_CODE_PROVIDER",
    "COVEN_CODE_API_BASE",
    "COVEN_PARENT",
    "COVEN_DAEMON_SOCKET",
    "GH_TOKEN",
    "GITHUB_TOKEN",
];

const CANNED_TURN: [&str; 3] = [
    r#"{"type":"system","subtype":"init","session_id":"stand-in"}"#,
    r#"{"type":"assistant","message":{"content":[{"type":"text","text":"accepted"}]}}"#,
    r#"{"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":1,"output_tokens":1}}"#,
];

fn contract_engine_bin() -> Option<PathBuf> {
    std::env::var_os("COVEN_ENGINE_BIN").map(PathBuf::from)
}

struct Scratch {
    root: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("coven-keyless-")
            .tempdir()
            .expect("create scratch directory");
        let scratch = Self { root };
        for directory in [
            scratch.app_data("Roaming"),
            scratch.app_data("Local"),
            scratch.project(),
            scratch.bin(),
        ] {
            fs::create_dir_all(directory).expect("create scratch subdirectory");
        }
        scratch.write_stand_in();
        scratch
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }
    fn home(&self) -> PathBuf {
        self.path("home")
    }
    /// Windows application-data roots, kept inside the scratch home.
    fn app_data(&self, name: &str) -> PathBuf {
        self.home().join("AppData").join(name)
    }
    fn project(&self) -> PathBuf {
        self.path("project")
    }
    fn bin(&self) -> PathBuf {
        self.path("bin")
    }
    fn argv_log(&self) -> PathBuf {
        self.path("claude-argv.log")
    }
    fn stdin_log(&self) -> PathBuf {
        self.path("claude-stdin.log")
    }
    fn stand_in(&self) -> PathBuf {
        self.bin().join(if cfg!(windows) {
            "claude.cmd"
        } else {
            "claude"
        })
    }

    /// A stand-in for the `claude` CLI. It records its arguments and its
    /// input, then prints one canned stream-json turn.
    #[cfg(unix)]
    fn write_stand_in(&self) {
        use std::os::unix::fs::PermissionsExt;

        let mut script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {argv}\ncat > {stdin}\n",
            argv = sh_quote(&self.argv_log()),
            stdin = sh_quote(&self.stdin_log()),
        );
        for frame in CANNED_TURN {
            script.push_str(&format!("printf '%s\\n' '{frame}'\n"));
        }
        let path = self.stand_in();
        fs::write(&path, script).expect("write the claude stand-in");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("make the claude stand-in executable");
    }

    #[cfg(windows)]
    fn write_stand_in(&self) {
        // `findstr "^"` copies standard input to the log and ends when the
        // engine closes it. The canned frames hold no character that cmd.exe
        // treats specially in an `echo`.
        let mut script = format!(
            "@echo off\r\n>\"{argv}\" echo %*\r\nfindstr \"^\" >\"{stdin}\"\r\n",
            argv = self.argv_log().display(),
            stdin = self.stdin_log().display(),
        );
        for frame in CANNED_TURN {
            script.push_str(&format!("echo {frame}\r\n"));
        }
        fs::write(self.stand_in(), script).expect("write the claude stand-in");
    }
}

#[cfg(unix)]
fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

struct Turn {
    status: String,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    claude_argv: Option<String>,
    claude_stdin: Option<String>,
}

impl Turn {
    /// Everything observed, for a failure message.
    fn report(&self) -> String {
        format!(
            "coven exit: {}\n--- coven stdout ---\n{}\n--- coven stderr ---\n{}\n--- claude argv ---\n{}\n--- claude stdin ---\n{}",
            self.status,
            self.stdout,
            self.stderr,
            self.claude_argv.as_deref().unwrap_or("(the stand-in was not called)"),
            self.claude_stdin.as_deref().unwrap_or("(none recorded)"),
        )
    }
}

fn run_turn(engine: &Path, permission: &str) -> Turn {
    let scratch = Scratch::new();
    let home = scratch.home();
    let stdout_path = scratch.path("coven-stdout.log");
    let stderr_path = scratch.path("coven-stderr.log");

    let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
    command
        .args([
            "run",
            "coven-code",
            "--stream-json",
            "--permission",
            permission,
            PROMPT,
        ])
        .current_dir(scratch.project())
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("APPDATA", scratch.app_data("Roaming"))
        .env("LOCALAPPDATA", scratch.app_data("Local"))
        .env("COVEN_HOME", home.join(".coven"))
        .env("COVEN_CODE_HOME", home.join(".coven-code"))
        .env("COVEN_ENGINE_BIN", engine)
        .env("COVEN_CODE_CLAUDE_BIN", scratch.stand_in())
        .env("NO_COLOR", "1")
        // Nothing in this turn needs the network; make a stray request fail fast.
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .stdin(Stdio::null())
        .stdout(File::create(&stdout_path).expect("create the stdout log"))
        .stderr(File::create(&stderr_path).expect("create the stderr log"));
    for name in SCRUBBED_ENV {
        command.env_remove(name);
    }

    let mut child = command.spawn().expect("spawn the coven binary");
    let started = Instant::now();
    let (status, exit_code) = loop {
        match child.try_wait().expect("poll the coven process") {
            Some(status) => break (status.to_string(), status.code()),
            None if started.elapsed() >= TURN_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break (
                    format!("killed after {}s without exiting", TURN_TIMEOUT.as_secs()),
                    None,
                );
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    Turn {
        status,
        exit_code,
        stdout: fs::read_to_string(&stdout_path).unwrap_or_default(),
        stderr: fs::read_to_string(&stderr_path).unwrap_or_default(),
        claude_argv: fs::read_to_string(scratch.argv_log()).ok(),
        claude_stdin: fs::read_to_string(scratch.stdin_log()).ok(),
    }
}

/// Runs one keyless turn and requires that it succeeded, that the stand-in
/// received the prompt, and that it received `expected_mode`.
fn assert_keyless_turn(permission: &str, expected_mode: &str) {
    let Some(engine) = contract_engine_bin() else {
        eprintln!("contract: skipped (COVEN_ENGINE_BIN unset)");
        return;
    };
    let turn = run_turn(&engine, permission);
    let report = turn.report();

    let argv = turn
        .claude_argv
        .as_deref()
        .unwrap_or_else(|| panic!("the engine did not call the claude stand-in\n{report}"));
    let arguments: Vec<&str> = argv.split_whitespace().collect();
    let mode = arguments
        .iter()
        .position(|argument| *argument == "--permission-mode")
        .and_then(|index| arguments.get(index + 1))
        .copied();
    assert_eq!(
        mode,
        Some(expected_mode),
        "--permission {permission} must reach claude as --permission-mode {expected_mode}\n{report}"
    );

    let delivered = argv.contains(PROMPT)
        || turn
            .claude_stdin
            .as_deref()
            .is_some_and(|input| input.contains(PROMPT));
    assert!(
        delivered,
        "the prompt did not reach the claude stand-in\n{report}"
    );

    assert_eq!(turn.exit_code, Some(0), "coven run must exit 0\n{report}");
    let results: Vec<serde_json::Value> = turn
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|frame| frame.get("type").and_then(|kind| kind.as_str()) == Some("result"))
        .collect();
    let last = results
        .last()
        .unwrap_or_else(|| panic!("coven run printed no result frame\n{report}"));
    assert_eq!(
        (
            last.get("subtype").and_then(|value| value.as_str()),
            last.get("is_error").and_then(|value| value.as_bool()),
        ),
        (Some("success"), Some(false)),
        "the last result frame must report success\n{report}"
    );
}

#[test]
fn contract_keyless_turn_with_full_permission_reaches_claude_as_bypass_permissions() {
    assert_keyless_turn("full", "bypassPermissions");
}

#[test]
fn contract_keyless_turn_with_read_only_permission_reaches_claude_as_plan() {
    assert_keyless_turn("read-only", "plan");
}
