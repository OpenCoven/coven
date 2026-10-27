//! Scoped read launches through the live runtime (#1603; gate proposal §4,
//! tests 8 and 9).

use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::*;
use crate::automations::runtime_envelope::CLAUDE_R0_SCOPED_READ;
use crate::automations::scoped_read::channel::{relay, CHANNEL_ENV, TOKEN_ENV};
use crate::automations::trusted_launch::test_support::{r0, read, scope, Fixture, GRANTED};

fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Makes `program` a stand-in claude 2.1.305. Launched, it records its
/// arguments, its working folder, that folder's entries and the names in its
/// environment under `capture`, then waits until `capture/release` exists.
fn stand_in(program: &Path, capture: &Path) {
    let capture = capture.display();
    fs::write(
        program,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo '2.1.305 (Claude Code)'; exit 0; fi\n\
             printf '%s\\n' \"$@\" > '{capture}/args'\n\
             pwd -P > '{capture}/cwd'\n\
             ls -A > '{capture}/entries'\n\
             env | sed 's/=.*//' > '{capture}/environment'\n\
             touch '{capture}/started'\n\
             while [ ! -e '{capture}/release' ]; do sleep 0.05; done\n"
        ),
    )
    .unwrap();
}

/// The text of each tool result in `output`, after the initialize reply.
fn tool_results(output: &[u8]) -> Vec<Value> {
    String::from_utf8(output.to_vec())
        .unwrap()
        .lines()
        .skip(1)
        .map(|line| {
            let reply: Value = serde_json::from_str(line).unwrap();
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
        })
        .collect()
}

#[test]
fn a_scoped_read_launch_runs_under_the_store_gate_in_an_empty_folder() {
    let fixture = Fixture::new();
    let capture = fixture
        .program
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("capture");
    fs::create_dir_all(&capture).unwrap();
    stand_in(&fixture.program, &capture);
    fixture.create("notes", r0(vec![scope("notes/today.md", false)]), "ACTIVE");
    fixture.schedule("notes");

    // The real runtime: preflight, then launch through trusted_launch.
    let runtime = LiveSessionRuntime::with_coven_home(fixture.home.path().to_path_buf())
        .with_scoped_harness(fixture.program.clone());
    let (report, runtime) = fixture.dispatch_pass_with(runtime);
    assert_eq!(report.dispatched.len(), 1, "{:?}", report.failed);
    wait_for("the harness to start", || capture.join("started").exists());

    // The harness reaches the service only through its MCP configuration.
    let args = fs::read_to_string(capture.join("args")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    let config = args
        .iter()
        .position(|arg| *arg == "--mcp-config")
        .map(|index| PathBuf::from(args[index + 1]))
        .expect("the harness is pointed at its channel");
    let config: Value = serde_json::from_slice(&fs::read(config).unwrap()).unwrap();
    let server = &config["mcpServers"]["coven"];
    let socket = PathBuf::from(server["env"][CHANNEL_ENV].as_str().unwrap());
    let channel = socket.parent().unwrap().to_path_buf();
    let mut input = Vec::new();
    for message in [
        json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize",
                "params": { "protocolVersion": "2025-06-18" } }),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "read_file", "arguments": read("notes/today.md") } }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "read_file", "arguments": read("note.txt") } }),
    ] {
        input.extend(serde_json::to_vec(&message).unwrap());
        input.push(b'\n');
    }
    let mut output = Vec::new();
    relay(
        &socket,
        server["env"][TOKEN_ENV].as_str().unwrap(),
        Cursor::new(input),
        &mut output,
    )
    .expect("the relay runs");
    let results = tool_results(&output);
    assert_eq!(results[0]["content"], GRANTED);
    assert_eq!(results[1]["refusal"], "scope_missing");

    // The harness ran in an empty folder outside the project, with exactly
    // the descriptor's environment (the shell adds its own few names).
    let cwd = PathBuf::from(fs::read_to_string(capture.join("cwd")).unwrap().trim());
    assert_eq!(fs::read_to_string(capture.join("entries")).unwrap(), "");
    assert!(!cwd.starts_with(fixture.project.canonicalize().unwrap()));
    let shell = ["PWD", "OLDPWD", "SHLVL", "_"];
    let names: BTreeSet<String> = fs::read_to_string(capture.join("environment"))
        .unwrap()
        .lines()
        .filter(|name| !shell.contains(name))
        .map(str::to_owned)
        .collect();
    let expected: BTreeSet<String> = CLAUDE_R0_SCOPED_READ
        .service
        .as_ref()
        .unwrap()
        .environment
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(names, expected);

    // Both reads were committed, and at session end the channel and the
    // launch folder are gone.
    fs::write(capture.join("release"), "").unwrap();
    let launch_root = cwd.parent().unwrap().to_path_buf();
    wait_for("the channel and the launch folder to go", || {
        !channel.exists() && !launch_root.exists()
    });
    let rows: Vec<(i64, String, Option<String>)> = fixture
        .home
        .conn
        .prepare(
            "SELECT sequence, outcome, refusal FROM automation_scoped_read_commitments
             ORDER BY sequence",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        rows,
        [
            (1, "served".to_owned(), None),
            (2, "refused".to_owned(), Some("scope_missing".to_owned())),
        ]
    );
    drop(runtime);
}

#[test]
fn the_args_only_path_refuses_a_scoped_envelope() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = LiveSessionRuntime::with_coven_home(temp.path().to_path_buf());
    // A folder that does not exist, so nothing could start even if the
    // envelope were wrongly accepted.
    let cwd = temp.path().join("missing").display().to_string();
    let launch = SessionLaunch {
        id: "session-args-only".to_string(),
        project_root: cwd.clone(),
        cwd,
        harness: "claude".to_string(),
        model: None,
        launch_mode: crate::harness::HarnessLaunchMode::NonInteractive,
        launch_policy: None,
        prompt: "Read the notes.".to_string(),
        title: "args only".to_string(),
        conversation: None,
        conversation_id: None,
        familiar_id: None,
        caller_familiar_id: None,
    };
    let error = runtime
        .launch_session_with_envelope(
            &launch,
            None,
            None,
            true,
            Some(EnvelopeLaunch::Args(&CLAUDE_R0_SCOPED_READ)),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains(crate::api::RUNTIME_AUTHORITY_PROJECTION_UNSUPPORTED),
        "{error:#}"
    );
    assert!(runtime.sessions.lock().unwrap().is_empty());
}
