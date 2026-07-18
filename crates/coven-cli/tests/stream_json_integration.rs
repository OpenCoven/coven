//! End-to-end tests for `coven run ... --stream-json`.
//!
//! These tests are `#[ignore]` by default because they shell out to the
//! built `coven` binary. Run them explicitly with:
//!
//!   cargo test -p coven-cli --test stream_json_integration -- --ignored
//!
//! The `--detach` path lets us exercise the framing without requiring codex
//! or claude to be installed: we synthesize `system.init` + `user` + `result`
//! around a session that never actually launches a harness.

use std::process::{Command, Stdio};

/// Regression test for #307: with a non-claude harness that spews raw PTY
/// noise (ANSI escapes, carriage-return progress lines, partial/broken
/// JSON), every line the CLI writes to stdout in `--stream-json` mode must
/// still parse as JSON. The noise is captured off the PTY and wrapped in
/// `output` events instead of interleaving with the frames.
///
/// Unlike its `#[ignore]`d siblings above, this test is hermetic (fake
/// harness on PATH, temp `COVEN_HOME`, temp project cwd) and runs by
/// default, matching the smoke-test conventions.
#[cfg(unix)]
#[test]
fn stream_json_stdout_stays_jsonl_when_harness_spews_pty_noise() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home).expect("failed to create coven home");
    let project_root = temp_dir.path().join("project");
    fs::create_dir_all(&project_root).expect("failed to create project root");

    // A manifest-backed external adapter that behaves like a TUI-ish harness
    // on a PTY: colored
    // banner, carriage-return progress updates, a partial line that looks
    // like broken JSON, then a completion marker. Before #307 all of this
    // leaked raw onto stdout between the JSONL frames.
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin).expect("failed to create fake bin dir");
    let fake_harness = fake_bin.join("fake-pty");
    fs::write(
        &fake_harness,
        "#!/bin/sh\n\
         printf '\\033[1;32mfake pty banner\\033[0m\\n'\n\
         printf 'progress 1/2\\rprogress 2/2\\n'\n\
         printf '{\"not\":\"terminated'\n\
         printf '\\nfake pty noise complete\\n'\n",
    )
    .expect("failed to write fake PTY harness");
    let mut permissions = fs::metadata(&fake_harness)
        .expect("failed to stat fake PTY harness")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_harness, permissions).expect("failed to chmod fake PTY harness");
    let manifest = temp_dir.path().join("adapters.json");
    fs::write(
        &manifest,
        r#"{
  "adapters": [{
    "id": "fakepty",
    "label": "Fake PTY",
    "executable": "fake-pty",
    "interactive_prompt_prefix_args": [],
    "non_interactive_prompt_prefix_args": [],
    "install_hint": "test fixture",
    "system_prompt_flag": null
  }]
}"#,
    )
    .expect("failed to write external adapter manifest");

    let mut paths = vec![fake_bin.clone()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    let path = std::env::join_paths(paths).expect("test PATH should be joinable");

    let out = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args(["run", "fakepty", "--stream-json", "--", "ping"])
        .current_dir(&project_root)
        .env("COVEN_HOME", &coven_home)
        .env("COVEN_HARNESS_ADAPTER_MANIFEST", &manifest)
        .env("PATH", &path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to spawn coven binary");

    assert!(
        out.status.success(),
        "coven run fakepty --stream-json failed: status={:?} stdout={} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let stdout = String::from_utf8(out.stdout).expect("stdout not utf-8");
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(
        lines.len() >= 3,
        "expected at least system + user + result frames; got {lines:?}",
    );

    // The core #307 contract: EVERY stdout line is valid JSON, no matter
    // what the harness wrote to its PTY.
    let frames: Vec<serde_json::Value> = lines
        .iter()
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|error| {
                panic!("stdout line is not valid JSON ({error}): {line:?}\nfull stdout:\n{stdout}")
            })
        })
        .collect();

    assert_eq!(frames[0]["type"], "system");
    assert_eq!(frames[0]["subtype"], "init");
    let session_id = frames[0]["session_id"]
        .as_str()
        .expect("system.init carries a session id")
        .to_string();

    assert_eq!(frames[1]["type"], "user");
    assert_eq!(frames[1]["message"]["content"][0]["text"], "ping");

    let last = frames.last().expect("at least one frame");
    assert_eq!(last["type"], "result");
    assert_eq!(last["subtype"], "success");
    assert_eq!(last["is_error"], false);

    // The PTY noise is not dropped: it rides inside `output` frames, raw
    // text preserved (ANSI escapes and the broken-JSON fragment included),
    // tagged with the session id.
    let output_text: String = frames
        .iter()
        .filter(|frame| frame["type"] == "output")
        .map(|frame| {
            assert_eq!(
                frame["session_id"], *session_id,
                "output frames carry the session id: {frame}"
            );
            frame["text"]
                .as_str()
                .expect("output frames carry raw text")
                .to_string()
        })
        .collect();
    assert!(
        output_text.contains("fake pty banner"),
        "captured PTY output should include the harness banner; got {output_text:?}",
    );
    assert!(
        output_text.contains("\u{1b}["),
        "ANSI escapes are preserved verbatim inside output frames; got {output_text:?}",
    );
    assert!(
        output_text.contains("{\"not\":\"terminated"),
        "broken-JSON PTY noise must ride inside output frames, not the stream; got {output_text:?}",
    );
    assert!(
        output_text.contains("fake pty noise complete"),
        "captured PTY output should include the completion marker; got {output_text:?}",
    );
}

