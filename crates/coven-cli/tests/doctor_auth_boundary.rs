use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::Value;
use sysinfo::{Pid, ProcessesToUpdate, System};

#[derive(Debug)]
struct Invocation {
    executable: String,
    pid: u32,
    args: Vec<String>,
}

struct ProbeFixture {
    _temp_dir: tempfile::TempDir,
    root: PathBuf,
    bin_dir: PathBuf,
    engine: PathBuf,
}

impl ProbeFixture {
    fn new() -> Result<Self> {
        let temp_dir = tempfile::tempdir()?;
        let root = temp_dir.path().to_path_buf();
        let bin_dir = root.join("bin");
        fs::create_dir_all(&bin_dir)?;

        let compiled = root.join(platform_executable_name("doctor-auth-probe"));
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/doctor_auth_probe.rs");
        let rustc = std::env::var_os("RUSTC")
            .unwrap_or_else(|| OsString::from(if cfg!(windows) { "rustc.exe" } else { "rustc" }));
        let compile = Command::new(rustc)
            .arg("--edition=2021")
            .arg("-o")
            .arg(&compiled)
            .arg(&source)
            .output()
            .context("failed to compile Doctor auth probe")?;
        anyhow::ensure!(
            compile.status.success(),
            "Doctor auth probe failed to compile:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );

        let engine = install_probe_as(&compiled, &bin_dir, "coven-code")?;
        for harness in ["codex", "claude", "copilot"] {
            install_probe_as(&compiled, &bin_dir, harness)?;
        }

        Ok(Self {
            _temp_dir: temp_dir,
            root,
            bin_dir,
            engine,
        })
    }

    fn run(&self, label: &str, mode: &str, json: bool) -> Result<ProbeRun> {
        let home = self.root.join(format!("home-{label}"));
        let config = self.root.join(format!("config-{label}"));
        let log = self.root.join(format!("invocations-{label}.log"));
        let descendant_pid_file = self.root.join(format!("descendant-{label}.pid"));
        fs::create_dir_all(&home)?;
        fs::create_dir_all(&config)?;

        let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
        command
            .arg("doctor")
            .env("COVEN_HOME", &home)
            .env("COVEN_ENGINE_BIN", &self.engine)
            .env("COVEN_DOCTOR_PROBE_LOG", &log)
            .env("COVEN_DOCTOR_PROBE_MODE", mode)
            .env("COVEN_DOCTOR_DESCENDANT_PID_FILE", &descendant_pid_file)
            .env("PATH", path_containing_only(&self.bin_dir)?)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("XDG_CONFIG_HOME", &config)
            .env_remove("COVEN_HARNESS_ADAPTER_MANIFEST")
            .env_remove("COVEN_HARNESS_ADAPTER_DIRS");
        if cfg!(windows) {
            command.env("PATHEXT", ".EXE");
        }
        if json {
            command.arg("--json");
        }

        let started = Instant::now();
        let output = command
            .output()
            .context("run coven doctor with auth probe")?;
        let elapsed = started.elapsed();
        let invocations = read_invocations(&log)?;
        let descendant_pid = fs::read_to_string(&descendant_pid_file)
            .ok()
            .and_then(|value| value.trim().parse().ok());
        Ok(ProbeRun {
            output,
            elapsed,
            invocations,
            descendant_pid,
        })
    }
}

struct ProbeRun {
    output: Output,
    elapsed: Duration,
    invocations: Vec<Invocation>,
    descendant_pid: Option<u32>,
}

#[test]
fn doctor_auth_boundary_is_offline_hermetic_and_failure_bounded() -> Result<()> {
    let fixture = ProbeFixture::new()?;

    let configured = fixture.run("configured", "configured", true)?;
    assert_success("configured auth", &configured.output);
    let configured_json = parse_json("configured auth", &configured.output)?;
    assert_eq!(credential_status(&configured_json, "engine"), "warn");
    assert_eq!(
        credential_message(&configured_json, "engine"),
        "authentication configured; provider turn not verified"
    );
    for harness in ["codex", "claude", "copilot"] {
        assert_eq!(harness_status(&configured_json, harness), "pass");
        assert_eq!(credential_status(&configured_json, harness), "warn");
    }
    assert_eq!(
        credential_hint(&configured_json, "codex"),
        "authenticate or inspect local setup with: codex login; verify provider access with an explicitly authorized test turn"
    );
    assert_only_local_engine_probes(&configured.invocations);

    let unconfigured = fixture.run("unconfigured", "unconfigured", false)?;
    assert_success("unconfigured auth", &unconfigured.output);
    let prose = String::from_utf8_lossy(&unconfigured.output.stdout);
    assert!(
        prose.contains("[--] Coven Code (engine) — authentication not configured"),
        "unconfigured engine auth must be advisory:\n{prose}"
    );
    assert!(
        !prose.contains("[!!] Coven Code (engine)"),
        "unconfigured engine auth must not look blocking:\n{prose}"
    );
    assert!(
        prose.contains("verify provider access with an explicitly authorized test turn"),
        "setup commands must not be presented as provider verification:\n{prose}"
    );
    assert_only_local_engine_probes(&unconfigured.invocations);

    let exit_two = fixture.run("exit-two", "exit2", true)?;
    assert_success("non-contract auth exit", &exit_two.output);
    let exit_two_json = parse_json("non-contract auth exit", &exit_two.output)?;
    assert_eq!(credential_status(&exit_two_json, "engine"), "warn");
    assert_eq!(
        credential_message(&exit_two_json, "engine"),
        "authentication status unavailable; provider turn not verified"
    );
    assert_only_local_engine_probes(&exit_two.invocations);

    let timed_out = fixture.run("timeout", "timeout", true)?;
    assert_success("timed-out auth probe", &timed_out.output);
    anyhow::ensure!(
        timed_out.elapsed < Duration::from_secs(12),
        "Doctor did not enforce its five-second auth timeout: {:?}",
        timed_out.elapsed
    );
    let timeout_json = parse_json("timed-out auth probe", &timed_out.output)?;
    assert_eq!(credential_status(&timeout_json, "engine"), "warn");
    assert_eq!(
        credential_message(&timeout_json, "engine"),
        "authentication status unavailable; provider turn not verified"
    );
    assert_only_local_engine_probes(&timed_out.invocations);
    let timeout_pid = timed_out
        .invocations
        .iter()
        .find(|invocation| invocation.args == ["auth", "status", "--json"])
        .expect("timed-out auth invocation")
        .pid;
    assert_process_stopped(timeout_pid, "timed-out auth probe");

    let assert_descendant = |label: &str| -> Result<()> {
        let descendant = fixture.run(label, "descendant", true)?;
        assert_success("stdout-holding descendant", &descendant.output);
        anyhow::ensure!(
            descendant.elapsed < Duration::from_secs(12),
            "a descendant-held stdout pipe escaped Doctor's bound: {:?}",
            descendant.elapsed
        );
        let descendant_json = parse_json("stdout-holding descendant", &descendant.output)?;
        assert_eq!(credential_status(&descendant_json, "engine"), "warn");
        assert_eq!(
            credential_message(&descendant_json, "engine"),
            "authentication configured; provider turn not verified"
        );
        assert_only_local_engine_probes(&descendant.invocations);
        let descendant_parent_pid = descendant
            .invocations
            .iter()
            .find(|invocation| invocation.args == ["auth", "status", "--json"])
            .expect("descendant-mode auth invocation")
            .pid;
        let descendant_pid = descendant
            .descendant_pid
            .context("probe did not record stdout-holding descendant pid")?;
        assert_process_stopped(descendant_parent_pid, "descendant-mode engine parent");
        assert_process_stopped(descendant_pid, "stdout-holding engine descendant");
        Ok(())
    };
    assert_descendant("descendant")?;
    #[cfg(windows)]
    for attempt in 0..3 {
        assert_descendant(&format!("descendant-stress-{attempt}"))?;
    }

    Ok(())
}

fn platform_executable_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

fn install_probe_as(compiled: &Path, bin_dir: &Path, name: &str) -> Result<PathBuf> {
    let destination = bin_dir.join(platform_executable_name(name));
    fs::copy(compiled, &destination)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o755))?;
    }
    Ok(destination)
}

