use std::process::Command;

#[cfg(any(unix, windows))]
#[test]
fn inspection_reads_real_daemon_records_without_mutating_them() -> anyhow::Result<()> {
    use coven_client::{DaemonClient, DaemonEndpoint};
    use serde_json::{json, Value};
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    struct Daemon(Child);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    // Short socket path on macOS; no real home, provider, or harness is used.
    #[cfg(unix)]
    let temp = tempfile::tempdir_in("/tmp")?;
    #[cfg(windows)]
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let log = temp.path().join("daemon.log");
    let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
    #[cfg(unix)]
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default());
    command
        .args(["daemon", "serve"])
        .env("COVEN_HOME", &home)
        .env("HOME", temp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log)?);
    let mut daemon = Daemon(command.spawn()?);
    let started = Instant::now();
    let mut client = loop {
        if let Ok(endpoint) = DaemonEndpoint::discover(&home) {
            let mut client = DaemonClient::new(endpoint);
            if client.health().is_ok() {
                break client;
            }
        }
        // Hang guard only: readiness is proven by the authenticated health response.
        anyhow::ensure!(
            daemon.0.try_wait()?.is_none() && started.elapsed() < Duration::from_secs(120),
            "fixture daemon failed readiness after {:?}: {}",
            started.elapsed(),
            std::fs::read_to_string(&log)?
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let created = client.request_json(
        "POST",
        "/api/v1/actions",
        Some(&json!({
            "action": "coven.automations.definition.create.v1", "adoptionKey": "inspection-fixture",
            "definition": {"schemaVersion": 1, "id": "inspection", "name": "Inspection fixture",
                "status": "PAUSED", "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc",
                "misfire": "latest", "overlap": "forbid", "timeoutMinutes": 30,
                "runtime": "coven-code", "prompt": "private diagnostic sentinel"}
        })),
    )?;
    assert_eq!(created.status, 200);
    assert_eq!(
        serde_json::from_slice::<Value>(&created.body)?["result"]["outcome"],
        "committed"
    );
    // Seed terminal history only in this disposable store. The paused definition
    // and terminal occurrence cannot schedule or launch any runtime.
    let conn = rusqlite::Connection::open(home.join("coven.sqlite3"))?;
    conn.busy_timeout(Duration::from_secs(30))?;
    conn.execute_batch("INSERT INTO automation_occurrences
        (id, automation_id, automation_revision, scheduled_for, kind, state, attempt, created_at, updated_at)
        SELECT 'occurrence-1', id, revision, '2026-10-18T00:00:00Z', 'manual', 'failed', 1,
            '2026-10-18T00:00:00Z', '2026-10-18T00:00:00Z' FROM automation_definitions WHERE id = 'inspection';
        INSERT INTO automation_runs
        (id, automation_id, automation_revision, occurrence_id, status, started_at, finished_at, log_json)
        SELECT 'run-1', automation_id, automation_revision, id, 'failed', '2026-10-18T00:00:00Z',
            '2026-10-18T00:00:01Z', 'private diagnostic sentinel' FROM automation_occurrences WHERE id = 'occurrence-1';
        INSERT INTO automation_attempts
        (id, run_id, occurrence_id, attempt_number, adoption_key, occurrence_fence_generation,
            dispatch_generation, state, failure_class, retry_classification, not_before, opened_at, settled_at, state_reason)
        VALUES ('attempt-1', 'run-1', 'occurrence-1', 1, 'private diagnostic sentinel', 1, 1,
            'failed', 'runtime_error', 'initial', '2026-10-18T00:00:00Z', '2026-10-18T00:00:00Z',
            '2026-10-18T00:00:01Z', 'private diagnostic sentinel');")?;
    let snapshot = || -> anyhow::Result<Vec<String>> {
        let mut snapshot = Vec::new();
        for table in [
            "automation_definitions",
            "automation_runs",
            "automation_attempts",
            "automation_occurrences",
            "automation_command_adoptions",
            "automation_events",
        ] {
            let mut statement = conn.prepare(&format!("SELECT * FROM {table}"))?;
            let columns = statement.column_count();
            let rows = statement.query_map([], |row| {
                (0..columns)
                    .map(|column| row.get::<_, rusqlite::types::Value>(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })?;
            for row in rows {
                snapshot.push(format!("{table}: {:?}", row?));
            }
        }
        snapshot.sort();
        Ok(snapshot)
    };
    let before = snapshot()?;
    for (args, key) in [
        (vec!["status", "--json"], "scheduler"),
        (vec!["occurrence", "occurrence-1", "--json"], "occurrence"),
        (vec!["run", "run-1", "--json"], "run"),
        (vec!["attempts", "run-1", "--json"], "attempts"),
        (vec!["explain", "inspection", "--json"], "explanation"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_coven"))
            .arg("automations")
            .args(&args)
            .env("COVEN_HOME", &home)
            .output()?;
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let body = String::from_utf8(output.stdout)?;
        assert!(!body.contains("private diagnostic sentinel"));
        let body: Value = serde_json::from_str(&body)?;
        assert_eq!(body["schemaVersion"], "coven.automations.inspection.v1");
        assert!(body.get(key).is_some(), "{body}");
        if key == "run" {
            assert_eq!(body[key]["status"], "failed");
        }
        if key == "attempts" {
            assert_eq!(body[key][0]["state"], "failed");
        }
        if key == "explanation" {
            // Paused: no next slot is promised.
            assert_eq!(
                body[key]["conditions"],
                json!([{"code": "not_active", "nextStep": "activate_if_intended"}])
            );
        }
    }
    for (args, error) in [
        (["run", "absent"], "automation run was not found"),
        (["explain", "absent"], "automation routine was not found"),
    ] {
        let missing = Command::new(env!("CARGO_BIN_EXE_coven"))
            .arg("automations")
            .args(args)
            .arg("--json")
            .env("COVEN_HOME", &home)
            .output()?;
        assert!(!missing.status.success());
        assert!(missing.stdout.is_empty());
        assert_eq!(
            String::from_utf8(missing.stderr)?,
            format!("Error: {error}\n")
        );
    }
    assert_eq!(snapshot()?, before);
    let raw: String = conn.query_row(
        "SELECT log_json FROM automation_runs WHERE id = 'run-1'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(raw, "private diagnostic sentinel");
    Ok(())
}

#[test]
fn inspection_refuses_unavailable_daemon_without_creating_home() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("missing-home");
    for args in [
        vec!["status", "--json"],
        vec!["occurrence", "occurrence-1", "--json"],
        vec!["run", "run-1", "--json"],
        vec!["attempts", "run-1", "--json"],
        vec!["explain", "routine-1", "--json"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_coven"))
            .arg("automations")
            .args(&args)
            .env("COVEN_HOME", &home)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("owner-local"), "{args:?}: {error}");
        assert!(!error.contains(home.to_str().unwrap()));
        assert!(!home.exists(), "inspection created home: {args:?}");
    }
}

#[test]
fn inspection_help_exposes_reads_without_initializing_state() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("missing-home");
    let output = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args(["automations", "--help"])
        .env("COVEN_HOME", &home)
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for command in [
        "status",
        "occurrence",
        "run",
        "attempts",
        "explain",
        "conformance",
    ] {
        assert!(help.contains(command), "missing {command}");
    }
    assert!(!home.exists());
}
