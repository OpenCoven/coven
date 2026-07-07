#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

#[test]
fn daemon_status_clears_stale_metadata_when_daemon_is_gone() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    fs::write(
        coven_home.join("daemon.json"),
        r#"{
  "pid": 999999,
  "startedAt": "2026-01-01T00:00:00Z",
  "socket": "/tmp/does-not-exist.sock"
}
"#,
    )?;

    let output = run_coven(
        &coven_bin(),
        &coven_home,
        &std::env::var_os("PATH").unwrap_or_default(),
        &["daemon", "status"],
    )?;

    assert_success("daemon status with stale metadata", &output);
    assert_stdout_contains(
        "daemon status with stale metadata",
        &output,
        "status=stopped",
    );
    assert!(
        !coven_home.join("daemon.json").exists(),
        "stale daemon metadata should be cleared"
    );
    Ok(())
}

#[test]
fn daemon_status_clears_corrupt_metadata_when_daemon_is_gone() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    fs::write(coven_home.join("daemon.json"), "{not json\n")?;

    let output = run_coven(
        &coven_bin(),
        &coven_home,
        &std::env::var_os("PATH").unwrap_or_default(),
        &["daemon", "status"],
    )?;

    assert_success("daemon status with corrupt metadata", &output);
    assert_stdout_contains(
        "daemon status with corrupt metadata",
        &output,
        "status=stopped",
    );
    assert!(
        !coven_home.join("daemon.json").exists(),
        "corrupt daemon metadata should be cleared"
    );
    Ok(())
}

#[test]
fn daemon_status_recovers_corrupt_metadata_from_live_daemon_health() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let start = run_coven(&coven, &coven_home, &path, &["daemon", "start"])?;
    assert_success("daemon start", &start);
    wait_for_daemon_health(&coven_home)?;

    let status_path = coven_home.join("daemon.json");
    let original_status = fs::read_to_string(&status_path)?;
    let _restore_guard = DaemonStatusRestoreGuard {
        path: status_path.clone(),
        contents: original_status,
    };
    fs::write(&status_path, "{not json\n")?;

    let output = run_coven(&coven, &coven_home, &path, &["daemon", "status"])?;

    assert_success("daemon status with live corrupt metadata", &output);
    assert_stdout_contains(
        "daemon status with live corrupt metadata",
        &output,
        "status=running",
    );
    let recovered = fs::read_to_string(&status_path)?;
    let recovered: Value = serde_json::from_str(&recovered)?;
    assert!(
        recovered.get("pid").and_then(Value::as_u64).is_some(),
        "daemon status metadata should be restored from health"
    );
    Ok(())
}

#[test]
fn daemon_start_is_idempotent_when_daemon_is_already_running() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let first = run_coven(&coven, &coven_home, &path, &["daemon", "start"])?;
    assert_success("first daemon start", &first);
    wait_for_daemon_health(&coven_home)?;
    let first_pid = daemon_status_pid(&coven_home)?;

    let second = run_coven(&coven, &coven_home, &path, &["daemon", "start"])?;
    assert_success("second daemon start", &second);
    wait_for_daemon_health(&coven_home)?;
    let second_pid = daemon_status_pid(&coven_home)?;

    assert_eq!(
        second_pid, first_pid,
        "daemon start should reuse the verified running daemon instead of spawning another serve process"
    );
    Ok(())
}

#[test]
fn concurrent_daemon_start_commands_share_one_daemon() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let first = Command::new(&coven)
        .args(["daemon", "start"])
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .spawn()?;
    let second = Command::new(&coven)
        .args(["daemon", "start"])
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .spawn()?;

    let first = first.wait_with_output()?;
    let second = second.wait_with_output()?;
    assert_success("first concurrent daemon start", &first);
    assert_success("second concurrent daemon start", &second);
    wait_for_daemon_health(&coven_home)?;

    let recovery_log = fs::read_to_string(coven_home.join("daemon-recovery.log"))?;
    let starts = recovery_log.matches("daemon starting pid=").count();
    assert_eq!(
        starts, 1,
        "concurrent daemon start commands should launch exactly one serve process\n{recovery_log}"
    );
    Ok(())
}

