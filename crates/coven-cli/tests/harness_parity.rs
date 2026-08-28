//! Hermetic three-harness contract parity.
//!
//! `coven run` promises the same surface across Codex, Claude Code, and GitHub
//! Copilot CLI, but each harness spells that surface differently — `--sandbox`
//! versus `--permission-mode`, a positional prompt versus `--prompt`. The
//! translation lives in `built_in_harness_specs`, and until now nothing proved
//! the three stay in step: a harness could quietly lose `--add-dir` forwarding
//! and only a real provider account would notice.
//!
//! These tests run the real `coven` binary against fake harness executables
//! that record their argv, so every assertion is about what Coven *actually
//! forwarded*, not about what a struct declares. No network, no provider
//! accounts, no installed CLIs.

//! Unix-only: the fake harnesses are `#!/bin/sh` scripts made executable via
//! the Unix permission bits, and Windows resolves harness executables through
//! `.cmd` shims instead. What these tests exercise -- the flag translation in
//! `built_in_harness_specs` -- is platform-independent, so Unix coverage does
//! prove the contract itself. Windows-specific *executable resolution* is
//! covered separately by `windows_daemon_lifecycle.rs` and by the Windows leg
//! of the npm onboarding smoke. Extending these fakes to `.cmd` so parity runs
//! on all three platforms is tracked as follow-up work.
#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The three built-in harnesses this release supports. Adding a fourth should
/// fail these tests until it declares the same contract.
const HARNESSES: [&str; 3] = ["codex", "claude", "copilot"];

fn coven_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_coven"))
}

/// Write a fake harness that appends its full argv to `record`, one argument
/// per line, then exits 0. `hold` makes it sleep instead so cancellation has
/// something to cancel; `exit_code` lets a case prove exit propagation.
fn write_recording_harness(
    bin_dir: &Path,
    name: &str,
    record: &Path,
    exit_code: i32,
    hold: bool,
) -> anyhow::Result<()> {
    let script = format!(
        r#"#!/bin/sh
for arg in "$@"; do
  printf '%s\n' "$arg" >> '{record}'
done
printf 'fake {name} ran\n'
{body}
exit {exit_code}
"#,
        record = record.display(),
        name = name,
        body = if hold { "sleep 300" } else { "" },
        exit_code = exit_code,
    );
    let path = bin_dir.join(name);
    fs::write(&path, script)?;
    let mut permissions = fs::metadata(&path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions)?;
    Ok(())
}

/// Every harness executable a run may resolve, so a missing fake never silently
/// falls through to a real CLI on the developer's PATH.
fn write_all_harnesses(
    bin_dir: &Path,
    record: &Path,
    exit_code: i32,
    hold: bool,
) -> anyhow::Result<()> {
    for name in ["codex", "claude", "copilot", "coven-code"] {
        write_recording_harness(bin_dir, name, record, exit_code, hold)?;
    }
    Ok(())
}

/// PATH containing only the fake harness directory plus the system basics the
/// fakes themselves need (`sh`, `sleep`, `printf`).
fn hermetic_path(bin_dir: &Path) -> OsString {
    let mut value = OsString::from(bin_dir);
    value.push(":/usr/bin:/bin");
    value
}

fn init_git_repo(repo: &Path) -> anyhow::Result<()> {
    let git = |args: &[&str]| -> anyhow::Result<()> {
        let output = Command::new("git").args(args).current_dir(repo).output()?;
        anyhow::ensure!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };
    git(&["init", "--initial-branch=main"])?;
    git(&["config", "user.name", "Parity"])?;
    git(&["config", "user.email", "parity@example.invalid"])?;
    git(&["config", "commit.gpgsign", "false"])?;
    git(&["commit", "--allow-empty", "-m", "init"])?;
    Ok(())
}

struct Fixture {
    _temp: tempfile::TempDir,
    coven_home: PathBuf,
    project: PathBuf,
    bin_dir: PathBuf,
    record: PathBuf,
}

