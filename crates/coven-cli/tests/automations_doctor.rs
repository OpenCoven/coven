#![cfg(any(unix, windows))]

use std::process::{Command, Stdio};

#[test]
fn doctor_missing_daemon_never_creates_home() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = root.path().join("must-not-be-created");
    let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
    #[cfg(unix)]
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default());
    let output = command
        .args(["automations", "doctor", "--json"])
        .env("COVEN_HOME", &home)
        .env("HOME", root.path())
        .env_remove("COVEN_MAINTENANCE_PARTICIPANT")
        .stdin(Stdio::null())
        .output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("owner-local Coven daemon unavailable"),
        "doctor did not report unavailable owner-local daemon"
    );
    assert!(!home.exists(), "doctor created state or started a daemon");
    Ok(())
}

#[test]
fn doctor_reports_every_category_through_owner_local_daemon() -> anyhow::Result<()> {
    doctor_report_fixture(false)
}

#[test]
fn doctor_verifies_base_receipt_through_owner_local_action_and_cli() -> anyhow::Result<()> {
    doctor_report_fixture(true)
}

fn doctor_report_fixture(genuine_receipt: bool) -> anyhow::Result<()> {
    use coven_client::{DaemonClient, DaemonEndpoint};
    use serde_json::{json, Value};
    use std::time::{Duration, Instant};

    struct Daemon(std::process::Child);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    #[cfg(unix)]
    let root = tempfile::tempdir_in("/tmp")?;
    #[cfg(windows)]
    let root = tempfile::tempdir()?;
    let home = root.path().join("home");
    let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
    #[cfg(unix)]
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default());
    let mut daemon = Daemon(
        command
            .args(["daemon", "serve"])
            .env("COVEN_HOME", &home)
            .env("HOME", root.path())
            .env_remove("COVEN_MAINTENANCE_PARTICIPANT")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let started = Instant::now();
    let mut client = loop {
        if let Ok(endpoint) = DaemonEndpoint::discover(&home) {
            let mut client = DaemonClient::new(endpoint);
            if client.health().is_ok() {
                break client;
            }
        }
        // Hang guard only; authenticated health establishes readiness.
        anyhow::ensure!(
            daemon.0.try_wait()?.is_none() && started.elapsed() < Duration::from_secs(120),
            "doctor fixture daemon failed readiness after {:?}",
            started.elapsed()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let conn = rusqlite::Connection::open(home.join("coven.sqlite3"))?;
    conn.busy_timeout(Duration::from_secs(30))?;
    // Terminal ledger fixture only: no runnable definition, session or authority
    // grant. Large private bodies must never be projected into doctor output.
    let private_body = "doctor-private-body".repeat(65_536);
    let receipt_body = serde_json::to_string(&json!({"private": private_body}))?;
    conn.execute_batch("BEGIN IMMEDIATE;
        INSERT INTO automation_occurrences(id,automation_id,state,scheduled_for,created_at,updated_at,failure_reason)
        VALUES('doctor-private-occurrence','doctor-private-definition','failed','2026-10-18T00:00:00Z','2026-10-18T00:00:00Z','2026-10-18T00:00:01Z','doctor-private-reason');
        INSERT INTO automation_runs(id,automation_id,occurrence_id,status,started_at,finished_at,delivery_state,delivery_reason)
        VALUES('doctor-private-run','doctor-private-definition','doctor-private-occurrence','failed','2026-10-18T00:00:00Z','2026-10-18T00:00:01Z','failed','doctor-private-delivery');
        INSERT INTO automation_attempts(id,run_id,occurrence_id,attempt_number,adoption_key,occurrence_fence_generation,state,failure_class,retry_classification,not_before,opened_at,settled_at,state_reason)
        VALUES('doctor-private-attempt','doctor-private-run','doctor-private-occurrence',1,'doctor-private-adoption',1,'failed','launch_refused','initial','2026-10-18T00:00:00Z','2026-10-18T00:00:00Z','2026-10-18T00:00:01Z','doctor-private-attempt-reason');")?;
    conn.execute(
        "UPDATE automation_runs SET log_json=?1 WHERE id='doctor-private-run'",
        [&receipt_body],
    )?;
    if genuine_receipt {
        insert_committed_base_receipt(&conn)?;
    } else {
        // Deliberately oversized evidence must produce a bounded-read gap.
        conn.execute("INSERT INTO automation_receipts(id,run_id,attempt_id,automation_id,automation_revision,occurrence_id,occurrence_fence_generation,attempt_number,outcome,receipt_digest,receipt_json,event_id,produced_at)
        VALUES('doctor-private-receipt','doctor-private-run','doctor-private-attempt','doctor-private-definition',1,'doctor-private-occurrence',1,1,'failed',?1,?2,'doctor-private-event','2026-10-18T00:00:01Z')",
        rusqlite::params!["a".repeat(64), receipt_body])?;
    }
    conn.execute_batch("UPDATE automation_runs SET receipt_id='doctor-private-receipt' WHERE id='doctor-private-run'; COMMIT;")?;
    // Reserve the single writer so background scheduler/maintenance observations
    // cannot race the comparison. The WAL reader must still serve doctor;
    // an accidental write in that request cannot acquire this reservation.
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let snapshot = || -> anyhow::Result<Vec<(String, Vec<Vec<rusqlite::types::Value>>)>> {
        let names = conn.prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut result = Vec::new();
        for name in names {
            let quoted = name.replace('"', "\"\"");
            let mut statement = conn.prepare(&format!("SELECT * FROM \"{quoted}\""))?;
            let columns = statement.column_count();
            let rows = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| row.get(column))
                        .collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            result.push((name, rows));
        }
        Ok(result)
    };
    let before = snapshot()?;
    let response = client.request_json(
        "POST",
        "/api/v1/actions",
        Some(&json!({"action": "coven.automations.doctor.v1"})),
    )?;
    assert_eq!(
        response.status, 200,
        "doctor action must be available locally"
    );
    assert!(
        response.body.len() <= 32_768,
        "doctor action exceeded its output bound"
    );
    assert!(!String::from_utf8_lossy(&response.body).contains("doctor-private"));
    for as_json in [true, false] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
        #[cfg(unix)]
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default());
        command.args(["automations", "doctor"]);
        if as_json {
            command.arg("--json");
        }
        let output = command
            .env("COVEN_HOME", &home)
            .env("HOME", root.path())
            .env_remove("COVEN_MAINTENANCE_PARTICIPANT")
            .stdin(Stdio::null())
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "doctor CLI failed while writer was reserved: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stdout.len() <= 32_768,
            "doctor CLI exceeded its output bound"
        );
        assert!(output.stderr.is_empty());
        let text = std::str::from_utf8(&output.stdout)?;
        assert!(!text.contains("doctor-private"));
        assert!(!text.contains(&home.to_string_lossy().to_string()));
        let payload = if as_json {
            text
        } else {
            text.strip_prefix(
                "Automations doctor (persisted observations; explicit evidence gaps)\n",
            )
            .expect("human output must preserve evidence-gap heading")
        };
        let report: Value = serde_json::from_str(payload)?;
        assert_eq!(report["schemaVersion"], "coven.automations.doctor.v1");
        assert_eq!(report["observations"].as_array().unwrap().len(), 11);
        assert!(report.get("healthy").is_none());
        let facts = |category: &str| {
            report["observations"]
                .as_array()
                .unwrap()
                .iter()
                .find(|observation| observation["category"] == category)
                .unwrap()["facts"]
                .clone()
        };
        assert_eq!(facts("repeated_failures")["failed_attempts"], 1);
        assert_eq!(facts("authority_refusals")["launch_refused_attempts"], 1);
        assert_eq!(facts("delivery_receipts")["failed_deliveries"], 1);
        assert_eq!(facts("delivery_receipts")["stored_receipts"], 1);
        let delivery = report["observations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|observation| observation["category"] == "delivery_receipts")
            .unwrap();
        if genuine_receipt {
            assert_eq!(delivery["facts"]["receipt_commitments_checked"], 1);
            assert_eq!(delivery["facts"]["invalid_receipt_commitments"], 0);
            assert_eq!(delivery["gaps"], json!(["receipt_authority_not_verified"]));
        } else {
            assert_eq!(
                delivery["gaps"],
                json!([
                    "receipt_commitments_read_budget_exceeded",
                    "receipt_authority_not_verified"
                ])
            );
        }
        assert!(
            snapshot()? == before,
            "doctor CLI changed application tables"
        );
    }
    let after = snapshot()?;
    assert!(before == after, "doctor changed application tables");
    conn.execute_batch("ROLLBACK")?;
    let body: Value = serde_json::from_slice(&response.body)?;
    assert_eq!(body["ok"], true);
    assert_eq!(body["accepted"], true);
    assert_eq!(body["status"], "completed");
    assert!(body["event"].is_null(), "doctor must not publish an event");
    assert_eq!(
        body["result"]["schemaVersion"],
        "coven.automations.doctor.v1"
    );
    let observations = body["result"]["observations"]
        .as_array()
        .expect("doctor observations must be an array");
    let delivery = observations
        .iter()
        .find(|o| o["category"] == "delivery_receipts")
        .unwrap();
    if genuine_receipt {
        assert_eq!(delivery["facts"]["receipt_commitments_checked"], 1);
        assert_eq!(delivery["facts"]["invalid_receipt_commitments"], 0);
        assert_eq!(delivery["gaps"], json!(["receipt_authority_not_verified"]));
    } else {
        assert_eq!(
            delivery["gaps"],
            json!([
                "receipt_commitments_read_budget_exceeded",
                "receipt_authority_not_verified"
            ])
        );
    }
    let mut categories = observations
        .iter()
        .map(|value| value["category"].as_str().expect("closed category code"))
        .collect::<Vec<_>>();
    categories.sort_unstable();
    assert_eq!(
        categories,
        [
            "authority_refusals",
            "delivery_receipts",
            "disk_retention",
            "event_changefeed",
            "last_successful_pass",
            "leadership",
            "leases_fences",
            "planning_start_lag",
            "queue",
            "repeated_failures",
            "store",
        ]
    );
    assert!(
        body["result"].get("healthy").is_none(),
        "a generic health bit must not hide unobserved categories"
    );
    Ok(())
}

