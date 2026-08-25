#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
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
        "Coven daemon: not running",
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
        "Coven daemon: not running",
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
        "Coven daemon: running",
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
fn managed_daemon_identity_matches_health_across_restart_and_stop() -> anyhow::Result<()> {
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
    assert_success("managed daemon start", &start);
    let first = wait_for_daemon_identity(&coven_home)?;

    let restart = run_coven(&coven, &coven_home, &path, &["daemon", "restart"])?;
    assert_success("managed daemon restart", &restart);
    let restarted = wait_for_daemon_identity(&coven_home)?;
    assert_ne!(
        first.get("pid"),
        restarted.get("pid"),
        "restart must replace the launched daemon process"
    );
    assert_ne!(
        first.get("startedAt"),
        restarted.get("startedAt"),
        "restart must publish a new canonical daemon identity"
    );

    let stop = run_coven(&coven, &coven_home, &path, &["daemon", "stop"])?;
    assert_success("managed daemon stop", &stop);
    assert!(
        !coven_home.join("daemon.json").exists(),
        "stop must remove the launched daemon's status"
    );
    assert!(
        !coven_home.join("coven.sock").exists(),
        "stop must remove the launched daemon's socket"
    );
    Ok(())
}

#[test]
fn daemon_restart_uses_only_platform_safe_base_fallback() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let selected_home = temp_dir.path().join("coven-home");
    fs::create_dir(&selected_home)?;
    let coven_home = fs::canonicalize(selected_home)?;
    let fixture = temp_dir.path().join("base-daemon");
    compile_unix_base_daemon_fixture(&fixture)?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };
    let mut unrelated = ChildGuard::spawn(
        Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )?;

    let mut spoofed = spawn_base_daemon(
        &fixture,
        &coven_home,
        &unrelated.id().to_string(),
        "base",
        false,
    )?;
    wait_for_base_daemon_status(&coven_home, unrelated.id())?;
    let rejected = run_coven(&coven, &coven_home, &path, &["daemon", "restart"])?;
    assert_failure(
        "restart with BASE health claiming an unrelated PID",
        &rejected,
    );
    assert!(
        spoofed.is_running()?,
        "identity rejection signaled the connected BASE fixture"
    );
    assert!(
        unrelated.is_running()?,
        "identity rejection signaled an unrelated process"
    );
    spoofed.terminate()?;
    fs::remove_file(coven_home.join("daemon.json"))?;
    fs::remove_file(coven_home.join("coven.sock"))?;
    let _ = fs::remove_file(coven_home.join("base-requests.log"));

    let mut incompatible = spawn_base_daemon(&fixture, &coven_home, "self", "incompatible", false)?;
    let incompatible_pid = incompatible.id();
    wait_for_base_daemon_status(&coven_home, incompatible_pid)?;
    let rejected = run_coven(&coven, &coven_home, &path, &["daemon", "restart"])?;
    assert_failure("restart with non-BASE health capabilities", &rejected);
    assert!(
        incompatible.is_running()?,
        "fallback signaled a daemon without BASE health capabilities"
    );
    incompatible.terminate()?;
    fs::remove_file(coven_home.join("daemon.json"))?;
    fs::remove_file(coven_home.join("coven.sock"))?;
    let _ = fs::remove_file(coven_home.join("base-requests.log"));

    let mut base = spawn_base_daemon(&fixture, &coven_home, "self", "base", true)?;
    let base_pid = base.id();
    wait_for_base_daemon_status(&coven_home, base_pid)?;
    let restart = run_coven(&coven, &coven_home, &path, &["daemon", "restart"])?;

    #[cfg(target_os = "linux")]
    {
        if !restart.status.success() {
            eprintln!(
                "BASE requests before restart failure:\n{}",
                fs::read_to_string(coven_home.join("base-requests.log")).unwrap_or_default()
            );
        }
        assert_success("restart from BASE daemon", &restart);
        wait_for_child_exit(&mut base, "BASE daemon")?;
        let replacement = wait_for_daemon_identity(&coven_home)?;
        assert_ne!(replacement["pid"].as_u64(), Some(u64::from(base_pid)));
        assert!(
            unrelated.is_running()?,
            "BASE fallback signaled an unrelated process"
        );
        let requests = fs::read_to_string(coven_home.join("base-requests.log"))?;
        assert_eq!(
            requests.lines().collect::<Vec<_>>(),
            vec![
                "POST /api/v1/internal/lifecycle/shutdown HTTP/1.1",
                "GET /health HTTP/1.1",
            ],
            "restart must prefer the authenticated shutdown route before BASE fallback"
        );

        let stop = run_coven(&coven, &coven_home, &path, &["daemon", "stop"])?;
        assert_success("stop replacement daemon", &stop);
    }

    #[cfg(not(target_os = "linux"))]
    {
        assert_failure(
            "restart from a BASE daemon without identity-bound signaling",
            &restart,
        );
        assert_stderr_contains(
            "legacy BASE restart upgrade guidance",
            &restart,
            "upgrade Coven",
        );
        assert_stderr_contains(
            "legacy BASE restart manual recovery guidance",
            &restart,
            "restart the daemon manually",
        );
        assert!(
            base.is_running()?,
            "fail-closed legacy fallback signaled the BASE daemon"
        );
        assert!(
            unrelated.is_running()?,
            "fail-closed legacy fallback signaled an unrelated process"
        );
        let requests = fs::read_to_string(coven_home.join("base-requests.log"))?;
        assert_eq!(
            requests.lines().collect::<Vec<_>>(),
            vec![
                "POST /api/v1/internal/lifecycle/shutdown HTTP/1.1",
                "GET /health HTTP/1.1",
            ],
            "upgrade refusal must still authenticate BASE health after an exact 404"
        );
        base.terminate()?;
        fs::remove_file(coven_home.join("daemon.json"))?;
        fs::remove_file(coven_home.join("coven.sock"))?;
    }

    unrelated.terminate()?;
    Ok(())
}