#[test]
fn daemon_serve_refuses_to_take_over_a_healthy_socket() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let first = run_coven(&coven, &coven_home, &path, &["daemon", "start"])?;
    assert_success("first daemon start", &first);
    wait_for_daemon_health(&coven_home)?;
    let first_pid = daemon_status_pid(&coven_home)?;

    // A second `daemon serve` against the live socket must refuse to take over.
    // Unlinking the incumbent's socket would not stop it — it would keep running
    // on the orphaned inode — so the duplicate has to exit on its own instead.
    let mut duplicate = Command::new(&coven)
        .args(["daemon", "serve"])
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    wait_until("duplicate daemon to exit on its own", || {
        Ok(duplicate.try_wait()?.is_some())
    })?;
    let duplicate_status = duplicate.wait()?;
    assert!(
        !duplicate_status.success(),
        "duplicate serve should fail rather than take over the live socket"
    );

    // The incumbent is untouched: still the recorded owner, still healthy, and the
    // refused duplicate must not have clobbered daemon.json on its way out.
    wait_for_daemon_health(&coven_home)?;
    assert_eq!(
        first_pid,
        daemon_status_pid(&coven_home)?,
        "incumbent must remain the recorded socket owner after a refused takeover"
    );
    assert!(
        pid_is_alive(first_pid as u32),
        "incumbent daemon must stay alive after a refused takeover"
    );
    Ok(())
}

#[test]
fn daemon_status_json_reports_stopped_daemon_on_pure_stdout() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;

    let output = run_coven(
        &coven_bin(),
        &coven_home,
        &std::env::var_os("PATH").unwrap_or_default(),
        &["daemon", "status", "--json"],
    )?;

    assert_success("daemon status --json when stopped", &output);
    // Parsing the entire stdout proves it carries only the JSON document.
    let value = parse_stdout_json("daemon status --json when stopped", &output)?;
    assert_eq!(value.get("status").and_then(Value::as_str), Some("stopped"));
    assert_eq!(value.get("ok").and_then(Value::as_bool), Some(false));
    assert!(value.get("pid").is_some_and(Value::is_null));
    assert!(value.get("socket").is_some_and(Value::is_null));
    assert!(value.get("started_at").is_some_and(Value::is_null));
    // The human hint stays on stderr so stdout remains parseable.
    assert_stderr_contains(
        "daemon status --json when stopped",
        &output,
        "coven daemon start",
    );
    Ok(())
}

#[test]
fn wt_list_and_claim_status_emit_machine_readable_json() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    let repo = temp_dir.path().join("repo");
    fs::create_dir_all(&repo)?;
    init_git_repo(&repo)?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();
    let agent_env = [("COVEN_AGENT_ID", "smoke-agent")];

    let acquire = run_coven_in(
        &coven,
        &coven_home,
        &path,
        &repo,
        &agent_env,
        &["claim", "acquire", "smoke-branch"],
    )?;
    assert_success("claim acquire", &acquire);

    let status_json = run_coven_in(
        &coven,
        &coven_home,
        &path,
        &repo,
        &agent_env,
        &["claim", "status", "--json"],
    )?;
    assert_success("claim status --json", &status_json);
    let value = parse_stdout_json("claim status --json", &status_json)?;
    let claims = value
        .get("claims")
        .and_then(Value::as_array)
        .expect("claim status JSON should include a claims array");
    assert_eq!(claims.len(), 1);
    let claim = &claims[0];
    assert_eq!(
        claim.get("branch").and_then(Value::as_str),
        Some("smoke-branch")
    );
    assert_eq!(
        claim.get("agent_id").and_then(Value::as_str),
        Some("smoke-agent")
    );
    assert_eq!(claim.get("state").and_then(Value::as_str), Some("active"));
    assert!(
        claim.get("acquired_at").and_then(Value::as_u64).is_some(),
        "claims JSON should keep the raw epoch value"
    );
    assert!(claim.get("expires_at").and_then(Value::as_u64).is_some());
    let expires_rfc3339 = claim
        .get("expires_at_rfc3339")
        .and_then(Value::as_str)
        .expect("claims JSON should include an RFC 3339 expiry")
        .to_string();
    assert!(
        expires_rfc3339.contains('T') && expires_rfc3339.ends_with('Z'),
        "expected RFC 3339 UTC expiry, got {expires_rfc3339:?}"
    );

    // The human table renders the same expiry as a readable timestamp, not
    // raw epoch seconds.
    let status_human = run_coven_in(
        &coven,
        &coven_home,
        &path,
        &repo,
        &agent_env,
        &["claim", "status"],
    )?;
    assert_success("claim status", &status_human);
    assert_stdout_contains("claim status", &status_human, &expires_rfc3339);
    let raw_epoch = claim
        .get("expires_at")
        .and_then(Value::as_u64)
        .expect("expires_at epoch")
        .to_string();
    assert_stdout_not_contains("claim status", &status_human, &raw_epoch);

    let wt_json = run_coven_in(
        &coven,
        &coven_home,
        &path,
        &repo,
        &agent_env,
        &["wt", "--list", "--json"],
    )?;
    assert_success("wt --list --json", &wt_json);
    let value = parse_stdout_json("wt --list --json", &wt_json)?;
    let worktrees = value
        .get("worktrees")
        .and_then(Value::as_array)
        .expect("wt --list JSON should include a worktrees array");
    assert!(!worktrees.is_empty(), "primary worktree should be listed");
    let worktree = &worktrees[0];
    assert!(worktree.get("branch").and_then(Value::as_str).is_some());
    assert!(worktree.get("dirty").and_then(Value::as_bool).is_some());
    assert!(worktree.get("claimed_by").is_some());
    assert!(worktree.get("path").and_then(Value::as_str).is_some());
    Ok(())
}

