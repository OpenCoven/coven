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

//! Unix fixtures use shell scripts. Windows fixtures use npm-style `.cmd`
//! shims and a native argv recorder, including the validated Codex npm layout.
//! The recorder observes tokens after shell parsing, rather than echoing `%*`
//! and accidentally treating a split argument as successful forwarding.

use std::ffi::OsString;
use std::fs;
#[cfg(unix)]
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
#[cfg(unix)]
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

#[cfg(windows)]
fn write_recording_harness(
    bin_dir: &Path,
    name: &str,
    record: &Path,
    exit_code: i32,
    hold: bool,
) -> anyhow::Result<()> {
    // Compile once for the parallel suite; each invocation has its own record
    // and exit settings, so no process-wide environment or shared output races.
    static PROBE: std::sync::OnceLock<Result<(tempfile::TempDir, PathBuf), String>> =
        std::sync::OnceLock::new();
    let probe = PROBE.get_or_init(|| {
        let build = || -> anyhow::Result<(tempfile::TempDir, PathBuf)> {
            let temp = tempfile::tempdir()?;
            let binary = temp.path().join("parity-probe.exe");
            let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc.exe".into());
            let output = Command::new(rustc)
                .arg("--edition=2021")
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/harness_parity_probe.rs"),
                )
                .arg("-o")
                .arg(&binary)
                .output()?;
            anyhow::ensure!(
                output.status.success(),
                "native parity probe compilation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            Ok((temp, binary))
        };
        build().map_err(|error| format!("{error:#}"))
    });
    let (_, probe) = probe.as_ref().map_err(|error| anyhow::anyhow!("{error}"))?;
    if name == "coven-code" {
        // Coven resolves the shared claim directory through Git before launch.
        // Proxy only Git, rather than exposing the inherited provider PATH.
        let output = Command::new("where.exe").arg("git.exe").output()?;
        anyhow::ensure!(
            output.status.success(),
            "Git is required by the parity fixture"
        );
        let paths = String::from_utf8(output.stdout)?;
        let git = paths
            .lines()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Git not found"))?;
        fs::copy(probe, bin_dir.join("git.exe"))?;
        fs::write(bin_dir.join("git.settings"), format!("git\n{git}\n"))?;
    }
    let native = if name == "codex" {
        // Noninteractive Codex resolves its official npm shim to this native
        // executable. A fake arbitrary batch file must remain rejected.
        let package = bin_dir.join("node_modules/@openai/codex");
        fs::create_dir_all(package.join("bin"))?;
        fs::write(package.join("bin/codex.js"), "// hermetic npm entry\n")?;
        let (target, cpu, triple) = if cfg!(target_arch = "aarch64") {
            ("codex-win32-arm64", "arm64", "aarch64-pc-windows-msvc")
        } else {
            ("codex-win32-x64", "x64", "x86_64-pc-windows-msvc")
        };
        fs::write(
            package.join("package.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "@openai/codex", "bin": {"codex": "bin/codex.js"},
                "optionalDependencies": {format!("@openai/{target}"): "0.0.0"}
            }))?,
        )?;
        let target_root = bin_dir.join("node_modules/@openai").join(target);
        let native_dir = target_root.join("vendor").join(triple).join("bin");
        fs::create_dir_all(&native_dir)?;
        fs::write(
            target_root.join("package.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "@openai/codex", "os": ["win32"], "cpu": [cpu]
            }))?,
        )?;
        native_dir.join("codex.exe")
    } else {
        let native_dir = bin_dir.join("recorders");
        fs::create_dir_all(&native_dir)?;
        native_dir.join(format!("{name}.exe"))
    };
    fs::copy(probe, &native)?;
    // Settings are stored beside each native recorder, including Codex's
    // nested executable, so concurrent fixtures never share mutable state.
    fs::write(
        native.with_extension("settings"),
        format!("{exit_code}\n{hold}\n{}", record.display()),
    )?;
    let shim = if name == "codex" {
        "@echo off\r\n\"%~dp0\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n".to_owned()
    } else {
        format!("@echo off\r\n\"%~dp0recorders\\{name}.exe\" %*\r\n")
    };
    fs::write(bin_dir.join(format!("{name}.cmd")), shim)?;
    Ok(())
}

#[cfg(windows)]
fn hermetic_path(bin_dir: &Path) -> OsString {
    // Only Windows system commands accompany the fakes; no real provider CLI
    // may be resolved through the developer's inherited PATH.
    let system = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"));
    std::env::join_paths([bin_dir.to_path_buf(), system.join("System32"), system])
        .expect("valid fixture PATH")
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
#[cfg(unix)]
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
        let project = temp.path().join("project with spaces");
        let bin_dir = temp.path().join("bin with spaces");
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
            .env("PATHEXT", ".COM;.EXE;.BAT;.CMD")
            .env_remove("COVEN_HARNESS_ADAPTER_MANIFEST")
            .env_remove("COVEN_HARNESS_ADAPTER_DIRS")
            .env(
                "COVEN_ENGINE_BIN",
                self.bin_dir.join(if cfg!(windows) {
                    "recorders/coven-code.exe"
                } else {
                    "coven-code"
                }),
            )
            .current_dir(&self.project)
            .output()
            .map_err(Into::into)
    }

    fn prompt_received(&self, argv: &[String], prompt: &str) -> anyhow::Result<bool> {
        if argv
            .iter()
            .any(|arg| arg == prompt || arg == &format!("--prompt={prompt}"))
        {
            return Ok(true);
        }
        // Windows Codex uses stdin to keep user text out of cmd.exe argv.
        let stdin = self.record.with_extension("stdin");
        Ok(stdin.exists() && fs::read_to_string(stdin)? == prompt)
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
        let output = fixture.run(&["run", harness, "parity prompt marker with spaces"])?;
        let argv = fixture.recorded()?;
        assert!(
            fixture.prompt_received(&argv, "parity prompt marker with spaces")?,
            "{harness} never received the prompt (exit {:?}, argv {argv:?}, stderr {})",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[cfg(windows)]
#[test]
fn codex_npm_shim_refuses_a_missing_native_package() -> anyhow::Result<()> {
    let fixture = Fixture::new(0, false)?;
    fs::remove_dir_all(fixture.bin_dir.join("node_modules/@openai").join(
        if cfg!(target_arch = "aarch64") {
            "codex-win32-arm64"
        } else {
            "codex-win32-x64"
        },
    ))?;
    let output = fixture.run(&["run", "codex", "prompt"])?;
    assert!(!output.status.success());
    assert!(fixture.recorded()?.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("could not validate its native"),
        "unexpected failure: {}",
        String::from_utf8_lossy(&output.stderr)
    );
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
        let first = fixture.project.join("granted one");
        let second = fixture.project.join("granted two");
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