#[test]
fn daemon_stop_terminates_live_piped_session_descendants() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let project_root = temp_dir.path().join("project");
    let fake_bin = temp_dir.path().join("bin");
    let descendant_pid_file = temp_dir.path().join("descendant.pid");
    fs::create_dir_all(&project_root)?;
    fs::create_dir_all(&fake_bin)?;
    write_shutdown_fake_codex(&fake_bin, &descendant_pid_file)?;
    let path = prepend_path(&fake_bin);
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let start = run_coven(&coven, &coven_home, &path, &["daemon", "start"])?;
    assert_success("daemon start for shutdown containment", &start);
    wait_for_daemon_health(&coven_home)?;
    let body = json!({
        "projectRoot": project_root,
        "harness": "codex",
        "launchMode": "nonInteractive",
        "prompt": "hold for daemon shutdown",
        "title": "Shutdown containment"
    })
    .to_string();
    let (status, response) = unix_http_request(&coven_home, "POST", "/sessions", Some(&body))?;
    assert_eq!(status, 201, "unexpected launch response: {response}");

    wait_until("piped descendant pid", || Ok(descendant_pid_file.exists()))?;
    let descendant_pid = fs::read_to_string(&descendant_pid_file)?
        .trim()
        .parse::<u32>()?;
    assert!(
        pid_is_alive(descendant_pid),
        "fixture descendant should be live before daemon stop"
    );

    let stop = run_coven(&coven, &coven_home, &path, &["daemon", "stop"])?;
    assert_success("daemon stop with live piped child", &stop);
    wait_until("piped descendant termination after daemon stop", || {
        Ok(!pid_is_alive(descendant_pid))
    })?;
    assert!(
        !coven_home.join("coven.sock").exists(),
        "graceful shutdown should remove its Unix socket"
    );
    assert!(
        !coven_home.join("daemon.json").exists(),
        "graceful shutdown should remove its status file"
    );
    Ok(())
}

