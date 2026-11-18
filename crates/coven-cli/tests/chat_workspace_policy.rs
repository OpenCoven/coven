//! No providers or user stores: an unqualified policy must stop before launch.
use serde_json::json;
use std::{fs, process::Command};

#[test]
fn unqualified_workspace_policy_stops_new_continued_and_retried_runs_before_launch() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let workspace = base.join("workspace");
    fs::create_dir(&workspace).unwrap();
    let policy = base.join("policy.json");
    fs::write(&policy, json!({"version":1,"runId":"r1","access":"workspace-write","primaryRoot":workspace,"writableRoots":[workspace],"readOnlyRoots":[]}).to_string()).unwrap();
    for resumed in [false, true, false] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
        command
            .env("COVEN_HOME", base.join("coven"))
            .env("COVEN_CAVE_HOME", base.join("cave"))
            .args([
                "run",
                "codex",
                "--stream-json",
                "--chat-workspace-policy-v1",
            ])
            .arg(&policy)
            .arg("--cwd")
            .arg(&workspace);
        if resumed {
            command.args(["--continue", "saved-session"]);
        }
        let output = command
            .args(["--", "Create a file in the workspace"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("cannot verify workspace write access"),
            "{error}"
        );
        let sessions = Command::new(env!("CARGO_BIN_EXE_coven"))
            .env("COVEN_HOME", base.join("coven"))
            .env("COVEN_CAVE_HOME", base.join("cave"))
            .args(["sessions", "--all", "--json"])
            .output()
            .unwrap();
        assert!(sessions.status.success());
        let sessions: serde_json::Value = serde_json::from_slice(&sessions.stdout).unwrap();
        assert_eq!(
            sessions["sessions"],
            json!([]),
            "rejected access must not admit a session"
        );
        assert_eq!(fs::read_dir(&workspace).unwrap().count(), 0);
    }
}

#[test]
fn workspace_contract_cannot_be_combined_with_full_or_arbitrary_extra_directories() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    for extra in [["--permission", "full"], ["--add-dir", "/"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_coven"))
            .env("COVEN_HOME", base.join("coven"))
            .env("COVEN_CAVE_HOME", base.join("cave"))
            .args([
                "run",
                "codex",
                "--stream-json",
                "--chat-workspace-policy-v1",
            ])
            .arg(base.join("not-read.json"))
            .args(extra)
            .args(["--", "hello"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("without separate permission"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("\"subtype\":\"init\""));
    }
}