// Seed independently canonicalized terminal evidence. The production reader,
// reached through both public owner-local surfaces, must validate every binding.
// This is unsigned base evidence and deliberately carries no authority claim.
fn insert_committed_base_receipt(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    fn seal(value: &mut Value, receipt: bool) -> anyhow::Result<()> {
        let bytes = Sha256::digest(serde_jcs::to_vec(&*value)?);
        let mut digest = String::with_capacity(64);
        for byte in bytes {
            use std::fmt::Write;
            write!(&mut digest, "{byte:02x}")?;
        }
        value["integrity"] = json!({
            "algorithm":"sha256", "canonicalization":"jcs-rfc8785", "value":digest
        });
        if receipt {
            value["integrity"]["authentication"] = json!("none");
        }
        Ok(())
    }
    conn.execute_batch("UPDATE automation_runs SET automation_revision=1,familiar_id='charm',runtime='coven-code' WHERE id='doctor-private-run';")?;
    let sequence: i64 = conn.query_row("SELECT next_sequence FROM automation_event_stream_heads WHERE stream_kind='run' AND stream_id='doctor-private-run'", [], |row| row.get(0))?;
    let time = "2026-10-18T00:00:01Z";
    let producer = json!({"component":"coven-daemon", "instanceId":"local-doctor-fixture", "implementationVersion":env!("CARGO_PKG_VERSION")});
    let privacy =
        json!({"classification":"operational", "retention":{"classification":"standard"}});
    let mut receipt = json!({
        "schemaVersion":"coven.automations.v1", "receiptId":"doctor-private-receipt",
        "automationId":"doctor-private-definition", "automationRevision":1,
        "occurrenceId":"doctor-private-occurrence", "occurrenceFenceGeneration":1,
        "runId":"doctor-private-run", "attemptId":"doctor-private-attempt", "attemptNumber":1,
        "identity":{"familiarId":"charm"}, "sideEffectClass":"none",
        "outcome":{"disposition":"failed", "failureClass":"launch_refused", "recoveryDisposition":"not_required"},
        "producedAt":time, "producer":producer, "privacy":privacy
    });
    seal(&mut receipt, true)?;
    let event_id = "evt00000000000000000000000000042";
    let mut event = json!({
        "schemaVersion":"coven.automations.v1", "eventId":event_id,
        "stream":{"kind":"run", "id":"doctor-private-run"}, "sequence":sequence,
        "recordedAt":time, "observedAt":time, "producer":producer,
        "automationId":"doctor-private-definition", "occurrenceId":"doctor-private-occurrence",
        "runId":"doctor-private-run", "attemptId":"doctor-private-attempt",
        "kind":"receipt.recorded", "summary":"automation run receipt recorded",
        "payload":{"receiptRef":"doctor-private-receipt", "outcome":"failed", "sideEffectClass":"none"},
        "privacy":privacy
    });
    seal(&mut event, false)?;
    conn.execute("UPDATE automation_event_stream_heads SET next_sequence=?1,updated_at=?2 WHERE stream_kind='run' AND stream_id='doctor-private-run'", rusqlite::params![sequence+1,time])?;
    conn.execute("INSERT INTO automation_events(event_id,stream_kind,stream_id,sequence,recorded_at,recorded_at_millis,observed_at,event_json)
        VALUES(?1,'run','doctor-private-run',?5,?2,?3,?2,?4)",
        rusqlite::params![event_id,time,chrono::DateTime::parse_from_rfc3339(time)?.timestamp_millis(),serde_json::to_string(&event)?,sequence])?;
    conn.execute("INSERT INTO automation_receipts(id,run_id,attempt_id,automation_id,automation_revision,occurrence_id,occurrence_fence_generation,attempt_number,outcome,receipt_digest,receipt_json,event_id,produced_at)
        VALUES('doctor-private-receipt','doctor-private-run','doctor-private-attempt','doctor-private-definition',1,'doctor-private-occurrence',1,1,'failed',?1,?2,?3,?4)",
        rusqlite::params![receipt["integrity"]["value"].as_str().unwrap(),serde_json::to_string(&receipt)?,event_id,time])?;
    Ok(())
}

#[test]
fn doctor_stale_discovery_does_not_echo_metadata_or_initialize_store() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = root.path().join("home");
    std::fs::create_dir(&home)?;
    let metadata = b"{\"private\":\"doctor-private-discovery-sentinel\"}";
    std::fs::write(home.join("daemon.json"), metadata)?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
    #[cfg(unix)]
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default());
    let output = command
        .args(["automations", "doctor", "--json"])
        .env("COVEN_HOME", &home)
        .env("HOME", root.path())
        .env_remove("COVEN_MAINTENANCE_PARTICIPANT")
        .stdin(Stdio::null())
        .output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("owner-local Coven daemon unavailable"));
    assert!(!error.contains("doctor-private-discovery-sentinel"));
    assert_eq!(std::fs::read(home.join("daemon.json"))?, metadata);
    assert_eq!(std::fs::read_dir(&home)?.count(), 1);
    Ok(())
}