#[test]
fn daemon_stop_kills_piped_tree_stalled_before_runtime_publication() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let project_root = temp_dir.path().join("project");
    let fake_bin = temp_dir.path().join("bin");
    let barrier_dir = temp_dir.path().join("prepublication-barrier");
    let descendant_pid_file = temp_dir.path().join("prepublication-descendant.pid");
    fs::create_dir_all(&project_root)?;
    fs::create_dir_all(&fake_bin)?;
    write_shutdown_fake_codex(&fake_bin, &descendant_pid_file)?;
    let path = prepend_path(&fake_bin);
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let start = Command::new(&coven)
        .args(["daemon", "start"])
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .env("COVEN_TEST_PIPED_PREPUBLICATION_BARRIER_DIR", &barrier_dir)
        .output()?;
    assert_success("daemon start for pre-publication containment", &start);
    wait_for_daemon_health(&coven_home)?;

    let body = json!({
        "projectRoot": project_root,
        "harness": "codex",
        "launchMode": "nonInteractive",
        "prompt": "hold before runtime ownership publication",
        "title": "Pre-publication shutdown containment"
    })
    .to_string();
    let request = format!(
        "POST /sessions HTTP/1.1\r\nHost: coven\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut launch_stream = UnixStream::connect(coven_home.join("coven.sock"))?;
    launch_stream.write_all(request.as_bytes())?;
    launch_stream.shutdown(Shutdown::Write)?;

    wait_until("piped pre-publication barrier", || {
        Ok(barrier_dir.join("ready").exists())
    })?;
    wait_until("pre-publication descendant pid", || {
        Ok(descendant_pid_file.exists())
    })?;
    let descendant_pid = fs::read_to_string(&descendant_pid_file)?
        .trim()
        .parse::<u32>()?;
    assert!(
        pid_is_alive(descendant_pid),
        "fixture descendant should be live in the pre-publication window"
    );

    let started = Instant::now();
    let stop = run_coven(&coven, &coven_home, &path, &["daemon", "stop"])?;
    let elapsed = started.elapsed();
    assert_success("daemon stop during pre-publication launch", &stop);
    assert!(
        elapsed < Duration::from_secs(2),
        "daemon stop exceeded its documented two-second deadline: {elapsed:?}"
    );
    wait_until("pre-publication descendant guardian cleanup", || {
        Ok(!pid_is_alive(descendant_pid))
    })?;
    drop(launch_stream);
    Ok(())
}

