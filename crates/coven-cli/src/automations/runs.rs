//! The run ledger for routine executions (coven#816).
//!
//! Every claimed occurrence that reaches a runtime records exactly one
//! automation_runs row: session id, familiar, runtime, bounded log, exit
//! code, terminal status, and output-commit state. The ledger is the single
//! observability surface Cave's UI reads — never a per-harness history file.

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};

pub const AUTOMATION_RUNS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_runs (
        id TEXT PRIMARY KEY NOT NULL,
        automation_id TEXT NOT NULL,
        automation_revision INTEGER NOT NULL DEFAULT 1 CHECK (automation_revision >= 1),
        definition_digest TEXT,
        occurrence_id TEXT,
        receipt_id TEXT,
        session_id TEXT,
        familiar_id TEXT,
        runtime TEXT,
        status TEXT NOT NULL,
        exit_code INTEGER,
        log_json TEXT,
        output_commit TEXT,
        started_at TEXT NOT NULL,
        timeout_at TEXT,
        finished_at TEXT,
        FOREIGN KEY (occurrence_id) REFERENCES automation_occurrences(id) ON DELETE SET NULL
    );

    CREATE INDEX IF NOT EXISTS idx_automation_runs_automation_started
        ON automation_runs(automation_id, started_at DESC);
";

#[allow(dead_code)]
const LOG_ENTRY_MAX_CHARS: usize = 64 * 1024;

#[allow(dead_code)]
fn iso(instant: DateTime<Utc>) -> String {
    instant.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
    pub id: String,
    pub automation_id: String,
    pub automation_revision: u64,
    pub definition_digest: Option<String>,
    pub occurrence_id: Option<String>,
    pub receipt_id: Option<String>,
    pub session_id: Option<String>,
    pub familiar_id: Option<String>,
    pub runtime: String,
    pub status: String,
    pub exit_code: Option<i64>,
    pub log_json: Option<String>,
    pub output_commit: Option<String>,
    pub started_at: String,
    pub timeout_at: Option<String>,
    pub finished_at: Option<String>,
}

#[allow(dead_code)] // consumed by the part-4 dispatch path; tests cover it today
pub struct RunStart<'a> {
    pub automation_id: &'a str,
    pub occurrence_id: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub familiar_id: Option<&'a str>,
    pub runtime: &'a str,
    pub timeout_at: DateTime<Utc>,
}

pub fn record_run_start(
    conn: &Connection,
    run_id: &str,
    start: RunStart<'_>,
    now: DateTime<Utc>,
) -> Result<()> {
    let (automation_revision, definition_digest) =
        definition_pin(conn, start.automation_id, start.occurrence_id)?;
    conn.execute(
        "INSERT INTO automation_runs
            (id, automation_id, automation_revision, definition_digest, occurrence_id,
             session_id, familiar_id, runtime, status, started_at, timeout_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'running', ?9, ?10)",
        params![
            run_id,
            start.automation_id,
            i64::try_from(automation_revision)
                .context("automation revision exceeds SQLite range")?,
            definition_digest,
            start.occurrence_id,
            start.session_id,
            start.familiar_id,
            start.runtime,
            iso(now),
            iso(start.timeout_at)
        ],
    )
    .context("failed to record run start")?;
    Ok(())
}