#[cfg(unix)]
#[test]
fn codex_json_stream_normalizes_assistant_and_thread_id() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home).expect("failed to create coven home");
    let project_root = temp_dir.path().join("project");
    fs::create_dir_all(&project_root).expect("failed to create project root");
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin).expect("failed to create fake bin dir");
    let fake_codex = fake_bin.join("codex");
    fs::write(
        &fake_codex,
        r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
printf '%s\n' '{"type":"thread.started","thread_id":"thread-unix-123"}'
printf '%s\n' '{"type":"turn.started"}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"reply for stream client"}}'
printf '%s\n' '{"type":"turn.completed"}'
"#,
    )
    .expect("failed to write fake codex");
    let mut permissions = fs::metadata(&fake_codex)
        .expect("failed to stat fake codex")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_codex, permissions).expect("failed to chmod fake codex");

    let mut paths = vec![fake_bin.clone()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    let path = std::env::join_paths(paths).expect("test PATH should be joinable");
    let out = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args([
            "run",
            "codex",
            "--stream-json",
            // Both values look like argv syntax. The Codex bridge must still
            // put its own flag after the real exec subcommand.
            "--model",
            "openai/exec",
            "--",
            "--json",
        ])
        .current_dir(&project_root)
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to spawn coven binary");

    assert!(
        out.status.success(),
        "coven run codex --stream-json failed: status={:?} stdout={} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).expect("stdout not utf-8");
    let frames: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("Coven stdout must remain JSONL"))
        .collect();
    assert_eq!(frames[0]["type"], "system");
    assert_eq!(frames[1]["type"], "user");
    assert_eq!(frames[1]["message"]["content"][0]["text"], "--json");
    let assistant = frames
        .iter()
        .find(|frame| frame["type"] == "assistant")
        .expect("Codex reply must be normalized to an assistant frame");
    assert_eq!(
        assistant["message"]["content"][0]["text"],
        "reply for stream client"
    );
    let result = frames.last().expect("result should be last");
    assert_eq!(result["type"], "result");
    assert_eq!(result["is_error"], false);
    assert_eq!(result["harness_session_id"], "thread-unix-123");
    assert!(
        !frames.iter().any(|frame| frame["type"] == "output"),
        "native Codex JSON must not be rewrapped as raw output"
    );
    assert_eq!(
        fs::read_to_string(project_root.join("args.txt")).expect("fake codex should record argv"),
        "--model\nexec\nexec\n--json\n--skip-git-repo-check\n--color\nnever\n--\n--json\n",
        "Codex must put its JSON option after the real exec subcommand"
    );
}