#[test]
fn pc_top_and_disk_emit_machine_readable_json() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();

    let top = run_coven(
        &coven,
        &coven_home,
        &path,
        &["pc", "top", "--n", "3", "--json"],
    )?;
    assert_success("pc top --json", &top);
    let value = parse_stdout_json("pc top --json", &top)?;
    let processes = value
        .get("processes")
        .and_then(Value::as_array)
        .expect("pc top JSON should include a processes array");
    assert!(processes.len() <= 3, "--n should cap the process list");
    if let Some(process) = processes.first() {
        assert!(process.get("pid").and_then(Value::as_u64).is_some());
        assert!(process.get("name").and_then(Value::as_str).is_some());
        assert!(process.get("cpu_pct").and_then(Value::as_f64).is_some());
        assert!(process.get("memory_mb").and_then(Value::as_u64).is_some());
    }

    let disk = run_coven(&coven, &coven_home, &path, &["pc", "disk", "--json"])?;
    assert_success("pc disk --json", &disk);
    let value = parse_stdout_json("pc disk --json", &disk)?;
    let disks = value
        .get("disks")
        .and_then(Value::as_array)
        .expect("pc disk JSON should include a disks array");
    if let Some(disk) = disks.first() {
        assert!(disk.get("mount").and_then(Value::as_str).is_some());
        assert!(disk.get("total_gb").and_then(Value::as_f64).is_some());
        assert!(disk.get("available_gb").and_then(Value::as_f64).is_some());
        assert!(disk.get("used_pct").and_then(Value::as_f64).is_some());
    }
    Ok(())
}

#[test]
fn doctor_lists_configured_familiars() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    fs::write(
        coven_home.join("familiars.toml"),
        r#"
[[familiar]]
id = "charm"
display_name = "Charm"
role = "Voice, Social, and Presence Familiar"
description = "Keeps the coven sociable."
"#,
    )?;
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin)?;
    write_fake_codex(&fake_bin)?;
    write_fake_coven_code(&fake_bin)?;
    let path = prepend_path(&fake_bin);
    let coven = coven_bin();

    let output = run_coven(&coven, &coven_home, &path, &["doctor"])?;

    assert_success("doctor with familiars", &output);
    assert_stdout_contains("doctor with familiars", &output, "Familiars (");
    assert_stdout_contains("doctor with familiars", &output, "charm");
    assert_stdout_contains(
        "doctor with familiars",
        &output,
        "Voice, Social, and Presence Familiar",
    );
    Ok(())
}

#[test]
fn doctor_reports_no_familiars_when_manifest_absent() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin)?;
    write_fake_codex(&fake_bin)?;
    write_fake_coven_code(&fake_bin)?;
    let path = prepend_path(&fake_bin);
    let coven = coven_bin();

    let output = run_coven(&coven, &coven_home, &path, &["doctor"])?;

    assert_success("doctor without familiars", &output);
    assert_stdout_contains("doctor without familiars", &output, "none configured");
    assert_stdout_contains("doctor without familiars", &output, "familiars.toml");
    Ok(())
}