fn definition_pin(
    conn: &Connection,
    automation_id: &str,
    occurrence_id: Option<&str>,
) -> Result<(u64, Option<String>)> {
    if let Some(occurrence_id) = occurrence_id {
        let occurrence = conn
            .query_row(
                "SELECT automation_id, automation_revision, definition_digest
                 FROM automation_occurrences
                 WHERE id = ?1",
                [occurrence_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .context("failed to read occurrence definition pin")?;
        let Some((occurrence_automation_id, revision, digest)) = occurrence else {
            anyhow::bail!("automation occurrence `{occurrence_id}` does not exist");
        };
        if occurrence_automation_id != automation_id {
            anyhow::bail!(
                "automation occurrence `{occurrence_id}` does not belong to automation `{automation_id}`"
            );
        }
        return Ok((
            u64::try_from(revision).context("occurrence revision is negative")?,
            digest,
        ));
    }

    let current = conn
        .query_row(
            "SELECT revision, definition_digest
             FROM automation_definitions
             WHERE id = ?1 AND tombstoned_at IS NULL",
            [automation_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .context("failed to read automation definition pin")?;
    match current {
        Some((revision, digest)) => Ok((
            u64::try_from(revision).context("automation definition revision is negative")?,
            digest,
        )),
        None => Ok((1, None)),
    }
}

#[allow(dead_code)] // consumed by the part-4 dispatch path; tests cover it today
pub struct RunFinish {
    pub status: &'static str,
    pub exit_code: Option<i64>,
    pub session_id: Option<String>,
    pub log_json: Option<String>,
    pub output_commit: Option<String>,
}

#[allow(dead_code)] // consumed by the part-4 dispatch path; tests cover it today
pub fn record_run_finish(
    conn: &Connection,
    run_id: &str,
    finish: RunFinish,
    now: DateTime<Utc>,
) -> Result<bool> {
    let RunFinish {
        status,
        exit_code,
        session_id,
        log_json,
        output_commit,
    } = finish;
    if status != "succeeded" && status != "failed" && status != "cancelled" {
        return Err(anyhow::anyhow!(
            "run status must be succeeded, failed, or cancelled"
        ));
    }
    let bounded_log = log_json
        .as_deref()
        .map(|log| {
            if log.chars().count() > LOG_ENTRY_MAX_CHARS {
                let mut truncated: String = log.chars().take(LOG_ENTRY_MAX_CHARS).collect();
                truncated.push_str("…(truncated)");
                truncated
            } else {
                log.to_string()
            }
        })
        .filter(|log| !log.is_empty());
    let changed = conn
        .execute(
            "UPDATE automation_runs
             SET status = ?2,
                 exit_code = ?3,
                 session_id = ?4,
                 log_json = ?5,
                 output_commit = ?6,
                 finished_at = ?7
             WHERE id = ?1 AND status = 'running'",
            params![
                run_id,
                status,
                exit_code,
                session_id,
                bounded_log,
                output_commit,
                iso(now)
            ],
        )
        .context("failed to record run finish")?;
    Ok(changed > 0)
}

pub fn list_runs(conn: &Connection, automation_id: &str, limit: i64) -> Result<Vec<RunRecord>> {
    let bounded = limit.clamp(1, 100);
    let mut statement = conn
        .prepare(
            "SELECT id, automation_id, automation_revision, definition_digest, occurrence_id,
                    receipt_id, session_id, familiar_id, runtime, status, exit_code, log_json,
                    output_commit, started_at, timeout_at, finished_at
             FROM automation_runs
             WHERE automation_id = ?1
             ORDER BY started_at DESC
             LIMIT ?2",
        )
        .context("failed to prepare run list query")?;
    let rows = statement
        .query_map(params![automation_id, bounded], |row| {
            Ok(RunRecord {
                id: row.get(0)?,
                automation_id: row.get(1)?,
                automation_revision: u64::try_from(row.get::<_, i64>(2)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?,
                definition_digest: row.get(3)?,
                occurrence_id: row.get(4)?,
                receipt_id: row.get(5)?,
                session_id: row.get(6)?,
                familiar_id: row.get(7)?,
                runtime: row.get(8)?,
                status: row.get(9)?,
                exit_code: row.get(10)?,
                log_json: row.get(11)?,
                output_commit: row.get(12)?,
                started_at: row.get(13)?,
                timeout_at: row.get(14)?,
                finished_at: row.get(15)?,
            })
        })
        .context("failed to list runs")?;

    let mut records = Vec::new();
    for row in rows {
        records.push(row.context("failed to read run row")?);
    }

    Ok(records)
}

pub fn ensure_timeout_column(conn: &Connection) -> Result<()> {
    let present = {
        let mut statement = conn
            .prepare("PRAGMA table_info(automation_runs)")
            .context("failed to inspect automation_runs columns")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .context("failed to query automation_runs columns")?;
        let mut present = false;
        for column in columns {
            if column.context("failed to read automation_runs column")? == "timeout_at" {
                present = true;
                break;
            }
        }
        present
    };
    if !present {
        conn.execute_batch("ALTER TABLE automation_runs ADD COLUMN timeout_at TEXT")
            .context("failed to add automation_runs.timeout_at")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::initialize_store;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn temp_store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        (temp, conn)
    }

    #[test]
    fn run_lifecycle_round_trip() {
        let (_temp, conn) = temp_store();
        let start = utc(2026, 8, 28, 9, 0);
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES ('occ-1', 'daily', '2026-08-28T09:00:00.000Z', 'claimed', 1,
                     '2026-08-28T09:00:00.000Z', '2026-08-28T09:00:00.000Z')",
            [],
        )
        .unwrap();
        record_run_start(
            &conn,
            "run-1",
            RunStart {
                automation_id: "daily",
                occurrence_id: Some("occ-1"),
                session_id: None,
                familiar_id: Some("charm"),
                runtime: "coven-code",
                timeout_at: start + chrono::Duration::minutes(30),
            },
            start,
        )
        .unwrap();

        let finished = record_run_finish(
            &conn,
            "run-1",
            RunFinish {
                status: "succeeded",
                exit_code: Some(0),
                session_id: Some("session-1".to_string()),
                log_json: Some("log line".to_string()),
                output_commit: Some("committed".to_string()),
            },
            start + chrono::Duration::minutes(5),
        )
        .unwrap();
        assert!(finished);

        let runs = list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "succeeded");
        assert_eq!(runs[0].session_id.as_deref(), Some("session-1"));
        assert_eq!(runs[0].familiar_id.as_deref(), Some("charm"));
        assert_eq!(
            runs[0].timeout_at.as_deref(),
            Some("2026-08-28T09:30:00.000Z")
        );
    }

    #[test]
    fn run_start_pins_the_occurrence_definition_metadata() {
        let (_temp, conn) = temp_store();
        let start = utc(2026, 8, 28, 9, 0);
        conn.execute(
            "INSERT INTO automation_occurrences (
                id, automation_id, automation_revision, definition_digest, scheduled_for,
                state, attempt, created_at, updated_at
             ) VALUES (
                'occ-pinned', 'daily', 7, 'definition-seven',
                '2026-08-28T09:00:00.000Z', 'claimed', 1,
                '2026-08-28T09:00:00.000Z', '2026-08-28T09:00:00.000Z'
             )",
            [],
        )
        .unwrap();

        record_run_start(
            &conn,
            "run-pinned",
            RunStart {
                automation_id: "daily",
                occurrence_id: Some("occ-pinned"),
                session_id: None,
                familiar_id: Some("cody"),
                runtime: "coven-code",
                timeout_at: start + chrono::Duration::minutes(30),
            },
            start,
        )
        .unwrap();

        let pin: (i64, String, Option<String>) = conn
            .query_row(
                "SELECT automation_revision, definition_digest, receipt_id
                 FROM automation_runs
                 WHERE id = 'run-pinned'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(pin, (7, "definition-seven".to_string(), None));

        let error = record_run_start(
            &conn,
            "run-mismatched",
            RunStart {
                automation_id: "other",
                occurrence_id: Some("occ-pinned"),
                session_id: None,
                familiar_id: Some("cody"),
                runtime: "coven-code",
                timeout_at: start + chrono::Duration::minutes(30),
            },
            start,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("does not belong to automation `other`"));
    }

    #[test]
    fn finishing_a_non_running_run_is_a_no_op() {
        let (_temp, conn) = temp_store();
        let start = utc(2026, 8, 28, 9, 0);
        record_run_start(
            &conn,
            "run-2",
            RunStart {
                automation_id: "daily",
                occurrence_id: None,
                session_id: None,
                familiar_id: None,
                runtime: "coven-code",
                timeout_at: start + chrono::Duration::minutes(30),
            },
            start,
        )
        .unwrap();
        record_run_finish(
            &conn,
            "run-2",
            RunFinish {
                status: "succeeded",
                exit_code: None,
                session_id: None,
                log_json: None,
                output_commit: None,
            },
            start,
        )
        .unwrap();

        let again = record_run_finish(
            &conn,
            "run-2",
            RunFinish {
                status: "failed",
                exit_code: Some(1),
                session_id: None,
                log_json: None,
                output_commit: None,
            },
            start + chrono::Duration::minutes(1),
        )
        .unwrap();
        assert!(!again);

        let runs = list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs[0].status, "succeeded");
    }

    #[test]
    fn rejects_unknown_terminal_status() {
        let (_temp, conn) = temp_store();
        let start = utc(2026, 8, 28, 9, 0);
        record_run_start(
            &conn,
            "run-3",
            RunStart {
                automation_id: "daily",
                occurrence_id: None,
                session_id: None,
                familiar_id: None,
                runtime: "coven-code",
                timeout_at: start + chrono::Duration::minutes(30),
            },
            start,
        )
        .unwrap();
        let error = record_run_finish(
            &conn,
            "run-3",
            RunFinish {
                status: "meh",
                exit_code: None,
                session_id: None,
                log_json: None,
                output_commit: None,
            },
            start,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("succeeded, failed, or cancelled"));
    }
}