#[test]
fn stalled_tcp_request_cannot_exhaust_daemon_stop_deadline() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();
    let port_reservation = TcpListener::bind("127.0.0.1:0")?;
    let tcp_addr = port_reservation.local_addr()?.to_string();
    drop(port_reservation);
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };
    let mut daemon = Command::new(&coven)
        .args(["daemon", "serve", "--tcp", &tcp_addr])
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    wait_for_daemon_health(&coven_home)?;

    let mut stalled = TcpStream::connect(&tcp_addr)?;
    stalled.write_all(
        b"POST /api/v1/sessions HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n\r\n{",
    )?;
    stalled.flush()?;
    // The TCP worker polls accept every 25 ms. Leave ample time for it to
    // accept this request and block waiting for the deliberately absent body.
    thread::sleep(Duration::from_millis(250));

    let started = Instant::now();
    // `serve` is our direct child in this test. Reap it concurrently with the
    // separate `daemon stop` command; otherwise that command's kill(0) probe
    // correctly observes our unreaped zombie as an extant pid and times out.
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let stop_coven = coven.clone();
    let stop_home = coven_home.clone();
    let stop_path = path.clone();
    thread::spawn(move || {
        let _ = stop_tx.send(run_coven(
            &stop_coven,
            &stop_home,
            &stop_path,
            &["daemon", "stop"],
        ));
    });
    let daemon_deadline = Instant::now() + Duration::from_secs(2);
    let daemon_status = loop {
        if let Some(status) = daemon.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= daemon_deadline {
            break None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stop = stop_rx.recv_timeout(Duration::from_secs(3))??;
    let elapsed = started.elapsed();
    if daemon_status.is_none() {
        let _ = daemon.kill();
        let _ = daemon.wait();
    }
    assert_success("daemon stop with stalled TCP request", &stop);
    assert!(
        elapsed < Duration::from_secs(2),
        "daemon stop exceeded its documented two-second deadline: {elapsed:?}"
    );
    let daemon_status = daemon_status.expect("successful daemon stop must reap the serve process");
    assert!(
        daemon_status.success(),
        "foreground daemon exited with {daemon_status}"
    );
    assert!(
        !coven_home.join("coven.sock").exists(),
        "bounded graceful shutdown should remove its Unix socket"
    );
    assert!(
        !coven_home.join("daemon.json").exists(),
        "bounded graceful shutdown should remove its status file"
    );
    drop(stalled);
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
fn doctor_json_reports_blocking_failure_when_no_harness_is_available() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let fake_home = temp_dir.path().join("fake-home");
    fs::create_dir_all(&coven_home)?;
    fs::create_dir_all(&fake_home)?;
    let coven = coven_bin();
    let empty_path = OsString::new();

    let output = Command::new(&coven)
        .args(["doctor", "--json"])
        .env("COVEN_HOME", &coven_home)
        .env_remove("COVEN_ENGINE_BIN")
        .env("PATH", &empty_path)
        .env("HOME", &fake_home)
        .output()?;

    // Same exit contract as prose doctor: blocking problems exit 1.
    assert_failure("doctor --json without harnesses", &output);
    let value = parse_stdout_json("doctor --json without harnesses", &output)?;
    assert_eq!(value["ok"], Value::Bool(false));
    assert_eq!(value["blocking"], Value::Bool(true));
    let checks = value["checks"]
        .as_array()
        .expect("doctor JSON should include a checks array");
    let harnesses = checks
        .iter()
        .find(|check| check["id"] == "harnesses")
        .expect("doctor JSON should include the harnesses aggregate check");
    assert_eq!(harnesses["status"], "fail");
    let engine = checks
        .iter()
        .find(|check| check["id"] == "engine")
        .expect("doctor JSON should include the engine check");
    assert_eq!(engine["status"], "fail");
    assert!(
        value["nextSteps"]
            .as_array()
            .is_some_and(|steps| !steps.is_empty()),
        "doctor JSON should carry next steps: {value}"
    );
    Ok(())
}

#[test]
fn doctor_json_passes_with_fake_harness_and_engine() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin)?;
    write_fake_codex(&fake_bin)?;
    write_fake_coven_code(&fake_bin)?;
    let path = prepend_path(&fake_bin);
    let coven = coven_bin();

    let output = run_coven_with_engine(
        &coven,
        &coven_home,
        &path,
        &fake_bin.join("coven-code"),
        &["doctor", "--json"],
    )?;

    assert_success("doctor --json with fakes", &output);
    let value = parse_stdout_json("doctor --json with fakes", &output)?;
    assert_eq!(value["ok"], Value::Bool(true));
    assert_eq!(value["blocking"], Value::Bool(false));
    let checks = value["checks"]
        .as_array()
        .expect("doctor JSON should include a checks array");
    assert!(
        checks.iter().all(|check| check["status"] != "fail"),
        "passing doctor must not report fail checks: {value}"
    );
    let codex = checks
        .iter()
        .find(|check| check["id"] == "harness:codex")
        .expect("doctor JSON should report harness:codex");
    assert_eq!(codex["status"], "pass");
    assert!(
        codex["message"]
            .as_str()
            .is_some_and(|message| message.contains("executable is available")),
        "harness discovery must describe executable availability only: {codex}"
    );
    let codex_auth = checks
        .iter()
        .find(|check| check["id"] == "credentials:codex")
        .expect("doctor JSON should report the unverified Codex auth boundary");
    assert_eq!(codex_auth["status"], "warn");
    assert_eq!(
        codex_auth["message"],
        "executable available; authentication not verified"
    );
    let engine_auth = checks
        .iter()
        .find(|check| check["id"] == "credentials:engine")
        .expect("doctor JSON should report the engine's local auth state");
    assert_eq!(engine_auth["status"], "warn");
    assert_eq!(
        engine_auth["message"],
        "authentication configured; provider turn not verified"
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("logged in"),
        "doctor must not turn local configuration evidence into a live-auth claim"
    );
    Ok(())
}

#[test]
fn adapter_doctor_json_reports_each_adapter() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin)?;
    write_fake_codex(&fake_bin)?;
    let path = prepend_path(&fake_bin);
    let coven = coven_bin();

    let output = run_coven(
        &coven,
        &coven_home,
        &path,
        &["adapter", "doctor", "codex", "--json"],
    )?;

    assert_success("adapter doctor codex --json", &output);
    let value = parse_stdout_json("adapter doctor codex --json", &output)?;
    assert_eq!(value["ok"], Value::Bool(true));
    let checks = value["checks"]
        .as_array()
        .expect("adapter doctor JSON should include a checks array");
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0]["id"], "adapter:codex");
    assert_eq!(checks[0]["status"], "pass");

    // A missing adapter is blocking for adapter doctor: JSON keeps the exit-1
    // contract and carries the install hint.
    let empty_path = OsString::new();
    let fake_home = temp_dir.path().join("fake-home");
    fs::create_dir_all(&fake_home)?;
    let output = Command::new(&coven)
        .args(["adapter", "doctor", "codex", "--json"])
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &empty_path)
        .env("HOME", &fake_home)
        .output()?;
    assert_failure("adapter doctor codex --json (missing)", &output);
    let value = parse_stdout_json("adapter doctor codex --json (missing)", &output)?;
    assert_eq!(value["ok"], Value::Bool(false));
    assert_eq!(value["checks"][0]["status"], "fail");
    assert!(
        value["checks"][0]["hint"]
            .as_str()
            .is_some_and(|hint| !hint.is_empty()),
        "missing adapter should carry an install hint: {value}"
    );
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

    let output = run_coven_with_engine(
        &coven,
        &coven_home,
        &path,
        &fake_bin.join("coven-code"),
        &["doctor"],
    )?;

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

    let output = run_coven_with_engine(
        &coven,
        &coven_home,
        &path,
        &fake_bin.join("coven-code"),
        &["doctor"],
    )?;

    assert_success("doctor without familiars", &output);
    assert_stdout_contains("doctor without familiars", &output, "none configured");
    assert_stdout_contains("doctor without familiars", &output, "familiars.toml");
    assert_stdout_contains(
        "doctor without familiars",
        &output,
        "`codex` executable is available",
    );
    assert_stdout_contains(
        "doctor without familiars",
        &output,
        "authentication not verified",
    );
    assert_stdout_contains(
        "doctor without familiars",
        &output,
        "provider turn not verified",
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("logged in"),
        "doctor must not turn local configuration evidence into a live-auth claim"
    );
    Ok(())
}