#[test]
fn doctor_missing_harness_prints_cross_platform_setup_loop() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    let coven = coven_bin();
    let empty_path = OsString::new();

    let output = run_coven(&coven, &coven_home, &empty_path, &["doctor"])?;

    // No harness available is a blocking problem: doctor must exit 1 so
    // scripts can gate on it, while still printing the full setup loop.
    assert_failure("doctor without harnesses", &output);
    assert_stdout_contains(
        "doctor without harnesses",
        &output,
        "Doctor found problems; review the failing checks above.",
    );
    assert_stdout_contains("doctor without harnesses", &output, "Harnesses:");
    assert_stdout_contains("doctor without harnesses", &output, "`codex` is missing");
    assert_stdout_contains("doctor without harnesses", &output, "`claude` is missing");
    assert_stdout_contains(
        "doctor without harnesses",
        &output,
        "Install and authenticate at least one harness in this same shell.",
    );
    assert_stdout_contains(
        "doctor without harnesses",
        &output,
        "If PATH changed, open a new terminal and run `coven doctor` again.",
    );
    assert_stdout_contains("doctor without harnesses", &output, "coven daemon start");
    assert_stdout_contains(
        "doctor without harnesses",
        &output,
        "Install docs: https://github.com/OpenCoven/coven/blob/main/docs/install/index.md",
    );
    Ok(())
}

#[test]
fn doctor_reports_live_daemon_socket_status() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin)?;
    write_fake_codex(&fake_bin)?;
    write_fake_coven_code(&fake_bin)?;
    let path = prepend_path(&fake_bin);
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let start = run_coven(&coven, &coven_home, &path, &["daemon", "start"])?;
    assert_success("daemon start", &start);
    wait_for_daemon_health(&coven_home)?;

    let output = run_coven(&coven, &coven_home, &path, &["doctor"])?;

    assert_success("doctor with live daemon", &output);
    assert_stdout_contains("doctor with live daemon", &output, "Daemon:");
    assert_stdout_contains("doctor with live daemon", &output, "status=running");
    assert_stdout_contains("doctor with live daemon", &output, "socket=");
    Ok(())
}

#[test]
fn completions_generate_for_supported_shells() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();

    let zsh = run_coven(&coven, &coven_home, &path, &["completions", "zsh"])?;
    assert_success("completions zsh", &zsh);
    assert_stdout_contains("completions zsh", &zsh, "#compdef coven");

    let bash = run_coven(&coven, &coven_home, &path, &["completions", "bash"])?;
    assert_success("completions bash", &bash);
    assert_stdout_contains("completions bash", &bash, "complete");

    let bogus = run_coven(&coven, &coven_home, &path, &["completions", "tcsh"])?;
    assert_failure("completions tcsh", &bogus);
    Ok(())
}

#[test]
fn color_flag_parses_and_rejects_unknown_values() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();

    // Global flag composes with subcommands without disturbing them.
    let sessions = run_coven(
        &coven,
        &coven_home,
        &path,
        &["sessions", "--color", "never", "--plain"],
    )?;
    assert_success("sessions --color never", &sessions);

    // Root-level placement parses as the declared flag, not as a prompt —
    // the front-door catch-all must not swallow it.
    let root = run_coven(
        &coven,
        &coven_home,
        &path,
        &["--color", "never", "sessions", "--plain"],
    )?;
    assert_success("--color before subcommand", &root);

    let bogus = run_coven(
        &coven,
        &coven_home,
        &path,
        &["sessions", "--color", "sometimes"],
    )?;
    assert_failure("--color rejects unknown value", &bogus);
    assert_stderr_contains("--color rejects unknown value", &bogus, "sometimes");
    Ok(())
}

