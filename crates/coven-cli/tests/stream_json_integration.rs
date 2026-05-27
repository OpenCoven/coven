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
}