#[test]
fn doctor_missing_harness_prints_cross_platform_setup_loop() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let fake_home = temp_dir.path().join("fake-home");
    fs::create_dir_all(&coven_home)?;
    fs::create_dir_all(&fake_home)?;
    let coven = coven_bin();
    let empty_path = OsString::new();

    // Point HOME at a scratch dir so the managed-engine resolver (which reads
    // ~/.coven/engine/) finds nothing — this ensures all three harnesses are
    // reported as missing regardless of what the developer has installed.
    let output = Command::new(&coven)
        .args(["doctor"])
        .env("COVEN_HOME", &coven_home)
        .env_remove("COVEN_ENGINE_BIN")
        .env("PATH", &empty_path)
        .env("HOME", &fake_home)
        .output()?;

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
    assert_stdout_contains("doctor without harnesses", &output, "[--] Codex");
    assert_stdout_contains(
        "doctor without harnesses",
        &output,
        "[!!] No supported harness is available",
    );
    assert_stdout_contains(
        "doctor without harnesses",
        &output,
        "`coven-code` is missing",
    );
    assert_stdout_contains(
        "doctor without harnesses",
        &output,
        "Set up at least one harness in this same shell.",
    );
    for command in [
        "Codex: coven setup codex",
        "Claude Code: coven setup claude",
        "GitHub Copilot CLI: coven setup copilot",
    ] {
        assert_stdout_contains("doctor without harnesses", &output, command);
    }
    assert_stdout_not_contains("doctor without harnesses", &output, "claude doctor");
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

    let output = run_coven_with_engine(
        &coven,
        &coven_home,
        &path,
        &fake_bin.join("coven-code"),
        &["doctor"],
    )?;

    assert_success("doctor with live daemon", &output);
    assert_stdout_contains("doctor with live daemon", &output, "Daemon:");
    assert_stdout_contains("doctor with live daemon", &output, "Running (pid ");
    assert_stdout_contains("doctor with live daemon", &output, ", socket ");
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
fn attached_copilot_run_persists_output_and_exit_events() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let project = temp_dir.path().join("project");
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&coven_home)?;
    fs::create_dir_all(&project)?;
    fs::create_dir_all(&fake_bin)?;
    init_git_repo(&project)?;
    write_fake_copilot(&fake_bin)?;
    let path = prepend_path(&fake_bin);
    let coven = coven_bin();

    let run = run_coven_in(
        &coven,
        &coven_home,
        &path,
        &project,
        &[],
        &["run", "copilot", "ledger check"],
    )?;
    assert_success("attached copilot run", &run);
    assert_stdout_contains("attached copilot run", &run, "fake copilot ledger output");

    let sessions = run_coven(&coven, &coven_home, &path, &["sessions", "--json"])?;
    assert_success("list copilot sessions", &sessions);
    let sessions = parse_stdout_json("list copilot sessions", &sessions)?;
    let session_id = sessions["sessions"]
        .as_array()
        .context("sessions array")?
        .iter()
        .find(|session| session["harness"] == "copilot")
        .and_then(|session| session["id"].as_str())
        .context("copilot session id")?;

    let events = run_coven(
        &coven,
        &coven_home,
        &path,
        &["sessions", "events", session_id, "--json"],
    )?;
    assert_success("list copilot session events", &events);
    let events = parse_stdout_json("list copilot session events", &events)?;
    let events = events["events"].as_array().context("events array")?;
    let kinds: Vec<_> = events
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .collect();

    assert_eq!(kinds, ["output", "exit"]);
    let output_payload: Value = serde_json::from_str(
        events[0]["payload_json"]
            .as_str()
            .context("output payload JSON")?,
    )?;
    assert!(
        output_payload["data"]
            .as_str()
            .is_some_and(|data| data.contains("fake copilot ledger output")),
        "unexpected output payload: {}",
        output_payload
    );
    let exit_payload: Value = serde_json::from_str(
        events[1]["payload_json"]
            .as_str()
            .context("exit payload JSON")?,
    )?;
    assert_eq!(exit_payload["status"], "completed");
    assert_eq!(exit_payload["exitCode"], 0);

    let log = run_coven(
        &coven,
        &coven_home,
        &path,
        &["sessions", "log", session_id, "--json"],
    )?;
    assert_success("read copilot session log", &log);
    let log = parse_stdout_json("read copilot session log", &log)?;
    assert!(log
        .as_array()
        .context("log lines")?
        .iter()
        .any(|line| line["message"]
            .as_str()
            .is_some_and(|message| message.contains("fake copilot ledger output"))));
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
    let manifest: Value =
        serde_json::from_slice(&fs::read(coven_home.join("adapters").join("hermes.json"))?)?;
    let adapter = manifest["adapters"]
        .as_array()
        .and_then(|adapters| adapters.first())
        .expect("installed Hermes manifest contains one adapter");
    assert_eq!(adapter["executable"], "hermes");
    assert_eq!(adapter["prompt_flag"], "--query");
    assert_eq!(adapter["model_flag"], "--model");
    assert_eq!(adapter["model_id_transform"], "preserve");
    assert_eq!(adapter["version"], "1.0.3");

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
fn adapter_install_grok_writes_trusted_manifest() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();

    let install = run_coven(&coven, &coven_home, &path, &["adapter", "install", "grok"])?;

    assert_success("adapter install grok", &install);
    assert_stdout_contains("adapter install grok", &install, "Installed adapter `grok`");
    // The suggested first run must carry an explicit --permission: Grok's
    // documented contract treats an omitted --permission as unsupported
    // (headless Grok auto-cancels would-prompt tool calls instead of
    // behaving like `full`).
    assert_stdout_contains(
        "adapter install grok",
        &install,
        "coven run grok --permission full",
    );
    let manifest_path = coven_home.join("adapters").join("grok.json");
    let manifest = serde_json::from_str::<Value>(&fs::read_to_string(manifest_path)?)?;
    let adapter = manifest
        .get("adapters")
        .and_then(Value::as_array)
        .and_then(|adapters| adapters.first())
        .expect("installed manifest should include one adapter");
    assert_eq!(adapter.get("id").and_then(Value::as_str), Some("grok"));
    assert_eq!(
        adapter.get("executable").and_then(Value::as_str),
        Some("grok")
    );
    assert_eq!(
        adapter.get("prompt_flag").and_then(Value::as_str),
        Some("--single")
    );
    assert_eq!(
        adapter
            .get("non_interactive_prompt_prefix_args")
            .and_then(Value::as_array)
            .and_then(|args| args.last())
            .and_then(Value::as_str),
        Some("plain")
    );

    // Keep diagnosis independent of whether Grok Build is installed on the
    // contributor's machine.
    let doctor = run_coven(
        &coven,
        &coven_home,
        &OsString::new(),
        &["adapter", "doctor", "grok"],
    )?;
    assert_failure("adapter doctor grok", &doctor);
    assert_stdout_contains("adapter doctor grok", &doctor, "Grok Build");
    assert_stdout_contains("adapter doctor grok", &doctor, "manifest:");
    assert_stdout_contains(
        "adapter doctor grok",
        &doctor,
        "Adapter doctor found unavailable adapters",
    );
    Ok(())
}