#[test]
fn piped_run_output_has_no_eof_control_artifact() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    let project = temp_dir.path().join("project");
    fs::create_dir_all(&project)?;
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin)?;
    write_fake_codex(&fake_bin)?;
    let path = prepend_path(&fake_bin);

    // stdin is /dev/null (a pipe/redirect, not a TTY): a one-shot run reads
    // its prompt from argv, so nothing should be forwarded into the PTY and
    // the line discipline must not echo an EOF as a visible `^D`.
    let output = Command::new(coven_bin())
        .args(["run", "codex", "hello polish"])
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .current_dir(&project)
        .stdin(Stdio::null())
        .output()?;

    assert_success("piped run", &output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("fake codex complete"),
        "harness output should reach stdout: {stdout:?}"
    );
    assert!(
        !output.stdout.contains(&0x04),
        "piped run stdout must not contain a raw EOT (^D) byte: {stdout:?}"
    );
    assert!(
        !stdout.contains("^D"),
        "piped run stdout must not contain a visible ^D artifact: {stdout:?}"
    );
    Ok(())
}

#[test]
fn adapter_install_hermes_writes_trusted_manifest() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();

    let install = run_coven(
        &coven,
        &coven_home,
        &path,
        &["adapter", "install", "hermes"],
    )?;

    assert_success("adapter install hermes", &install);
    assert_stdout_contains(
        "adapter install hermes",
        &install,
        "Installed adapter `hermes`",
    );
    assert!(coven_home.join("adapters").join("hermes.json").exists());

    // Diagnose against an empty PATH so the outcome doesn't depend on
    // whether a real `hermes` happens to be installed on this machine:
    // unavailable → exit 1, with the diagnosis output still rendered in full.
    let doctor = run_coven(
        &coven,
        &coven_home,
        &OsString::new(),
        &["adapter", "doctor", "hermes"],
    )?;

    assert_failure("adapter doctor hermes", &doctor);
    assert_stdout_contains("adapter doctor hermes", &doctor, "Hermes Agent");
    assert_stdout_contains("adapter doctor hermes", &doctor, "manifest:");
    assert_stdout_contains(
        "adapter doctor hermes",
        &doctor,
        "Adapter doctor found unavailable adapters",
    );
    Ok(())
}

#[test]
fn adapter_install_hermes_replaces_existing_manifest() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let adapter_dir = coven_home.join("adapters");
    fs::create_dir_all(&adapter_dir)?;
    let manifest_path = adapter_dir.join("hermes.json");
    fs::write(
        &manifest_path,
        r#"{"adapters":[{"id":"hermes","label":"Planted","executable":"sh","interactive_prompt_prefix_args":["-c"],"non_interactive_prompt_prefix_args":["-c"],"install_hint":"planted"}]}"#,
    )?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();

    let install = run_coven(
        &coven,
        &coven_home,
        &path,
        &["adapter", "install", "hermes"],
    )?;

    assert_success("adapter install hermes replaces manifest", &install);
    assert_stdout_contains(
        "adapter install hermes replaces manifest",
        &install,
        "Installed adapter `hermes`",
    );
    let manifest = serde_json::from_str::<Value>(&fs::read_to_string(manifest_path)?)?;
    let adapter = manifest
        .get("adapters")
        .and_then(Value::as_array)
        .and_then(|adapters| adapters.first())
        .expect("installed manifest should include one adapter");
    assert_eq!(adapter.get("id").and_then(Value::as_str), Some("hermes"));
    assert_eq!(
        adapter.get("executable").and_then(Value::as_str),
        Some("hermes")
    );
    Ok(())
}

#[test]
fn adapter_install_hermes_replaces_existing_manifest_directory() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let adapter_dir = coven_home.join("adapters");
    let manifest_path = adapter_dir.join("hermes.json");
    fs::create_dir_all(&manifest_path)?;
    fs::write(manifest_path.join("planted.txt"), "keep install broken")?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();

    let install = run_coven(
        &coven,
        &coven_home,
        &path,
        &["adapter", "install", "hermes"],
    )?;

    assert_success(
        "adapter install hermes replaces manifest directory",
        &install,
    );
    assert_stdout_contains(
        "adapter install hermes replaces manifest directory",
        &install,
        "Installed adapter `hermes`",
    );
    let manifest = serde_json::from_str::<Value>(&fs::read_to_string(manifest_path)?)?;
    let adapter = manifest
        .get("adapters")
        .and_then(Value::as_array)
        .and_then(|adapters| adapters.first())
        .expect("installed manifest should include one adapter");
    assert_eq!(adapter.get("id").and_then(Value::as_str), Some("hermes"));
    assert_eq!(
        adapter.get("executable").and_then(Value::as_str),
        Some("hermes")
    );
    Ok(())
}

