//! Import through the real owner-local daemon, with disposable source files.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use coven_client::{DaemonClient, DaemonEndpoint};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use serde_json::{json, Value};

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Vectors {
    schema_version: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    case_id: String,
    route: Route,
    sources: Vec<Source>,
    expected: Expected,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Route {
    Legacy,
    V1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    name: String,
    contents: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expected {
    imported: Vec<String>,
    skipped: usize,
    failures: usize,
    definition_status: String,
    lifecycle_state: String,
    source_preserved: bool,
    preexisting_preserved: bool,
    replay_preserved: bool,
    restart_preserved: bool,
    occurrences: i64,
    runs: i64,
    attempts: i64,
}

fn validate(vector: &Value) -> Result<Vectors, &'static str> {
    let vectors: Vectors = serde_json::from_value(vector.clone()).map_err(|_| INVALID)?;
    if vectors.schema_version != "coven.automations.legacy-import-source-preservation-vectors.v1"
        || vectors.cases.len() != 2
    {
        return Err(INVALID);
    }
    let mut routes = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for case in &vectors.cases {
        let expected = &case.expected;
        if !super::valid_case_id(&case.case_id)
            || !ids.insert(&case.case_id)
            || !routes.insert(case.route)
            || case.sources.len() != 4
            || expected.imported != ["imported"]
            || expected.skipped > 4
            || expected.failures > 4
            || expected.definition_status != "PAUSED"
            || expected.lifecycle_state
                != if case.route == Route::Legacy {
                    "paused"
                } else {
                    "draft"
                }
            || !expected.source_preserved
            || !expected.preexisting_preserved
            || !expected.replay_preserved
            || !expected.restart_preserved
            || expected.occurrences != 0
            || expected.runs != 0
            || expected.attempts != 0
        {
            return Err(INVALID);
        }
        let mut names = BTreeSet::new();
        let mut source_ids = Vec::new();
        let mut imported_contents = None;
        for source in &case.sources {
            if !matches!(
                source.name.as_str(),
                "a-valid" | "b-duplicate" | "c-existing" | "d-unsupported"
            ) || !names.insert(&source.name)
                || source.contents.len() > 4096
            {
                return Err(INVALID);
            }
            let parsed: toml::Value = toml::from_str(&source.contents).map_err(|_| INVALID)?;
            let id = parsed
                .get("id")
                .and_then(toml::Value::as_str)
                .ok_or(INVALID)?;
            let schedule = parsed.get("rrule").and_then(toml::Value::as_str);
            if parsed.get("status").and_then(toml::Value::as_str) != Some("ACTIVE")
                || parsed
                    .get("prompt")
                    .and_then(toml::Value::as_str)
                    .is_none_or(str::is_empty)
                || schedule
                    != Some(if id == "unsupported" {
                        "FREQ=HOURLY"
                    } else {
                        "FREQ=DAILY;BYHOUR=9"
                    })
            {
                return Err(INVALID);
            }
            if id == "imported" {
                // Directory enumeration order must not select different definitions.
                if imported_contents.is_some_and(|contents| contents != &source.contents) {
                    return Err(INVALID);
                }
                imported_contents = Some(&source.contents);
            }
            source_ids.push(id.to_owned());
        }
        source_ids.sort();
        if source_ids != ["existing", "imported", "imported", "unsupported"] {
            return Err(INVALID);
        }
    }
    Ok(vectors)
}

pub(super) fn evaluate(vector: &Value) -> Result<(bool, Value), &'static str> {
    let vectors = validate(vector)?;
    let mut passed = true;
    let mut observations = Vec::new();
    for case in vectors.cases {
        let (case_passed, observation) = case_matches(&case).map_err(|_| FAILED)?;
        passed &= case_passed;
        observations.push(observation);
    }
    Ok((passed, json!(observations)))
}

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_daemon(root: &Path) -> anyhow::Result<(Daemon, DaemonClient)> {
    let home = root.join("d");
    let mut command = Command::new(std::env::current_exe()?);
    command.env_clear();
    // Windows process startup needs its OS directory. No ambient provider or
    // Coven configuration is passed into the disposable daemon.
    #[cfg(windows)]
    for key in ["SystemRoot", "WINDIR"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .args(["daemon", "serve"])
        .current_dir(root)
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("COVEN_HOME", &home)
        .env("TMPDIR", root)
        .env("TMP", root)
        .env("TEMP", root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut daemon = Daemon(command.spawn()?);
    let started = Instant::now();
    loop {
        if let Ok(endpoint) = DaemonEndpoint::discover(&home) {
            let mut client = DaemonClient::new(endpoint);
            if client.health().is_ok() {
                return Ok((daemon, client));
            }
        }
        // Hang guard, not a startup SLO: health proves readiness.
        anyhow::ensure!(
            daemon.0.try_wait()?.is_none() && started.elapsed() < Duration::from_secs(120),
            "fixture readiness failed after {:?}",
            started.elapsed()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn action(client: &mut DaemonClient, request: &Value) -> anyhow::Result<Value> {
    let response = client.request_json("POST", "/api/v1/actions", Some(request))?;
    anyhow::ensure!(response.status == 200, "fixture action failed");
    let body: Value = serde_json::from_slice(&response.body)?;
    anyhow::ensure!(
        body["ok"] == true
            && body["accepted"] == true
            && body["status"] == "completed"
            && body["action"] == request["action"],
        "fixture action was not completed"
    );
    Ok(body)
}

/// Elapsed time of each import request in a case, by step. Measurement only
/// (#1360, draft): no request or response content.
#[derive(Default)]
struct ImportTimes(BTreeMap<&'static str, u64>);

impl ImportTimes {
    fn action(
        &mut self,
        step: &'static str,
        client: &mut DaemonClient,
        request: &Value,
    ) -> anyhow::Result<Value> {
        let started = Instant::now();
        let result = action(client, request);
        let millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.0.insert(step, millis);
        result
    }
}

fn source_snapshot(root: &Path) -> anyhow::Result<BTreeMap<PathBuf, Option<Vec<u8>>>> {
    let mut snapshot = BTreeMap::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        anyhow::ensure!(entry.file_type()?.is_dir(), "unexpected source entry");
        let directory = PathBuf::from(entry.file_name());
        snapshot.insert(directory.clone(), None);
        for file in fs::read_dir(entry.path())? {
            let file = file?;
            anyhow::ensure!(file.file_type()?.is_file(), "unexpected source file");
            let key = directory.join(file.file_name());
            snapshot.insert(key, Some(fs::read(file.path())?));
        }
    }
    Ok(snapshot)
}

fn rows(conn: &Connection, table: &str) -> anyhow::Result<Vec<String>> {
    // All callers supply static table names from this fixture.
    query_rows(conn, &format!("SELECT * FROM {table}"))
}

fn query_rows(conn: &Connection, query: &str) -> anyhow::Result<Vec<String>> {
    let mut statement = conn.prepare(query)?;
    let columns = statement.column_count();
    let mut rows = statement
        .query_map([], |row| {
            (0..columns)
                .map(|i| row.get::<_, rusqlite::types::Value>(i))
                .collect::<rusqlite::Result<Vec<_>>>()
        })?
        .map(|row| row.map(|row| format!("{row:?}")))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.sort();
    Ok(rows)
}

fn state(conn: &Connection) -> anyhow::Result<Vec<Vec<String>>> {
    [
        "automation_definitions",
        "automation_events",
        "automation_command_adoptions",
        "automation_occurrences",
        "automation_runs",
        "automation_attempts",
    ]
    .into_iter()
    .map(|table| rows(conn, table))
    .collect()
}

fn case_matches(case: &Case) -> anyhow::Result<(bool, Value)> {
    let temp = super::scheduler_conformance_tempdir().map_err(anyhow::Error::msg)?;
    let source_root = temp.path().join(".codex/automations");
    for source in &case.sources {
        let directory = source_root.join(&source.name);
        fs::create_dir_all(&directory)?;
        fs::write(directory.join("automation.toml"), &source.contents)?;
    }
    let before_source = source_snapshot(&source_root)?;
    let (daemon, mut client) = start_daemon(temp.path())?;
    action(
        &mut client,
        &json!({
            "action":"coven.automations.definition.create.v1", "adoptionKey":"existing-definition",
            "definition":{"schemaVersion":1,"id":"existing","name":"Preexisting fixture",
                "status":"PAUSED","rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc",
                "misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code",
                "prompt":"preexisting synthetic sentinel"}
        }),
    )?;
    let store = temp.path().join("d/coven.sqlite3");
    let conn = Connection::open_with_flags(&store, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(Duration::from_secs(30))?;
    let existing = query_rows(
        &conn,
        "SELECT * FROM automation_definitions WHERE id='existing'",
    )?;
    let request = if case.route == Route::Legacy {
        json!({"action":"coven.automations.import"})
    } else {
        json!({"action":"coven.automations.legacy.import.v1", "source":"codex-automation-toml",
            "adoptionKey":"import-fixture", "dryRun":false})
    };
    let mut passed = true;
    let mut times = ImportTimes::default();
    let mut dry_run_preserved = None;
    if case.route == Route::V1 {
        let before = (
            rows(&conn, "automation_definitions")?,
            rows(&conn, "automation_events")?,
        );
        let mut dry = request.clone();
        dry["dryRun"] = json!(true);
        dry["adoptionKey"] = json!("dry-import-fixture");
        let result = times.action("dryRun", &mut client, &dry)?;
        let preserved = result["result"]["outcome"] == "committed"
            && result["result"]["result"]["dryRun"] == true
            && result["result"]["result"]["imported"] == json!(case.expected.imported)
            && before
                == (
                    rows(&conn, "automation_definitions")?,
                    rows(&conn, "automation_events")?,
                )
            && before_source == source_snapshot(&source_root)?;
        dry_run_preserved = Some(preserved);
        passed &= preserved;
    }
    let response = times.action("import", &mut client, &request)?;
    let command_outcome = if case.route == Route::Legacy {
        "legacy_completed"
    } else if response["result"]["outcome"] == "committed" {
        "committed"
    } else {
        "unexpected"
    };
    passed &= command_outcome != "unexpected";
    let report = if case.route == Route::Legacy {
        &response["event"]["payload"]
    } else {
        &response["result"]["result"]
    };
    passed &= report["imported"] == json!(case.expected.imported)
        && report["skipped"].as_array().map(Vec::len) == Some(case.expected.skipped)
        && report["failures"].as_array().map(Vec::len) == Some(case.expected.failures);
    let (status, lifecycle): (String, String) = conn.query_row(
        "SELECT status,lifecycle_state FROM automation_definitions WHERE id='imported'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let after_existing = query_rows(
        &conn,
        "SELECT * FROM automation_definitions WHERE id='existing'",
    )?;
    let preexisting_preserved = existing == after_existing;
    let mut source_preserved = before_source == source_snapshot(&source_root)?;
    passed &= status == case.expected.definition_status
        && lifecycle == case.expected.lifecycle_state
        && preexisting_preserved
        && source_preserved
        && rows(&conn, "automation_definitions")?.len() == 2;
    let mut work_counts = BTreeMap::new();
    for (table, expected) in [
        ("automation_occurrences", case.expected.occurrences),
        ("automation_runs", case.expected.runs),
        ("automation_attempts", case.expected.attempts),
    ] {
        let count = i64::try_from(rows(&conn, table)?.len())?;
        passed &= count == expected;
        work_counts.insert(table, count);
    }
    let committed = state(&conn)?;
    let replay = times.action("replay", &mut client, &request)?;
    let mut replay_preserved = committed == state(&conn)?;
    if case.route == Route::V1 {
        replay_preserved &= replay["result"]["outcome"] == "replayed"
            && replay["result"]["result"] == response["result"]["result"];
    } else {
        replay_preserved &= replay["event"]["payload"]["imported"] == json!([]);
    }
    source_preserved &= before_source == source_snapshot(&source_root)?;
    passed &= replay_preserved && source_preserved;
    drop(client);
    drop(conn);
    drop(daemon);
    let (_restarted, mut client) = start_daemon(temp.path())?;
    let reopened = Connection::open_with_flags(&store, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    reopened.busy_timeout(Duration::from_secs(30))?;
    let mut restart_preserved = committed == state(&reopened)?;
    source_preserved &= before_source == source_snapshot(&source_root)?;
    let replay = times.action("restartReplay", &mut client, &request)?;
    if case.route == Route::V1 {
        restart_preserved &= replay["result"]["outcome"] == "replayed"
            && replay["result"]["result"] == response["result"]["result"];
    }
    restart_preserved &= committed == state(&reopened)?;
    source_preserved &= before_source == source_snapshot(&source_root)?;
    passed &= restart_preserved && source_preserved;
    // Only fixed classifications, booleans, counts, elapsed milliseconds and
    // caller-supplied case IDs leave this fixture. Never copy raw daemon
    // reports, prompts or paths.
    let status = match status.as_str() {
        "ACTIVE" => "ACTIVE",
        "PAUSED" => "PAUSED",
        "DISABLED" => "DISABLED",
        _ => "unrecognized",
    };
    let lifecycle = match lifecycle.as_str() {
        "draft" => "draft",
        "paused" => "paused",
        "active" => "active",
        "disabled" => "disabled",
        "invalid" => "invalid",
        _ => "unrecognized",
    };
    Ok((
        passed,
        json!({
            "caseId": case.case_id,
            "route": if case.route == Route::Legacy { "legacy" } else { "v1" },
            "definitionStatus": status, "lifecycleState": lifecycle,
            "commandOutcome": command_outcome,
            "importedCount": report["imported"].as_array().map(Vec::len),
            "skippedCount": report["skipped"].as_array().map(Vec::len),
            "failureCount": report["failures"].as_array().map(Vec::len),
            "sourcePreserved": source_preserved, "preexistingPreserved": preexisting_preserved,
            "dryRunPreserved": dry_run_preserved, "replayPreserved": replay_preserved,
            "restartPreserved": restart_preserved, "workCounts": work_counts,
            "importActionMillis": times.0,
            "passed": passed
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // macOS filesystems reject these names; Linux permits distinct byte paths.
    #[cfg(target_os = "linux")]
    #[test]
    fn source_snapshot_detects_non_utf8_rename() -> anyhow::Result<()> {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let root = tempfile::tempdir()?;
        let source = root.path().join("original");
        fs::create_dir(&source)?;
        let original = source.join(OsString::from_vec(vec![b'f', 0x80]));
        let renamed = source.join(OsString::from_vec(vec![b'f', 0x81]));
        fs::write(&original, "synthetic source")?;
        let before = source_snapshot(root.path())?;

        fs::rename(original, renamed)?;

        assert_ne!(
            before,
            source_snapshot(root.path())?,
            "distinct source paths must not collapse through lossy Unicode conversion"
        );
        Ok(())
    }

    #[test]
    fn source_snapshot_detects_added_empty_directory() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("original");
        fs::create_dir(&source)?;
        fs::write(source.join("automation.toml"), "synthetic source")?;
        let before = source_snapshot(root.path())?;

        fs::create_dir(root.path().join("unexpected-empty"))?;

        assert_ne!(
            before,
            source_snapshot(root.path())?,
            "source inventory changes must include empty directories"
        );
        Ok(())
    }
}