impl Fixture {
    fn new(exit_code: i32, hold: bool) -> anyhow::Result<Self> {
        let temp = tempfile::tempdir()?;
        let coven_home = temp.path().join("coven-home");
        let project = temp.path().join("project");
        let bin_dir = temp.path().join("bin");
        let record = temp.path().join("argv.txt");
        fs::create_dir_all(&coven_home)?;
        fs::create_dir_all(&project)?;
        fs::create_dir_all(&bin_dir)?;
        fs::write(&record, "")?;
        init_git_repo(&project)?;
        write_all_harnesses(&bin_dir, &record, exit_code, hold)?;
        Ok(Self {
            _temp: temp,
            coven_home,
            project,
            bin_dir,
            record,
        })
    }

    fn run(&self, args: &[&str]) -> anyhow::Result<Output> {
        Command::new(coven_bin())
            .args(args)
            .env("COVEN_HOME", &self.coven_home)
            .env("PATH", hermetic_path(&self.bin_dir))
            .env("COVEN_ENGINE_BIN", self.bin_dir.join("coven-code"))
            .current_dir(&self.project)
            .output()
            .map_err(Into::into)
    }

    /// Argv the fake harness observed, one token per line.
    fn recorded(&self) -> anyhow::Result<Vec<String>> {
        Ok(fs::read_to_string(&self.record)?
            .lines()
            .map(str::to_owned)
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Dimension 1: prompt delivery
// ---------------------------------------------------------------------------

#[test]
fn every_harness_forwards_the_prompt() -> anyhow::Result<()> {
    for harness in HARNESSES {
        let fixture = Fixture::new(0, false)?;
        let output = fixture.run(&["run", harness, "parity-prompt-marker"])?;
        let argv = fixture.recorded()?;
        assert!(
            argv.iter().any(|arg| arg.contains("parity-prompt-marker")),
            "{harness} never received the prompt (exit {:?}, argv {argv:?}, stderr {})",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dimension 2: model selection
// ---------------------------------------------------------------------------

#[test]
fn every_harness_forwards_model_selection_with_its_native_flag() -> anyhow::Result<()> {
    // Provider-qualified id: each adapter declares whether to strip the
    // `provider/` segment. Both current mappings strip, so all three must
    // receive the bare id rather than the qualified one.
    for harness in HARNESSES {
        let fixture = Fixture::new(0, false)?;
        fixture.run(&["run", harness, "--model", "openai/gpt-5.5", "prompt"])?;
        let argv = fixture.recorded()?;
        assert!(
            argv.iter().any(|arg| arg == "--model"),
            "{harness} did not forward --model: argv {argv:?}"
        );
        assert!(
            argv.iter().any(|arg| arg == "gpt-5.5"),
            "{harness} did not strip the provider prefix: argv {argv:?}"
        );
        assert!(
            !argv.iter().any(|arg| arg == "openai/gpt-5.5"),
            "{harness} forwarded the provider-qualified id verbatim: argv {argv:?}"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dimension 3: permission / sandbox policy
// ---------------------------------------------------------------------------

#[test]
fn every_harness_maps_permission_policy_to_its_native_sandbox_flag() -> anyhow::Result<()> {
    // The flag names deliberately differ per harness; parity is that each one
    // maps BOTH policies to something, and that the two policies are distinct.
    for harness in HARNESSES {
        let full = Fixture::new(0, false)?;
        full.run(&["run", harness, "--permission", "full", "prompt"])?;
        let full_argv = full.recorded()?;

        let read_only = Fixture::new(0, false)?;
        read_only.run(&["run", harness, "--permission", "read-only", "prompt"])?;
        let read_only_argv = read_only.recorded()?;

        assert_ne!(
            full_argv, read_only_argv,
            "{harness} produced identical argv for --permission full and read-only, \
             so the policy is not reaching the harness: {full_argv:?}"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dimension 4: add-directory grants
// ---------------------------------------------------------------------------

#[test]
fn every_harness_forwards_each_add_dir_grant() -> anyhow::Result<()> {
    for harness in HARNESSES {
        let fixture = Fixture::new(0, false)?;
        let first = fixture.project.join("granted-one");
        let second = fixture.project.join("granted-two");
        fs::create_dir_all(&first)?;
        fs::create_dir_all(&second)?;
        fixture.run(&[
            "run",
            harness,
            "--add-dir",
            first.to_str().expect("utf-8 path"),
            "--add-dir",
            second.to_str().expect("utf-8 path"),
            "prompt",
        ])?;
        let argv = fixture.recorded()?;

        // Both grants must appear. A harness that forwards only the first is
        // the regression this catches: repeated flags are easy to collapse.
        for granted in [&first, &second] {
            let needle = granted.to_str().expect("utf-8 path");
            assert!(
                argv.iter().any(|arg| arg == needle),
                "{harness} dropped an --add-dir grant {needle}: argv {argv:?}"
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dimension 5: continuity (resume)
// ---------------------------------------------------------------------------

#[test]
fn every_harness_refuses_an_unsatisfiable_continue_instead_of_starting_over() -> anyhow::Result<()>
{
    // Each harness declares its own resume mechanism (Codex re-invokes through
    // `exec ... resume`, others use their own flags), so asserting one exact
    // token would only test Codex. The parity claim that holds for all three is
    // that a resumed turn must not be invoked identically to a fresh one --
    // that is precisely the regression where `--continue` is silently dropped
    // and the harness starts a brand new conversation instead.
    for harness in HARNESSES {
        let fixture = Fixture::new(0, false)?;

        fixture.run(&["run", harness, "first-turn"])?;
        let fresh = fixture.recorded()?;
        assert!(
            !fresh.is_empty(),
            "{harness} never invoked the harness on a fresh turn"
        );

        // Truncate the record so the second run's argv is isolated.
        fs::write(&fixture.record, "")?;
        let output = fixture.run(&["run", harness, "--continue", "second-turn"])?;
        let resumed = fixture.recorded()?;

        // The first turn already completed, so there is no active session to
        // resume. The dangerous failure here is not an error -- it is silently
        // starting a BRAND NEW conversation while the operator believes they
        // continued the old one. All three harnesses must refuse rather than
        // launch, and must say so rather than exiting 0.
        assert!(
            resumed.is_empty(),
            "{harness} launched a fresh conversation for --continue with no resumable \
             session, so continuity was silently downgraded: {resumed:?}"
        );
        assert!(
            !output.status.success(),
            "{harness} reported success for an unsatisfiable --continue: stdout {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dimension 6: output persistence
// ---------------------------------------------------------------------------

#[test]
fn every_harness_persists_session_output() -> anyhow::Result<()> {
    for harness in HARNESSES {
        let fixture = Fixture::new(0, false)?;
        fixture.run(&["run", harness, "persistence-marker"])?;

        let sessions = fixture.run(&["sessions", "--json"])?;
        let listed = String::from_utf8_lossy(&sessions.stdout);
        assert!(
            listed.contains(harness),
            "{harness} left no session record after a completed run: {listed}"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dimension 7: exit behaviour
// ---------------------------------------------------------------------------

#[test]
fn every_harness_propagates_a_failing_exit_code() -> anyhow::Result<()> {
    for harness in HARNESSES {
        let fixture = Fixture::new(17, false)?;
        let output = fixture.run(&["run", harness, "prompt"])?;
        assert!(
            !output.status.success(),
            "{harness} reported success for a harness that exited 17: stdout {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dimension 8: no harness silently ignores a declared flag
// ---------------------------------------------------------------------------

#[test]
fn no_harness_silently_drops_a_supported_flag() -> anyhow::Result<()> {
    // A harness that declares no mechanism for a flag must warn rather than
    // accept it silently, so an operator never believes a policy applied when
    // it did not. Parity is that the behaviour is uniform: either the flag
    // reaches argv, or the run says so.
    for harness in HARNESSES {
        let fixture = Fixture::new(0, false)?;
        let output = fixture.run(&["run", harness, "--model", "openai/gpt-5.5", "prompt"])?;
        let argv = fixture.recorded()?;
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        let forwarded = argv.iter().any(|arg| arg == "--model");
        assert!(
            forwarded || stderr.contains("warn") || stderr.contains("no model"),
            "{harness} neither forwarded --model nor warned about ignoring it: \
             argv {argv:?}, stderr {stderr}"
        );
    }
    Ok(())
}