#[test]
fn smoke_daemon_session_replay_and_safe_session_rituals() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let project_root = temp_dir.path().join("project");
    fs::create_dir_all(&project_root)?;

    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin)?;
    write_fake_codex(&fake_bin)?;
    let path = prepend_path(&fake_bin);
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let start = run_coven(&coven, &coven_home, &path, &["daemon", "start"])?;
    assert_success("daemon start", &start);
    assert_stdout_contains("daemon start", &start, "status=running");

    wait_for_daemon_health(&coven_home)?;

    let status = run_coven(&coven, &coven_home, &path, &["daemon", "status"])?;
    assert_success("daemon status", &status);
    assert_stdout_contains("daemon status", &status, "status=running");

    let status_json = run_coven(&coven, &coven_home, &path, &["daemon", "status", "--json"])?;
    assert_success("daemon status --json", &status_json);
    let status_value = parse_stdout_json("daemon status --json", &status_json)?;
    assert_eq!(
        status_value.get("status").and_then(Value::as_str),
        Some("running")
    );
    assert!(status_value.get("pid").and_then(Value::as_u64).is_some());
    assert!(status_value.get("socket").and_then(Value::as_str).is_some());
    assert!(status_value
        .get("started_at")
        .and_then(Value::as_str)
        .is_some());

    let replay_session = launch_daemon_session(
        &coven_home,
        &project_root,
        "codex",
        "smoke replay",
        "Smoke replay",
    )?;
    wait_for_session_status(&coven_home, &replay_session, "completed")?;
    wait_for_event_text(
        &coven_home,
        &replay_session,
        "fake codex complete: smoke replay",
    )?;

    let restart = run_coven(&coven, &coven_home, &path, &["daemon", "restart"])?;
    assert_success("daemon restart", &restart);
    assert_stdout_contains("daemon restart", &restart, "status=restarted");
    wait_for_daemon_health(&coven_home)?;

    let restarted_status = run_coven(&coven, &coven_home, &path, &["daemon", "status"])?;
    assert_success("daemon restarted status", &restarted_status);
    assert_stdout_contains(
        "daemon restarted status",
        &restarted_status,
        "status=running",
    );

    let attach = run_coven(&coven, &coven_home, &path, &["attach", &replay_session])?;
    assert_success("attach replay", &attach);
    assert_stdout_contains(
        "attach replay",
        &attach,
        "fake codex complete: smoke replay",
    );
    assert_stdout_contains(
        "attach replay",
        &attach,
        "[coven session completed (exit code 0)]",
    );

    let kill_session = launch_daemon_session(
        &coven_home,
        &project_root,
        "codex",
        "hold-for-kill",
        "Smoke kill",
    )?;
    wait_for_event_text(&coven_home, &kill_session, "fake codex ready for kill")?;

    let kill = run_coven(&coven, &coven_home, &path, &["kill", &kill_session])?;
    assert_success("kill", &kill);
    assert_stdout_contains("kill", &kill, "killed session");
    wait_for_session_status(&coven_home, &kill_session, "killed")?;

    // A second kill must refuse: the session is no longer running.
    let rekill = run_coven(&coven, &coven_home, &path, &["kill", &kill_session])?;
    assert_failure("rekill refused", &rekill);
    assert_stderr_contains("rekill refused", &rekill, "is not running");

    let archive = run_coven(&coven, &coven_home, &path, &["archive", &kill_session])?;
    assert_success("archive", &archive);
    assert_stdout_contains("archive", &archive, "archived session");

    let active_sessions = run_coven(&coven, &coven_home, &path, &["sessions", "--plain"])?;
    assert_success("active sessions", &active_sessions);
    assert_stdout_not_contains("active sessions", &active_sessions, &kill_session);

    let archived_sessions = run_coven(
        &coven,
        &coven_home,
        &path,
        &["sessions", "--all", "--plain"],
    )?;
    assert_success("archived sessions", &archived_sessions);
    assert_stdout_contains("archived sessions", &archived_sessions, &kill_session);
    assert_stdout_contains("archived sessions", &archived_sessions, "archived");

    let summon = run_coven(&coven, &coven_home, &path, &["summon", &kill_session])?;
    assert_success("summon", &summon);

    let restored_sessions = run_coven(&coven, &coven_home, &path, &["sessions", "--plain"])?;
    assert_success("restored sessions", &restored_sessions);
    assert_stdout_contains("restored sessions", &restored_sessions, &kill_session);
    assert_stdout_contains("restored sessions", &restored_sessions, "active");

    let sacrifice = run_coven(
        &coven,
        &coven_home,
        &path,
        &["sacrifice", &kill_session, "--yes"],
    )?;
    assert_success("sacrifice", &sacrifice);
    assert_stdout_contains("sacrifice", &sacrifice, "sacrificed session");

    let all_sessions = run_coven(
        &coven,
        &coven_home,
        &path,
        &["sessions", "--all", "--plain"],
    )?;
    assert_success("all sessions after sacrifice", &all_sessions);
    assert_stdout_not_contains("all sessions after sacrifice", &all_sessions, &kill_session);

    let stop = run_coven(&coven, &coven_home, &path, &["daemon", "stop"])?;
    assert_success("daemon stop", &stop);
    assert_stdout_contains("daemon stop", &stop, "status=stopped");

    let stopped = run_coven(&coven, &coven_home, &path, &["daemon", "status"])?;
    assert_success("daemon stopped status", &stopped);
    assert_stdout_contains("daemon stopped status", &stopped, "status=stopped");

    Ok(())
}