/// A plain `coven run grok <prompt>` turn: no `--stream-json`, no daemon, no
/// `--continue`. This is the same shape every `stream: false` harness gets
/// (Copilot is `stream: false` too) — `run_session`'s `conversation_hint`
/// stays `None` here regardless of `capabilities.preassigned_session_id`;
/// only the TUI chat path (`conversation_hint_for_harness` in
/// `tui/chat/app.rs`, unit-tested there) and the `--stream-json` passthrough
/// (stream-capable harnesses only) ever assign one. So unlike the
/// `harness.rs` argv-construction tests above, this test intentionally does
/// not exercise `--session-id`/`--resume` — that would test a usage pattern
/// the plain CLI doesn't support for this class of harness, Grok included.
#[test]
fn grok_adapter_runs_a_plain_turn() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let fake_bin = temp_dir.path().join("bin");
    let repo = temp_dir.path().join("repo");
    let arg_log = temp_dir.path().join("grok-args.log");
    fs::create_dir_all(&fake_bin)?;
    fs::create_dir_all(&repo)?;
    init_git_repo(&repo)?;
    write_fake_grok(&fake_bin)?;
    let path = prepend_path(&fake_bin);
    let coven = coven_bin();

    let install = run_coven(&coven, &coven_home, &path, &["adapter", "install", "grok"])?;
    assert_success("adapter install grok", &install);
    let doctor = run_coven(&coven, &coven_home, &path, &["adapter", "doctor", "grok"])?;
    assert_success("adapter doctor available grok", &doctor);

    let arg_log_value = arg_log.to_string_lossy().into_owned();
    let turn = run_coven_in(
        &coven,
        &coven_home,
        &path,
        &repo,
        &[("FAKE_GROK_ARG_LOG", arg_log_value.as_str())],
        &["run", "grok", "explain this repo"],
    )?;
    assert_success("Grok turn", &turn);
    assert_stdout_contains("Grok turn", &turn, "fake grok reply");

    let invocations = fs::read_to_string(&arg_log)?;
    assert!(invocations.contains("--output-format\nplain\n"));
    assert!(!invocations.contains("--session-id"));
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
    assert_stdout_contains("daemon start", &start, "Coven daemon: running");

    wait_for_daemon_health(&coven_home)?;

    let status = run_coven(&coven, &coven_home, &path, &["daemon", "status"])?;
    assert_success("daemon status", &status);
    assert_stdout_contains("daemon status", &status, "Coven daemon: running");

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
    assert_stdout_contains("daemon restart", &restart, "Coven daemon: restarted");
    wait_for_daemon_health(&coven_home)?;

    let restarted_status = run_coven(&coven, &coven_home, &path, &["daemon", "status"])?;
    assert_success("daemon restarted status", &restarted_status);
    assert_stdout_contains(
        "daemon restarted status",
        &restarted_status,
        "Coven daemon: running",
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
    assert_stdout_contains("daemon stop", &stop, "Coven daemon: stopped");

    let stopped = run_coven(&coven, &coven_home, &path, &["daemon", "status"])?;
    assert_success("daemon stopped status", &stopped);
    assert_stdout_contains(
        "daemon stopped status",
        &stopped,
        "Coven daemon: not running",
    );

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

struct ChildGuard {
    child: std::process::Child,
}

impl ChildGuard {
    fn spawn(command: &mut Command) -> anyhow::Result<Self> {
        Ok(Self {
            child: command.spawn()?,
        })
    }

    fn id(&self) -> u32 {
        self.child.id()
    }

    fn is_running(&mut self) -> anyhow::Result<bool> {
        Ok(self.child.try_wait()?.is_none())
    }

    fn terminate(&mut self) -> anyhow::Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        let _ = self.child.wait()?;
        Ok(())
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
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

fn run_coven_with_engine(
    coven: &Path,
    coven_home: &Path,
    path: &OsString,
    engine: &Path,
    args: &[&str],
) -> anyhow::Result<Output> {
    Command::new(coven)
        .args(args)
        .env("COVEN_HOME", coven_home)
        .env("COVEN_ENGINE_BIN", engine)
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

fn compile_unix_base_daemon_fixture(output: &Path) -> anyhow::Result<()> {
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unix_base_daemon.rs");
    let compile = Command::new("rustc")
        .args(["--edition=2021", "-o"])
        .arg(output)
        .arg(source)
        .output()?;
    assert_success("compile BASE daemon fixture", &compile);
    Ok(())
}

fn spawn_base_daemon(
    fixture: &Path,
    coven_home: &Path,
    reported_pid: &str,
    health_style: &str,
    relative_socket: bool,
) -> anyhow::Result<ChildGuard> {
    let mut command = Command::new(fixture);
    command
        .arg(coven_home)
        .arg("2026-08-16T12:00:00Z")
        .arg(reported_pid)
        .arg(health_style)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if relative_socket {
        command.env("COVEN_TEST_BASE_RECORDED_SOCKET", "relative");
    }
    ChildGuard::spawn(&mut command)
}

fn wait_for_base_daemon_status(coven_home: &Path, expected_pid: u32) -> anyhow::Result<()> {
    wait_until("BASE daemon status", || {
        let status_path = coven_home.join("daemon.json");
        if !status_path.exists() || !coven_home.join("coven.sock").exists() {
            return Ok(false);
        }
        let status: Value = serde_json::from_str(&fs::read_to_string(status_path)?)?;
        Ok(status["pid"].as_u64() == Some(u64::from(expected_pid)))
    })
}

#[cfg(target_os = "linux")]
fn wait_for_child_exit(child: &mut ChildGuard, label: &str) -> anyhow::Result<()> {
    wait_until(&format!("{label} exit"), || Ok(!child.is_running()?))
}

fn write_fake_copilot(fake_bin: &Path) -> anyhow::Result<()> {
    let copilot = fake_bin.join("copilot");
    fs::write(
        &copilot,
        "#!/bin/sh\nprintf 'fake copilot ledger output\\n'\n",
    )?;
    let mut permissions = fs::metadata(&copilot)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&copilot, permissions)?;
    Ok(())
}

fn write_shutdown_fake_codex(fake_bin: &Path, pid_file: &Path) -> anyhow::Result<()> {
    let codex = fake_bin.join("codex");
    fs::write(
        &codex,
        format!(
            "#!/bin/sh\nsleep 300 </dev/null >/dev/null 2>&1 &\nprintf '%s\\n' \"$!\" > '{}'\nprintf 'fake codex ready for daemon shutdown\\n'\nwait\n",
            pid_file.display()
        ),
    )?;
    let mut permissions = fs::metadata(&codex)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&codex, permissions)?;
    Ok(())
}

fn write_fake_grok(fake_bin: &Path) -> anyhow::Result<()> {
    let grok = fake_bin.join("grok");
    fs::write(
        &grok,
        r#"#!/bin/sh
if [ -n "$FAKE_GROK_ARG_LOG" ]; then
  printf 'BEGIN\n' >> "$FAKE_GROK_ARG_LOG"
  for arg in "$@"; do
    printf '%s\n' "$arg" >> "$FAKE_GROK_ARG_LOG"
  done
fi

printf 'fake grok reply\n'
"#,
    )?;
    let mut permissions = fs::metadata(&grok)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&grok, permissions)?;
    Ok(())
}