fn path_containing_only(bin_dir: &Path) -> Result<OsString> {
    std::env::join_paths([bin_dir]).context("construct isolated probe PATH")
}

fn read_invocations(path: &Path) -> Result<Vec<Invocation>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read invocation log {}", path.display()))?;
    text.lines()
        .map(|line| {
            let mut fields = line.splitn(3, '\t');
            let executable = fields.next().unwrap_or_default().to_string();
            let pid = fields
                .next()
                .context("invocation log missing pid")?
                .parse::<u32>()
                .context("invocation log pid is not numeric")?;
            let args = fields
                .next()
                .unwrap_or_default()
                .split('\u{1f}')
                .filter(|arg| !arg.is_empty())
                .map(str::to_string)
                .collect();
            Ok(Invocation {
                executable,
                pid,
                args,
            })
        })
        .collect()
}

fn assert_only_local_engine_probes(invocations: &[Invocation]) {
    assert_eq!(
        invocations.len(),
        2,
        "Doctor must run exactly the two local engine probes: {invocations:#?}"
    );
    assert!(
        invocations
            .iter()
            .all(|invocation| invocation.executable == "coven-code"),
        "Doctor must not launch a provider harness: {invocations:#?}"
    );
    assert!(
        invocations
            .iter()
            .any(|invocation| invocation.args == ["--version"]),
        "missing engine version probe: {invocations:#?}"
    );
    assert!(
        invocations
            .iter()
            .any(|invocation| invocation.args == ["auth", "status", "--json"]),
        "missing local engine auth probe: {invocations:#?}"
    );
}

fn parse_json(label: &str, output: &Output) -> Result<Value> {
    serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "{label} did not emit one JSON document\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn check<'a>(document: &'a Value, id: &str) -> &'a Value {
    document["checks"]
        .as_array()
        .expect("Doctor checks array")
        .iter()
        .find(|check| check["id"] == id)
        .unwrap_or_else(|| panic!("missing Doctor check {id}: {document}"))
}

fn harness_status<'a>(document: &'a Value, harness: &str) -> &'a str {
    check(document, &format!("harness:{harness}"))["status"]
        .as_str()
        .expect("harness status string")
}

fn credential_status<'a>(document: &'a Value, harness: &str) -> &'a str {
    check(document, &format!("credentials:{harness}"))["status"]
        .as_str()
        .expect("credential status string")
}

fn credential_message<'a>(document: &'a Value, harness: &str) -> &'a str {
    check(document, &format!("credentials:{harness}"))["message"]
        .as_str()
        .expect("credential message string")
}

fn credential_hint<'a>(document: &'a Value, harness: &str) -> &'a str {
    check(document, &format!("credentials:{harness}"))["hint"]
        .as_str()
        .expect("credential hint string")
}

fn assert_success(label: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{label} failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn process_is_running(pid: u32) -> bool {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).is_some()
}

fn assert_process_stopped(pid: u32, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while process_is_running(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_is_running(pid),
        "Doctor returned without stopping {label} pid {pid}"
    );
}