/// A Codex protocol failure is a failed Coven run even when the Codex wrapper
/// exits 0. The terminal CLI status and persisted ledger must agree so clients
/// never see a failed turn recorded as a successful process exit.
#[cfg(unix)]
#[test]
fn codex_json_turn_failure_with_zero_exit_marks_ledger_failed() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home).expect("failed to create coven home");
    let project_root = temp_dir.path().join("project");
    fs::create_dir_all(&project_root).expect("failed to create project root");
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin).expect("failed to create fake bin dir");
    let fake_codex = fake_bin.join("codex");
    fs::write(
        &fake_codex,
        r#"#!/bin/sh
printf '%s\n' '{"type":"thread.started","thread_id":"thread-failed-zero"}'
printf '%s\n' '{"type":"turn.failed","error":{"message":"fixture turn failure"}}'
exit 0
"#,
    )
    .expect("failed to write fake codex");
    let mut permissions = fs::metadata(&fake_codex)
        .expect("failed to stat fake codex")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_codex, permissions).expect("failed to chmod fake codex");

    let mut paths = vec![fake_bin];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    let path = std::env::join_paths(paths).expect("test PATH should be joinable");
    let out = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args(["run", "codex", "--stream-json", "--", "fail once"])
        .current_dir(&project_root)
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to spawn coven binary");

    assert!(
        !out.status.success(),
        "protocol failure must return non-zero: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).expect("stdout not utf-8");
    let frames: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("Coven stdout must remain JSONL"))
        .collect();
    let result = frames.last().expect("result frame should be last");
    assert_eq!(result["type"], "result");
    assert_eq!(result["is_error"], true);
    assert_eq!(result["error"], "fixture turn failure");

    let session_id = frames[0]["session_id"]
        .as_str()
        .expect("system frame carries stable Coven id");
    let conn = rusqlite::Connection::open(coven_home.join("coven.sqlite3"))
        .expect("failed to open Coven session ledger");
    let (status, exit_code): (String, Option<i32>) = conn
        .query_row(
            "SELECT status, exit_code FROM sessions WHERE id = ?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("failed session should remain in the ledger");
    assert_eq!(status, "failed");
    assert_eq!(exit_code, Some(1));
}