struct DaemonGuard {
    coven: PathBuf,
    coven_home: PathBuf,
    path: OsString,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = run_coven(
            &self.coven,
            &self.coven_home,
            &self.path,
            &["daemon", "stop"],
        );
    }
}

struct DaemonStatusRestoreGuard {
    path: PathBuf,
    contents: String,
}

impl Drop for DaemonStatusRestoreGuard {
    fn drop(&mut self) {
        let _ = fs::write(&self.path, &self.contents);
    }
}

fn coven_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_coven"))
}

fn run_coven(
    coven: &Path,
    coven_home: &Path,
    path: &OsString,
    args: &[&str],
) -> anyhow::Result<Output> {
    Command::new(coven)
        .args(args)
        .env("COVEN_HOME", coven_home)
        .env("PATH", path)
        .output()
        .map_err(Into::into)
}

/// Like `run_coven`, but runs from `cwd` with extra env vars — for commands
/// that discover a git repository from the working directory.
fn run_coven_in(
    coven: &Path,
    coven_home: &Path,
    path: &OsString,
    cwd: &Path,
    envs: &[(&str, &str)],
    args: &[&str],
) -> anyhow::Result<Output> {
    let mut command = Command::new(coven);
    command
        .args(args)
        .env("COVEN_HOME", coven_home)
        .env("PATH", path)
        .current_dir(cwd);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().map_err(Into::into)
}

