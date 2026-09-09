use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

pub const AUTOMATION_SCHEDULER_DIAGNOSTICS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_scheduler_last_pass (
        id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
        scheduler_generation INTEGER NOT NULL CHECK (scheduler_generation >= 1),
        trigger TEXT NOT NULL CHECK (trigger IN ('startup', 'deadline', 'wake')),
        scheduled_at TEXT NOT NULL,
        started_at TEXT NOT NULL,
        finished_at TEXT NOT NULL,
        duration_ms INTEGER NOT NULL CHECK (duration_ms >= 0),
        status TEXT NOT NULL CHECK (status IN ('succeeded', 'failed')),
        error_class TEXT,
        planned INTEGER CHECK (planned IS NULL OR planned >= 0),
        recovered INTEGER CHECK (recovered IS NULL OR recovered >= 0),
        claimed INTEGER CHECK (claimed IS NULL OR claimed >= 0),
        dispatched INTEGER CHECK (dispatched IS NULL OR dispatched >= 0),
        failures INTEGER CHECK (failures IS NULL OR failures >= 0),
        CHECK (
            (
                status = 'succeeded'
                AND error_class IS NULL
                AND planned IS NOT NULL
                AND recovered IS NOT NULL
                AND claimed IS NOT NULL
                AND dispatched IS NOT NULL
                AND failures IS NOT NULL
            )
            OR (status = 'failed' AND error_class IS NOT NULL)
        )
    );
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerQueueStatus {
    pub planned: i64,
    pub claimed: i64,
    pub running: i64,
    pub recovery_required: i64,
    pub batch_limit: usize,
    pub oldest_eligible_at: Option<String>,
    pub oldest_eligible_age_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerPassStatus {
    pub generation: i64,
    pub trigger: String,
    pub scheduled_at: String,
    pub started_at: String,
    pub start_lag_ms: i64,
    pub finished_at: String,
    pub duration_ms: i64,
    pub status: String,
    pub error_class: Option<String>,
    pub planned: Option<i64>,
    pub recovered: Option<i64>,
    pub claimed: Option<i64>,
    pub dispatched: Option<i64>,
    pub failures: Option<i64>,
}

pub(crate) struct SchedulerPassRecord<'a> {
    pub trigger: &'a str,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub duration_ms: i64,
    pub status: &'a str,
    pub error_class: Option<&'a str>,
    pub planned: Option<usize>,
    pub recovered: Option<usize>,
    pub claimed: Option<usize>,
    pub dispatched: Option<usize>,
    pub failures: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerStatus {
    pub authority_assigned: bool,
    pub generation: i64,
    pub owner_id: Option<String>,
    pub acquired_at: Option<String>,
    pub last_pass: Option<SchedulerPassStatus>,
    pub queue: SchedulerQueueStatus,
}

pub(crate) fn record_scheduler_pass(
    conn: &Connection,
    fence: &super::leadership::SchedulerFence,
    record: &SchedulerPassRecord<'_>,
) -> Result<()> {
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .context("failed to begin scheduler pass status transaction")?;
    let current: bool = transaction
        .query_row(
            "SELECT COALESCE(owner_id = ?1 AND generation = ?2, 0)
             FROM automation_scheduler_authority
             WHERE id = 1",
            params![fence.owner_id(), fence.generation()],
            |row| row.get(0),
        )
        .context("failed to verify scheduler authority for pass status")?;
    anyhow::ensure!(current, "automations scheduler fence is stale");
    transaction
        .execute(
            "INSERT INTO automation_scheduler_last_pass
                (id, scheduler_generation, trigger, scheduled_at, started_at, finished_at,
                 duration_ms, status, error_class, planned, recovered, claimed, dispatched,
                 failures)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET
                 scheduler_generation = excluded.scheduler_generation,
                 trigger = excluded.trigger,
                 scheduled_at = excluded.scheduled_at,
                 started_at = excluded.started_at,
                 finished_at = excluded.finished_at,
                 duration_ms = excluded.duration_ms,
                 status = excluded.status,
                 error_class = excluded.error_class,
                 planned = excluded.planned,
                 recovered = excluded.recovered,
                 claimed = excluded.claimed,
                 dispatched = excluded.dispatched,
                 failures = excluded.failures",
            params![
                fence.generation(),
                record.trigger,
                iso(record.scheduled_at),
                iso(record.started_at),
                iso(record.finished_at),
                record.duration_ms,
                record.status,
                record.error_class,
                sqlite_count(record.planned, "planned")?,
                sqlite_count(record.recovered, "recovered")?,
                sqlite_count(record.claimed, "claimed")?,
                sqlite_count(record.dispatched, "dispatched")?,
                sqlite_count(record.failures, "failure")?,
            ],
        )
        .context("failed to persist scheduler pass status")?;
    transaction
        .commit()
        .context("failed to commit scheduler pass status")?;
    Ok(())
}

pub fn scheduler_status(conn: &Connection, now: DateTime<Utc>) -> Result<SchedulerStatus> {
    let transaction = conn
        .unchecked_transaction()
        .context("failed to begin automations scheduler status snapshot")?;
    let (owner_id, generation, acquired_at): (Option<String>, i64, Option<String>) = transaction
        .query_row(
            "SELECT owner_id, generation, acquired_at
             FROM automation_scheduler_authority
             WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .context("failed to read automations scheduler authority")?;
    let (planned, claimed, running, recovery_required) = transaction
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM automation_occurrences WHERE state = 'planned'),
                (SELECT COUNT(*) FROM automation_occurrences WHERE state = 'claimed'),
                (SELECT COUNT(*) FROM automation_occurrences WHERE state = 'running'),
                (SELECT COUNT(*) FROM automation_occurrences WHERE state = 'recovery_required')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .context("failed to read automations scheduler queue counts")?;
    let oldest_eligible_at = super::occurrences::eligible_occurrences(&transaction, now, 1)?
        .into_iter()
        .next()
        .map(|occurrence| occurrence.scheduled_for);
    let oldest_eligible_age_ms = oldest_eligible_at
        .as_deref()
        .map(|scheduled_for| -> Result<i64> {
            let scheduled_for = DateTime::parse_from_rfc3339(scheduled_for)
                .context("oldest eligible occurrence has invalid scheduled_for")?
                .with_timezone(&Utc);
            Ok(now
                .signed_duration_since(scheduled_for)
                .num_milliseconds()
                .max(0))
        })
        .transpose()?;
    let queue = SchedulerQueueStatus {
        planned,
        claimed,
        running,
        recovery_required,
        batch_limit: super::occurrences::SCHEDULER_PASS_BATCH_LIMIT,
        oldest_eligible_at,
        oldest_eligible_age_ms,
    };
    let mut last_pass = transaction
        .query_row(
            "SELECT scheduler_generation, trigger, scheduled_at, started_at, finished_at,
                    duration_ms, status, error_class, planned, recovered, claimed, dispatched,
                    failures
             FROM automation_scheduler_last_pass
             WHERE id = 1",
            [],
            |row| {
                Ok(SchedulerPassStatus {
                    generation: row.get(0)?,
                    trigger: row.get(1)?,
                    scheduled_at: row.get(2)?,
                    started_at: row.get(3)?,
                    start_lag_ms: 0,
                    finished_at: row.get(4)?,
                    duration_ms: row.get(5)?,
                    status: row.get(6)?,
                    error_class: row.get(7)?,
                    planned: row.get(8)?,
                    recovered: row.get(9)?,
                    claimed: row.get(10)?,
                    dispatched: row.get(11)?,
                    failures: row.get(12)?,
                })
            },
        )
        .optional()
        .context("failed to read the last automations scheduler pass")?;
    if let Some(pass) = last_pass.as_mut() {
        let scheduled_at = DateTime::parse_from_rfc3339(&pass.scheduled_at)
            .context("scheduler pass has invalid scheduled_at")?;
        let started_at = DateTime::parse_from_rfc3339(&pass.started_at)
            .context("scheduler pass has invalid started_at")?;
        pass.start_lag_ms = started_at
            .signed_duration_since(scheduled_at)
            .num_milliseconds()
            .max(0);
    }
    transaction
        .commit()
        .context("failed to commit automations scheduler status snapshot")?;

    Ok(SchedulerStatus {
        authority_assigned: owner_id.is_some(),
        generation,
        owner_id,
        acquired_at,
        last_pass,
        queue,
    })
}

fn iso(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn sqlite_count(value: Option<usize>, label: &str) -> Result<Option<i64>> {
    value
        .map(|value| {
            i64::try_from(value).with_context(|| format!("{label} count exceeds SQLite range"))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{record_scheduler_pass, scheduler_status, SchedulerPassRecord};

    fn pass<'a>(trigger: &'a str, scheduled_at: chrono::DateTime<Utc>) -> SchedulerPassRecord<'a> {
        SchedulerPassRecord {
            trigger,
            scheduled_at,
            started_at: scheduled_at + chrono::Duration::milliseconds(10),
            finished_at: scheduled_at + chrono::Duration::milliseconds(25),
            duration_ms: 25,
            status: "succeeded",
            error_class: None,
            planned: Some(1),
            recovered: Some(2),
            claimed: Some(3),
            dispatched: Some(4),
            failures: Some(0),
        }
    }

    #[test]
    fn stale_scheduler_cannot_overwrite_a_successor_pass_status() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("coven.sqlite3");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).unwrap();
        let first =
            crate::automations::leadership::SchedulerLeadership::acquire(temp.path(), &conn, now)
                .unwrap();
        let stale_fence = first.fence();
        record_scheduler_pass(&conn, &stale_fence, &pass("startup", now)).unwrap();
        drop(first);

        let second = crate::automations::leadership::SchedulerLeadership::acquire(
            temp.path(),
            &conn,
            now + chrono::Duration::seconds(1),
        )
        .unwrap();
        record_scheduler_pass(
            &conn,
            &second.fence(),
            &pass("wake", now + chrono::Duration::seconds(1)),
        )
        .unwrap();

        let error = record_scheduler_pass(
            &conn,
            &stale_fence,
            &pass("deadline", now + chrono::Duration::seconds(2)),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("scheduler fence is stale"),
            "{error:#}"
        );
        let status = scheduler_status(&conn, now + chrono::Duration::seconds(2)).unwrap();
        let last_pass = status.last_pass.unwrap();
        assert_eq!(last_pass.generation, second.fence().generation());
        assert_eq!(last_pass.trigger, "wake");
        assert_eq!(last_pass.start_lag_ms, 10);
    }
}
