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

    // Fake codex that behaves like a TUI-ish harness on a PTY: colored
    // banner, carriage-return progress updates, a partial line that looks
    // like broken JSON, then a completion marker. Before #307 all of this
    // leaked raw onto stdout between the JSONL frames.
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin).expect("failed to create fake bin dir");
    let fake_codex = fake_bin.join("codex");
    fs::write(
        &fake_codex,
        "#!/bin/sh\n\
         printf '\\033[1;32mfake codex banner\\033[0m\\n'\n\
         printf 'progress 1/2\\rprogress 2/2\\n'\n\
         printf '{\"not\":\"terminated'\n\
         printf '\\nfake codex noise complete\\n'\n",
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
        .args(["run", "codex", "--stream-json", "--", "ping"])
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
        output_text.contains("fake codex banner"),
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
        output_text.contains("fake codex noise complete"),
        "captured PTY output should include the completion marker; got {output_text:?}",
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