/// Parse a command's entire stdout as one JSON document. Fails when anything
/// besides the JSON document landed on stdout.
fn parse_stdout_json(label: &str, output: &Output) -> anyhow::Result<Value> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).map_err(|error| {
        anyhow::anyhow!(
            "{label} stdout was not a single JSON document: {error}\nstdout:\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn init_git_repo(repo: &Path) -> anyhow::Result<()> {
    let git = |args: &[&str]| -> anyhow::Result<()> {
        let output = Command::new("git").args(args).current_dir(repo).output()?;
        anyhow::ensure!(
            output.status.success(),
            "git {args:?} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };
    git(&["init", "--initial-branch=main"])?;
    git(&[
        "-c",
        "user.name=Coven Smoke",
        "-c",
        "user.email=smoke@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--allow-empty",
        "-m",
        "init",
    ])?;
    Ok(())
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

fn assert_failure(label: &str, output: &Output) {
    assert!(
        !output.status.success(),
        "{label} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_stderr_contains(label: &str, output: &Output, needle: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(needle),
        "{label} stderr did not contain {needle:?}\nstdout:\n{}\nstderr:\n{stderr}",
        String::from_utf8_lossy(&output.stdout)
    );
}

fn assert_stdout_contains(label: &str, output: &Output, needle: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(needle),
        "{label} stdout did not contain {needle:?}\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_stdout_not_contains(label: &str, output: &Output, needle: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains(needle),
        "{label} stdout unexpectedly contained {needle:?}\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn prepend_path(fake_bin: &Path) -> OsString {
    let mut paths = vec![fake_bin.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).expect("test PATH should be joinable")
}

fn write_fake_codex(fake_bin: &Path) -> anyhow::Result<()> {
    let codex = fake_bin.join("codex");
    fs::write(
        &codex,
        r#"#!/bin/sh
# Consume the options terminator like the real CLI: coven passes prompts
# behind `--` so dash-prefixed prompts stay positional.
if [ "$1" = "--" ]; then shift; fi
if [ "$*" = "hold-for-kill" ]; then
  printf 'fake codex ready for kill\n'
  exec sleep 300
fi
printf 'fake codex complete: %s\n' "$*"
"#,
    )?;
    let mut permissions = fs::metadata(&codex)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&codex, permissions)?;
    Ok(())
}

/// Doctor exits 1 when coven-code is missing, so doctor tests that expect a
/// healthy environment plant a fake alongside the fake harness.
fn write_fake_coven_code(fake_bin: &Path) -> anyhow::Result<()> {
    let coven_code = fake_bin.join("coven-code");
    fs::write(&coven_code, "#!/bin/sh\nexit 0\n")?;
    let mut permissions = fs::metadata(&coven_code)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&coven_code, permissions)?;
    Ok(())
}

fn wait_for_daemon_health(coven_home: &Path) -> anyhow::Result<()> {
    wait_until("daemon health", || {
        let socket = coven_home.join("coven.sock");
        if !socket.exists() {
            return Ok(false);
        }
        let (status, body) = unix_http_request(coven_home, "GET", "/health", None)?;
        Ok(status == 200 && body.contains(r#""ok":true"#))
    })
}

fn daemon_status_pid(coven_home: &Path) -> anyhow::Result<u64> {
    let status = fs::read_to_string(coven_home.join("daemon.json"))?;
    let status = serde_json::from_str::<Value>(&status)?;
    status
        .get("pid")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("daemon status should include pid"))
}

fn pid_is_alive(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn launch_daemon_session(
    coven_home: &Path,
    project_root: &Path,
    harness: &str,
    prompt: &str,
    title: &str,
) -> anyhow::Result<String> {
    let body = json!({
        "projectRoot": project_root,
        "harness": harness,
        "prompt": prompt,
        "title": title
    })
    .to_string();
    let (status, response_body) = unix_http_request(coven_home, "POST", "/sessions", Some(&body))?;
    assert_eq!(
        status, 201,
        "unexpected session launch response: {response_body}"
    );
    Ok(serde_json::from_str::<Value>(&response_body)?
        .get("id")
        .and_then(Value::as_str)
        .expect("daemon response should include session id")
        .to_string())
}

fn wait_for_session_status(
    coven_home: &Path,
    session_id: &str,
    expected_status: &str,
) -> anyhow::Result<()> {
    wait_until(
        &format!("session {session_id} status {expected_status}"),
        || {
            let (_status, body) =
                unix_http_request(coven_home, "GET", &format!("/sessions/{session_id}"), None)?;
            let body = serde_json::from_str::<Value>(&body)?;
            Ok(body
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status == expected_status))
        },
    )
}

fn wait_for_event_text(coven_home: &Path, session_id: &str, needle: &str) -> anyhow::Result<()> {
    wait_until(&format!("session {session_id} event {needle:?}"), || {
        let (_status, body) = unix_http_request(
            coven_home,
            "GET",
            &format!("/events?sessionId={session_id}"),
            None,
        )?;
        Ok(body.contains(needle))
    })
}

fn wait_until(
    label: &str,
    mut predicate: impl FnMut() -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last_error = None;
    while Instant::now() < deadline {
        match predicate() {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => last_error = Some(error),
        }
        thread::sleep(Duration::from_millis(100));
    }

    if let Some(error) = last_error {
        anyhow::bail!("timed out waiting for {label}; last error: {error}");
    }
    anyhow::bail!("timed out waiting for {label}")
}

fn unix_http_request(
    coven_home: &Path,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> anyhow::Result<(u16, String)> {
    let body = body.unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: coven\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = UnixStream::connect(coven_home.join("coven.sock"))?;
    stream.write_all(request.as_bytes())?;
    stream.shutdown(Shutdown::Write)?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| anyhow::anyhow!("invalid HTTP response: {response}"))?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    Ok((status, body))
}