/// Cancelling Coven itself must also reap the separate Unix Codex session
/// group, then emit the terminal result that lets a bridge mark the session
/// failed instead of leaving it `running` forever.
#[cfg(unix)]
#[test]
fn codex_json_sigterm_reaps_descendants_and_marks_ledger_failed() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home).expect("failed to create coven home");
    let project_root = temp_dir.path().join("project");
    fs::create_dir_all(&project_root).expect("failed to create project root");
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin).expect("failed to create fake bin dir");
    let fake_codex = fake_bin.join("codex");
    fs::write(
        &fake_codex,
        r#"#!/bin/sh
sleep 10 </dev/null >/dev/null 2>&1 &
echo $! > descendant.pid
while :; do sleep 1; done
"#,
    )
    .expect("failed to write fake codex");
    let mut permissions = fs::metadata(&fake_codex)
        .expect("failed to stat fake codex")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_codex, permissions).expect("failed to chmod fake codex");

    let mut paths = vec![fake_bin];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    let path = std::env::join_paths(paths).expect("test PATH should be joinable");
    let mut coven = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args([
            "run",
            "codex",
            "--stream-json",
            "--",
            "wait for cancellation",
        ])
        .current_dir(&project_root)
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn coven binary");

    let descendant_path = project_root.join("descendant.pid");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !descendant_path.exists() && Instant::now() < deadline {
        if let Some(status) = coven.try_wait().expect("failed polling coven") {
            panic!("coven exited before Codex fixture was ready: {status}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        descendant_path.exists(),
        "Codex fixture did not start before cancellation"
    );
    let signal_result = unsafe { libc::kill(coven.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(
        signal_result,
        0,
        "failed to signal the Coven process: {}",
        std::io::Error::last_os_error()
    );
    let out = coven
        .wait_with_output()
        .expect("failed waiting for cancelled coven");

    assert!(
        !out.status.success(),
        "cancellation must return non-zero: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).expect("stdout not utf-8");
    let frames: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("Coven stdout must remain JSONL"))
        .collect();
    let result = frames.last().expect("result frame should be last");
    assert_eq!(result["type"], "result");
    assert_eq!(result["is_error"], true);
    assert!(
        result["error"]
            .as_str()
            .is_some_and(|error| error.contains("cancelled by SIGTERM")),
        "cancellation detail should reach the client protocol: {result}"
    );

    let session_id = frames[0]["session_id"]
        .as_str()
        .expect("system frame carries stable Coven id");
    let conn = rusqlite::Connection::open(coven_home.join("coven.sqlite3"))
        .expect("failed to open Coven session ledger");
    let (status, exit_code): (String, Option<i32>) = conn
        .query_row(
            "SELECT status, exit_code FROM sessions WHERE id = ?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("cancelled session should remain in the ledger");
    assert_eq!(status, "failed");
    assert_eq!(exit_code, Some(1));

    let descendant_pid = fs::read_to_string(&descendant_path)
        .expect("failed to read descendant pid")
        .trim()
        .to_string();
    let mut alive = true;
    for _ in 0..80 {
        alive = Command::new("kill")
            .args(["-0", &descendant_pid])
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !alive {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        !alive,
        "cancelled Codex descendant {descendant_pid} should be reaped"
    );
}

/// End-to-end Windows regression: an npm-style `codex.cmd` must receive a
/// multiline prompt through stdin (not ConPTY/cmd argv), and the Coven CLI
/// must surface the Codex JSON response as an `assistant` frame. The second
/// invocation proves Cave can keep using the stable Coven ledger id while
/// Coven resumes the native Codex thread internally.
#[cfg(windows)]
#[test]
fn windows_codex_cmd_stream_json_emits_assistant_and_resumes_native_thread() {
    use std::fs;

    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home).expect("failed to create coven home");
    fs::write(
        coven_home.join("familiars.toml"),
        r#"[[familiar]]
id = "briar"
display_name = "Briar"
role = "Code and diagnostics"
description = "Windows Codex regression fixture"
"#,
    )
    .expect("failed to write familiar fixture");
    let project_root = temp_dir.path().join("project");
    fs::create_dir_all(&project_root).expect("failed to create project root");
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin).expect("failed to create fake bin dir");
    let fake_codex = fake_bin.join("codex.cmd");
    fs::write(
        &fake_codex,
        concat!(
            "@echo off\r\n",
            // findstr copies stdin without PowerShell's multi-second cold
            // start, which flaked the sibling pty_runner test (issue #407).
            "\"%SystemRoot%\\System32\\findstr.exe\" \"^\" > stdin.txt\r\n",
            "echo %* > args.txt\r\n",
            "echo {\"type\":\"thread.started\",\"thread_id\":\"thread-789\"}\r\n",
            "echo {\"type\":\"turn.started\"}\r\n",
            "echo {\"type\":\"item.completed\",\"item\":{\"id\":\"item-1\",\"type\":\"agent_message\",\"text\":\"reply for Cave\"}}\r\n",
            "echo {\"type\":\"turn.completed\"}\r\n",
            "exit /b 0\r\n"
        ),
    )
    .expect("failed to write fake codex cmd");

    let mut paths = vec![fake_bin.clone()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    let path = std::env::join_paths(paths).expect("test PATH should be joinable");
    let prompt = "first line\nsecond line";
    let first = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args([
            "run",
            "codex",
            "--stream-json",
            "--familiar",
            "briar",
            "--",
            prompt,
        ])
        .current_dir(&project_root)
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .env("PATHEXT", ".CMD")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to spawn coven binary");

    assert!(
        first.status.success(),
        "first Coven run failed: status={:?} stdout={} stderr={}",
        first.status,
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr),
    );
    let first_stdout = String::from_utf8(first.stdout).expect("stdout not utf-8");
    let frames: Vec<serde_json::Value> = first_stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("Coven stdout must remain JSONL"))
        .collect();
    let ledger_id = frames[0]["session_id"]
        .as_str()
        .expect("system frame carries stable Coven id")
        .to_string();
    let assistant = frames
        .iter()
        .find(|frame| frame["type"] == "assistant")
        .expect("Codex message must reach Coven assistant protocol");
    assert_eq!(assistant["message"]["content"][0]["text"], "reply for Cave");
    let result = frames.last().expect("result frame should be last");
    assert_eq!(result["type"], "result");
    assert_eq!(result["is_error"], false);
    assert_eq!(result["harness_session_id"], "thread-789");
    let first_args =
        fs::read_to_string(project_root.join("args.txt")).expect("fake cmd should record argv");
    assert!(
        first_args.contains("--json"),
        "expected Codex JSON flag: {first_args:?}"
    );
    assert!(
        !first_args.contains("first line") && !first_args.contains("second line"),
        "prompt must be kept out of cmd.exe argv: {first_args:?}"
    );
    let stdin = fs::read_to_string(project_root.join("stdin.txt"))
        .expect("fake cmd should record stdin prompt");
    assert!(
        stdin.contains("[Identity: You are Briar, a Code and diagnostics. Respond as Briar, not as the underlying tool.]"),
        "familiar preamble should reach Codex exactly once: stdin={stdin:?}, argv={first_args:?}, stream={first_stdout:?}"
    );
    assert_eq!(
        stdin.matches("[Identity: You are Briar").count(),
        1,
        "familiar preamble must not be duplicated: {stdin:?}"
    );
    assert!(
        stdin.contains("first line"),
        "first prompt line missing: {stdin:?}"
    );
    assert!(
        stdin.contains("second line"),
        "second prompt line missing: {stdin:?}"
    );

    let second = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args([
            "run",
            "codex",
            "--stream-json",
            "--continue",
            &ledger_id,
            "--",
            "follow-up",
        ])
        .current_dir(&project_root)
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .env("PATHEXT", ".CMD")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to spawn resumed coven binary");
    assert!(
        second.status.success(),
        "resumed Coven run failed: status={:?} stdout={} stderr={}",
        second.status,
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr),
    );
    let resumed_args = fs::read_to_string(project_root.join("args.txt"))
        .expect("fake cmd should record resumed argv");
    assert!(
        resumed_args.contains("resume thread-789"),
        "stable Coven id should resolve to the native Codex thread: {resumed_args:?}"
    );
}

/// The no-output failure is exercised through the actual CLI, not just the
/// runner, because CovenCave's observed failure mode was a session that stayed
/// `running` forever with no terminal stream frame. The debug-only timeout
/// override keeps this integration regression bounded.
#[cfg(windows)]
#[test]
fn windows_silent_codex_cmd_emits_terminal_error_and_marks_session_failed() {
    use std::fs;

    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home).expect("failed to create coven home");
    let project_root = temp_dir.path().join("project");
    fs::create_dir_all(&project_root).expect("failed to create project root");
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin).expect("failed to create fake bin dir");
    fs::write(
        fake_bin.join("codex.cmd"),
        "@echo off\r\n:spin\r\ngoto spin\r\n",
    )
    .expect("failed to write silent codex cmd");

    let mut paths = vec![fake_bin];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    let path = std::env::join_paths(paths).expect("test PATH should be joinable");
    let started = std::time::Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args(["run", "codex", "--stream-json", "--", "wait forever"])
        .current_dir(&project_root)
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .env("PATHEXT", ".CMD")
        .env("COVEN_TEST_CODEX_JSON_IDLE_TIMEOUT_MS", "150")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to spawn coven binary");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "silent npm shim should be cancelled promptly"
    );
    assert!(
        !out.status.success(),
        "timeout must return a non-zero CLI status: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).expect("stdout not utf-8");
    let frames: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("Coven stdout must remain JSONL"))
        .collect();
    assert_eq!(frames[0]["type"], "system");
    assert_eq!(frames[1]["type"], "user");
    assert!(
        !frames.iter().any(|frame| frame["type"] == "assistant"),
        "a silent Codex turn must not invent an assistant reply: {frames:?}"
    );
    let result = frames.last().expect("result frame should be last");
    assert_eq!(result["type"], "result");
    assert_eq!(result["is_error"], true);
    assert!(
        result["error"]
            .as_str()
            .is_some_and(|error| error.contains("machine-readable activity")),
        "timeout detail should reach the client protocol: {result}"
    );

    let session_id = frames[0]["session_id"]
        .as_str()
        .expect("system frame carries stable Coven id");
    let conn = rusqlite::Connection::open(coven_home.join("coven.sqlite3"))
        .expect("failed to open Coven session ledger");
    let (status, exit_code): (String, Option<i32>) = conn
        .query_row(
            "SELECT status, exit_code FROM sessions WHERE id = ?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("timed-out session should remain in the ledger");
    assert_eq!(status, "failed", "silent session must not remain running");
    assert!(
        exit_code.is_some_and(|code| code != 0),
        "timed-out session must persist a non-zero exit code: {exit_code:?}"
    );
}

