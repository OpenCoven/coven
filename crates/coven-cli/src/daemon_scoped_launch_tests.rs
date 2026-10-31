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
/// environment under `capture`, then waits until `capture/release` exists and
/// prints `capture/stream`, if there is one, as its output.
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
             for n in 3 4 5 6 7 8 9; do if (: <&$n) 2>/dev/null || (: >&$n) 2>/dev/null; then echo $n; fi; done > '{capture}/fds'\n\
             touch '{capture}/started'\n\
             while [ ! -e '{capture}/release' ]; do sleep 0.05; done\n\
             if [ -e '{capture}/stream' ]; then cat '{capture}/stream'; fi\n"
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
    // It inherited no descriptor but its standard streams: duplicating
    // each of 3 to 9 fails (#1608). The shell's own descriptors sit at 10
    // and above.
    assert_eq!(fs::read_to_string(capture.join("fds")).unwrap(), "");

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

#[test]
fn a_launch_refused_after_consumption_records_its_profile_code() {
    let fixture = Fixture::new();
    // A project inside COVEN_HOME passes dispatch, but admission refuses it
    // (`root_protected`) only once the decision is consumed.
    let inside = fixture.home.path().join("inside");
    fs::create_dir_all(inside.join("notes")).unwrap();
    fs::write(inside.join("notes/today.md"), GRANTED).unwrap();
    fixture.create_in(
        "notes",
        r0(vec![scope("notes/today.md", false)]),
        "ACTIVE",
        inside.to_str().unwrap(),
    );
    let occurrence = fixture.schedule("notes");
    let runtime = LiveSessionRuntime::with_coven_home(fixture.home.path().to_path_buf())
        .with_scoped_harness(fixture.program.clone());
    let (report, _runtime) = fixture.dispatch_pass_with(runtime);
    assert!(report.dispatched.is_empty(), "{report:?}");
    assert_eq!(
        fixture.threads_rows(),
        (1, 1, 1),
        "the decision was consumed"
    );

    let expected = "automation authority refused dispatch: AUTHORITY_RUNTIME_DOWNGRADE";
    let (class, reason): (String, String) = fixture
        .home
        .conn
        .query_row(
            "SELECT failure_class, state_reason FROM automation_attempts
             WHERE occurrence_id = ?1",
            [&occurrence],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (class.as_str(), reason.as_str()),
        ("launch_refused", expected)
    );
    let recorded: String = fixture
        .home
        .conn
        .query_row(
            "SELECT failure_reason FROM automation_occurrences WHERE id = ?1",
            [&occurrence],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(recorded, expected);
}

/// A scoped read run through the live runtime whose harness prints `stream`
/// as its output after the test reads the granted note and the sibling
/// through its channel. Returns once the session has ended and its terminal
/// evidence is stored.
fn scoped_run_ending_with(fixture: &Fixture, stream: &[String]) -> LiveSessionRuntime {
    scoped_run(
        fixture,
        stream,
        |_| {},
        |results| {
            assert_eq!(results[0]["content"], GRANTED);
            assert_eq!(results[1]["refusal"], "scope_missing");
        },
    )
}

/// [`scoped_run_ending_with`], with `before_reads` run once the harness has
/// started and before the reads, and `check` given the reads' results.
fn scoped_run(
    fixture: &Fixture,
    stream: &[String],
    before_reads: impl FnOnce(&Fixture),
    check: impl FnOnce(&[Value]),
) -> LiveSessionRuntime {
    let capture = fixture
        .program
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("capture");
    fs::create_dir_all(&capture).unwrap();
    stand_in(&fixture.program, &capture);
    fs::write(
        capture.join("stream"),
        crate::automations::stream_observation::test_support::stream(stream),
    )
    .unwrap();
    fixture.create("notes", r0(vec![scope("notes/today.md", false)]), "ACTIVE");
    fixture.schedule("notes");
    let runtime = LiveSessionRuntime::with_coven_home(fixture.home.path().to_path_buf())
        .with_scoped_harness(fixture.program.clone());
    let (report, runtime) = fixture.dispatch_pass_with(runtime);
    assert_eq!(report.dispatched.len(), 1, "{:?}", report.failed);
    wait_for("the harness to start", || capture.join("started").exists());
    let args = fs::read_to_string(capture.join("args")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    let config = args
        .iter()
        .position(|arg| *arg == "--mcp-config")
        .map(|index| PathBuf::from(args[index + 1]))
        .unwrap();
    let config: Value = serde_json::from_slice(&fs::read(config).unwrap()).unwrap();
    let server = &config["mcpServers"]["coven"];
    before_reads(fixture);
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
        Path::new(server["env"][CHANNEL_ENV].as_str().unwrap()),
        server["env"][TOKEN_ENV].as_str().unwrap(),
        Cursor::new(input),
        &mut output,
    )
    .expect("the relay runs");
    check(&tool_results(&output));
    fs::write(capture.join("release"), "").unwrap();
    wait_for("the session's terminal evidence", || {
        fixture
            .home
            .conn
            .query_row(
                "SELECT COUNT(*) FROM automation_runtime_terminal_evidence",
                [],
                |row| row.get::<_, i64>(0),
            )
            .is_ok_and(|stored| stored == 1)
    });
    runtime
}

/// The stream a harness that made both reads prints.
fn two_reads() -> Vec<String> {
    crate::automations::stream_observation::test_support::scoped_read_run_calling(2)
}

fn run_state(fixture: &Fixture) -> (String, Option<String>, Option<String>) {
    fixture
        .home
        .conn
        .query_row(
            "SELECT r.status, r.receipt_id, a.state_reason
             FROM automation_runs AS r
             JOIN automation_attempts AS a ON a.run_id = r.id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
}

#[test]
fn a_scoped_read_run_whose_reads_reconcile_settles_from_its_signed_evidence() {
    let fixture = Fixture::new();
    let runtime = scoped_run_ending_with(&fixture, &two_reads());
    let report =
        crate::automations::runner::settle_finished_runs(&fixture.home.conn, chrono::Utc::now())
            .unwrap();
    assert_eq!(report.succeeded, 1, "{report:?}: {:?}", run_state(&fixture));
    let (status, receipt, _) = run_state(&fixture);
    assert_eq!(status, "succeeded");
    assert!(receipt.is_some(), "the launched receipt committed");
    drop(runtime);
}

#[test]
fn a_scoped_read_run_whose_stream_hides_a_read_is_held() {
    let fixture = Fixture::new();
    // The harness reports one read; the service served two.
    let runtime = scoped_run_ending_with(
        &fixture,
        &crate::automations::stream_observation::test_support::scoped_read_run(),
    );
    let report =
        crate::automations::runner::settle_finished_runs(&fixture.home.conn, chrono::Utc::now())
            .unwrap();
    assert_eq!(report.succeeded, 0, "{report:?}");
    let (status, receipt, reason) = run_state(&fixture);
    assert_eq!(status, "running");
    assert!(receipt.is_none());
    assert!(
        reason
            .as_deref()
            .is_some_and(|reason| reason.contains("recovery required")),
        "{reason:?}"
    );
    drop(runtime);
}

#[test]
fn a_commitment_no_closed_channel_accounts_for_holds_settlement() {
    let fixture = Fixture::new();
    let runtime = scoped_run_ending_with(&fixture, &two_reads());
    // A commitment appears for the attempt after its evidence was signed
    // complete, from a channel that never closed.
    let attempt: String = fixture
        .home
        .conn
        .query_row("SELECT id FROM automation_attempts", [], |row| row.get(0))
        .unwrap();
    fixture
        .home
        .conn
        .execute(
            "INSERT INTO automation_scoped_read_commitments (
                attempt_id, channel_id, sequence, tool, outcome, refusal,
                result_digest, result_bytes, committed_at
             ) VALUES (?1, 'forged', 1, 'read_file', 'served', NULL, ?2, 1,
                       '2026-10-30T00:00:00.000Z')",
            rusqlite::params![attempt, "ab".repeat(32)],
        )
        .unwrap();
    let report =
        crate::automations::runner::settle_finished_runs(&fixture.home.conn, chrono::Utc::now())
            .unwrap();
    assert_eq!(report.succeeded, 0, "{report:?}");
    let (status, receipt, reason) = run_state(&fixture);
    assert_eq!(status, "running");
    assert!(receipt.is_none());
    assert!(
        reason
            .as_deref()
            .is_some_and(|reason| reason.contains("scoped read commitments do not reconcile")),
        "{reason:?}"
    );
    drop(runtime);
}

/// Authority withdrawn or expired while the harness runs, before it reads
/// (#1608, work breakdown 4). The gate withholds the read and stops the
/// service, no byte of the note reaches the harness, and settlement holds
/// the run for recovery: the harness's stream claims both reads, and the
/// authority a success would rest on is gone.
#[test]
fn a_run_whose_authority_ends_while_it_runs_reads_nothing_and_never_succeeds() {
    type End = (&'static str, fn(&Fixture));
    let ends: [End; 3] = [
        ("familiar revision revoked", |fixture| {
            fixture
                .home
                .conn
                .execute(
                    "UPDATE familiar_ledger_revisions
                     SET status = 'revoked', revoked_at = '2026-10-30T00:00:00.000Z',
                         revocation_reason = 'compromised'",
                    [],
                )
                .unwrap();
        }),
        ("definition paused", |fixture| {
            fixture
                .home
                .conn
                .execute(
                    "UPDATE automation_definitions SET lifecycle_state = 'paused'",
                    [],
                )
                .unwrap();
        }),
        ("run timed out", |fixture| {
            let past = (chrono::Utc::now() - chrono::TimeDelta::seconds(1))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            fixture
                .home
                .conn
                .execute("UPDATE automation_runs SET timeout_at = ?1", [past])
                .unwrap();
        }),
    ];
    for (name, end) in ends {
        let fixture = Fixture::new();
        let runtime = scoped_run(&fixture, &two_reads(), end, |results| {
            assert_eq!(results.len(), 1, "{name}: the service stops: {results:?}");
            assert!(
                results[0]["refusal"].is_string() && results[0].get("content").is_none(),
                "{name}: {results:?}"
            );
        });
        let served: i64 = fixture
            .home
            .conn
            .query_row(
                "SELECT COUNT(*) FROM automation_scoped_read_commitments WHERE outcome = 'served'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(served, 0, "{name}: nothing was authorized");
        let report = crate::automations::runner::settle_finished_runs(
            &fixture.home.conn,
            chrono::Utc::now(),
        )
        .unwrap();
        let (status, receipt, reason) = run_state(&fixture);
        assert_eq!(report.succeeded, 0, "{name}: {report:?}");
        assert_eq!(status, "running", "{name}: held, not settled");
        assert!(receipt.is_none(), "{name}: no receipt");
        assert!(
            reason
                .as_deref()
                .is_some_and(|reason| reason.contains("a scoped read was withheld")),
            "{name}: {reason:?}"
        );
        drop(runtime);
    }
}