/// Doctor exits 1 when coven-code is missing, so doctor tests that expect a
/// healthy environment plant a fake alongside the fake harness.
fn write_fake_coven_code(fake_bin: &Path) -> anyhow::Result<()> {
    let coven_code = fake_bin.join("coven-code");
    fs::write(
        &coven_code,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf 'coven-code 0.6.1\n'
elif [ "$1" = "auth" ] && [ "$2" = "status" ] && [ "$3" = "--json" ]; then
  printf '{"loggedIn":true}\n'
fi
exit 0
"#,
    )?;
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

fn wait_for_daemon_identity(coven_home: &Path) -> anyhow::Result<Value> {
    let mut identity = None;
    wait_until("daemon status and health identity agreement", || {
        let status_path = coven_home.join("daemon.json");
        if !status_path.exists() || !coven_home.join("coven.sock").exists() {
            return Ok(false);
        }
        let status: Value = serde_json::from_str(&fs::read_to_string(status_path)?)?;
        let (http_status, body) = unix_http_request(coven_home, "GET", "/health", None)?;
        if http_status != 200 {
            return Ok(false);
        }
        let health: Value = serde_json::from_str(&body)?;
        let Some(daemon) = health.get("daemon") else {
            return Ok(false);
        };
        if daemon != &status {
            return Ok(false);
        }
        identity = Some(status);
        Ok(true)
    })?;
    identity.ok_or_else(|| anyhow::anyhow!("daemon identity was not captured"))
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
    let mut last_observed = None;
    wait_until(
        &format!("session {session_id} status {expected_status}"),
        || {
            let (_status, body) =
                unix_http_request(coven_home, "GET", &format!("/sessions/{session_id}"), None)?;
            let body = serde_json::from_str::<Value>(&body)?;
            last_observed = Some(body.to_string());
            Ok(body
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status == expected_status))
        },
    )
    .with_context(|| {
        format!(
            "last observed session response: {}",
            last_observed.unwrap_or_else(|| "<none>".to_string())
        )
    })
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