#[test]
#[ignore = "requires built coven binary; run with `cargo test -- --ignored`"]
fn stream_json_emits_init_and_result_for_codex_dry_run() {
    let out = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args(["run", "codex", "--stream-json", "--detach", "ping"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to spawn coven binary");

    assert!(
        out.status.success(),
        "coven run --detach --stream-json failed: status={:?} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
    );

    let stdout = String::from_utf8(out.stdout).expect("stdout not utf-8");
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(
        lines.len() >= 3,
        "expected at least 3 JSONL frames (system, user, result); got {lines:?}",
    );

    let first: serde_json::Value =
        serde_json::from_str(lines[0]).expect("first line is not valid JSON");
    assert_eq!(first["type"], "system");
    assert_eq!(first["subtype"], "init");
    assert!(first["session_id"].is_string());
    assert!(first["cwd"].is_string());

    let second: serde_json::Value =
        serde_json::from_str(lines[1]).expect("second line is not valid JSON");
    assert_eq!(second["type"], "user");
    assert_eq!(second["message"]["role"], "user");
    assert_eq!(second["message"]["content"][0]["type"], "text");
    assert_eq!(second["message"]["content"][0]["text"], "ping");

    let last: serde_json::Value =
        serde_json::from_str(lines.last().unwrap()).expect("last line is not valid JSON");
    assert_eq!(last["type"], "result");
    assert_eq!(last["subtype"], "success");
    assert_eq!(last["is_error"], false);

    // No --model passed: system.init carries a null model field.
    assert!(first["model"].is_null(), "model defaults to null: {first}");
}

#[test]
#[ignore = "requires built coven binary; run with `cargo test -- --ignored`"]
fn stream_json_init_echoes_requested_model_verbatim() {
    let out = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args([
            "run",
            "codex",
            "--stream-json",
            "--detach",
            "--model",
            "openai/gpt-5.5",
            "ping",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to spawn coven binary");

    assert!(
        out.status.success(),
        "coven run --detach --stream-json --model failed: status={:?} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
    );

    let stdout = String::from_utf8(out.stdout).expect("stdout not utf-8");
    let first_line = stdout
        .lines()
        .find(|l| !l.trim().is_empty())
        .expect("expected at least one frame");
    let first: serde_json::Value =
        serde_json::from_str(first_line).expect("first line is not valid JSON");
    assert_eq!(first["type"], "system");
    assert_eq!(first["subtype"], "init");
    // system.init echoes the requested id verbatim (namespaced form preserved)
    // so Cave can confirm acceptance with an exact match.
    assert_eq!(first["model"], "openai/gpt-5.5");
}
