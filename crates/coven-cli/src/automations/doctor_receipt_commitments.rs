//! Bounded base receipt commitments; authenticated authority evidence is separate.
use super::super::contract::types::{AttemptId, EventId, ReceiptId, RunId};
use anyhow::Result;
use rusqlite::{limits::Limit, Connection, ErrorCode, Row};
use std::collections::BTreeSet;

pub(super) const PREREQUISITES: &[(&str, &[&str])] = &[
    (
        "automation_receipts",
        &[
            "id",
            "run_id",
            "attempt_id",
            "automation_id",
            "automation_revision",
            "definition_digest",
            "occurrence_id",
            "occurrence_fence_generation",
            "attempt_number",
            "outcome",
            "receipt_digest",
            "receipt_json",
            "event_id",
            "produced_at",
        ],
    ),
    (
        "automation_events",
        &[
            "event_id",
            "stream_kind",
            "stream_id",
            "sequence",
            "recorded_at",
            "recorded_at_millis",
            "observed_at",
            "event_json",
        ],
    ),
    (
        "automation_runs",
        &[
            "id",
            "automation_id",
            "automation_revision",
            "definition_digest",
            "occurrence_id",
            "familiar_id",
            "runtime",
            "authority_profile",
            "status",
            "receipt_id",
            "finished_at",
        ],
    ),
    (
        "automation_attempts",
        &[
            "id",
            "run_id",
            "occurrence_id",
            "attempt_number",
            "adoption_key",
            "occurrence_fence_generation",
            "authority_extension_json",
            "state",
            "failure_class",
            "state_reason",
            "settled_at",
        ],
    ),
    (
        "automation_rich_definition_revisions",
        &["automation_id", "revision", "integrity"],
    ),
];
pub(super) const MAX_ROWS: usize = 256;
const MAX_BODY_BYTES: u64 = 128 * 1024;
const MAX_COHORT_BYTES: u64 = 1024 * 1024;
const MAX_ROW_BYTES: i32 = 256 * 1024;
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Observation {
    Verified(u64),
    Inconsistent(u64),
    InvalidMetadata,
    BudgetExceeded,
    Unavailable,
}

struct LengthLimit<'a> {
    conn: &'a Connection,
    prior: i32,
    restored: bool,
}
impl<'a> LengthLimit<'a> {
    fn install(conn: &'a Connection) -> Result<Self> {
        let prior = conn.limit(Limit::SQLITE_LIMIT_LENGTH)?;
        conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, prior.min(MAX_ROW_BYTES))?;
        Ok(Self {
            conn,
            prior,
            restored: false,
        })
    }
    fn restore(mut self) -> Result<()> {
        self.conn
            .set_limit(Limit::SQLITE_LIMIT_LENGTH, self.prior)?;
        self.restored = true;
        Ok(())
    }
}
impl Drop for LengthLimit<'_> {
    fn drop(&mut self) {
        if !self.restored {
            // Panic backstop on a live borrowed connection and a known valid
            // limit. Normal and error returns use the fallible explicit restore.
            let _ = self.conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, self.prior);
        }
    }
}

fn sql_code(error: &anyhow::Error) -> Option<ErrorCode> {
    error
        .chain()
        .find_map(|cause| match cause.downcast_ref::<rusqlite::Error>() {
            Some(rusqlite::Error::SqliteFailure(error, _)) => Some(error.code),
            _ => None,
        })
}
fn key(row: &Row<'_>, offset: usize, valid: impl FnOnce(String) -> bool) -> Result<Option<String>> {
    if row.get::<_, String>(offset)? != "text" {
        return Ok(None);
    }
    let bytes: Vec<u8> = row.get(offset + 1)?;
    if bytes.len() > 160 {
        return Ok(None);
    }
    let Ok(value) = String::from_utf8(bytes) else {
        return Ok(None);
    };
    Ok(valid(value.clone()).then_some(value))
}
fn add_body(total: &mut u64, length: i64) -> bool {
    if length < 0 || length as u64 > MAX_BODY_BYTES {
        return false;
    }
    *total = total.saturating_add(length as u64);
    *total <= MAX_COHORT_BYTES
}
fn matching_rows(conn: &Connection, table: &str, key: &str) -> Result<usize> {
    // The table is selected only from fixed calls below. Never interpolate data.
    let mut stmt = conn.prepare(&format!(
        "SELECT 1 FROM main.{table} WHERE id=?1 AND +id COLLATE BINARY=?1 LIMIT 2"
    ))?;
    let mut rows = stmt.query([key])?;
    let mut count = 0;
    while rows.next()?.is_some() {
        count += 1;
    }
    Ok(count)
}

pub(super) fn inspect(conn: &Connection) -> Result<Observation> {
    if conn.is_autocommit() {
        let snapshot = conn.unchecked_transaction()?;
        let result = inspect_in(&snapshot);
        snapshot.commit()?;
        result
    } else {
        inspect_in(conn)
    }
}
fn inspect_in(conn: &Connection) -> Result<Observation> {
    let limit = LengthLimit::install(conn)?;
    let result = inspect_bounded(conn);
    limit.restore()?;
    match result {
        Ok(observation) => Ok(observation),
        Err(error) if sql_code(&error) == Some(ErrorCode::TooBig) => {
            Ok(Observation::BudgetExceeded)
        }
        Err(error) if sql_code(&error) == Some(ErrorCode::OperationInterrupted) => Err(error),
        Err(_) => Ok(Observation::Unavailable),
    }
}
fn inspect_bounded(conn: &Connection) -> Result<Observation> {
    let mut cohort = Vec::new();
    let mut seen: [BTreeSet<String>; 4] = Default::default();
    let mut total_bytes = 0;
    {
        // Prefixes bound key materialization even with damaged table constraints.
        // Body sizes are inspected before the existing validator can read bodies.
        let mut stmt = conn.prepare(&format!(
            "SELECT
            typeof(id),substr(CAST(id AS BLOB),1,161),
            typeof(event_id),substr(CAST(event_id AS BLOB),1,161),
            typeof(run_id),substr(CAST(run_id AS BLOB),1,161),
            typeof(attempt_id),substr(CAST(attempt_id AS BLOB),1,161),
            typeof(receipt_json),length(CAST(receipt_json AS BLOB))
            FROM main.automation_receipts LIMIT {}",
            MAX_ROWS + 1
        ))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            if cohort.len() == MAX_ROWS {
                return Ok(Observation::BudgetExceeded);
            }
            let Some(id) = key(row, 0, |v| ReceiptId::new(v).is_ok())? else {
                return Ok(Observation::InvalidMetadata);
            };
            let Some(event) = key(row, 2, |v| EventId::new(v).is_ok())? else {
                return Ok(Observation::InvalidMetadata);
            };
            let Some(run) = key(row, 4, |v| RunId::new(v).is_ok())? else {
                return Ok(Observation::InvalidMetadata);
            };
            let Some(attempt) = key(row, 6, |v| AttemptId::new(v).is_ok())? else {
                return Ok(Observation::InvalidMetadata);
            };
            for (set, value) in seen.iter_mut().zip([&id, &event, &run, &attempt]) {
                if !set.insert(value.clone()) {
                    return Ok(Observation::InvalidMetadata);
                }
            }
            if row.get::<_, String>(8)? != "text" {
                return Ok(Observation::InvalidMetadata);
            }
            if !add_body(&mut total_bytes, row.get(9)?) {
                return Ok(Observation::BudgetExceeded);
            }
            cohort.push((id, event, run, attempt));
        }
    }
    let mut missing = BTreeSet::new();
    for (id, event, run, attempt) in &cohort {
        for (table, key) in [("automation_runs", run), ("automation_attempts", attempt)] {
            match matching_rows(conn, table, key)? {
                0 => {
                    missing.insert(id.clone());
                }
                1 => {}
                _ => return Ok(Observation::InvalidMetadata),
            }
        }
        let mut stmt = conn.prepare("SELECT typeof(event_json),length(CAST(event_json AS BLOB)) FROM main.automation_events WHERE event_id=?1 AND +event_id COLLATE BINARY=?1 LIMIT 2")?;
        let mut rows = stmt.query([event])?;
        if let Some(row) = rows.next()? {
            if row.get::<_, String>(0)? != "text" {
                return Ok(Observation::InvalidMetadata);
            }
            if !add_body(&mut total_bytes, row.get(1)?) {
                return Ok(Observation::BudgetExceeded);
            }
            if rows.next()?.is_some() {
                return Ok(Observation::InvalidMetadata);
            }
        } else {
            missing.insert(id.clone());
        }
    }
    let mut invalid = 0;
    for (id, _, _, _) in &cohort {
        if missing.contains(id) {
            invalid += 1;
            continue;
        }
        match super::super::receipts::read_receipt(conn, id) {
            Ok(Some(receipt)) => {
                // The published revision lookup also needs an unambiguous row.
                let mut stmt = conn.prepare("SELECT typeof(revision) FROM main.automation_rich_definition_revisions WHERE automation_id=?1 AND +automation_id COLLATE BINARY=?1 AND (revision=?2 OR CAST(revision AS NUMERIC)=?2) LIMIT 2")?;
                let mut rows = stmt.query(rusqlite::params![
                    receipt.automation_id.as_str(),
                    i64::try_from(receipt.automation_revision.get())?
                ])?;
                if let Some(row) = rows.next()? {
                    if row.get::<_, String>(0)? != "integer" || rows.next()?.is_some() {
                        return Ok(Observation::InvalidMetadata);
                    }
                }
            }
            Ok(None) => {
                invalid += 1;
            }
            Err(error) => match sql_code(&error) {
                Some(ErrorCode::TooBig) => return Ok(Observation::BudgetExceeded),
                Some(ErrorCode::OperationInterrupted) => return Err(error),
                Some(_) => return Ok(Observation::Unavailable),
                None => {
                    invalid += 1;
                }
            },
        }
    }
    Ok(if invalid == 0 {
        Observation::Verified(cohort.len() as u64)
    } else {
        Observation::Inconsistent(invalid)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Connection) {
        let (root, conn, receipt) =
            crate::automations::receipts::tests::doctor_committed_receipt_fixture();
        assert!(
            crate::automations::receipts::read_receipt(&conn, receipt.receipt_id.as_str())
                .unwrap()
                .is_some()
        );
        (root, conn)
    }
    #[test]
    fn verifies_a_genuine_committed_receipt_without_writes() {
        let (_root, conn) = fixture();
        let changes = conn.total_changes();
        assert_eq!(inspect(&conn).unwrap(), Observation::Verified(1));
        assert_eq!(conn.total_changes(), changes);
    }
    #[test]
    fn rejects_independent_body_event_index_run_and_attempt_tampering() {
        for sql in [
            "DROP TRIGGER automation_receipts_no_update; UPDATE automation_receipts SET receipt_json=json_set(receipt_json,'$.privacy.notes','PRIVATE_DOCTOR_TAMPER');",
            "DROP TRIGGER automation_receipts_no_update; UPDATE automation_receipts SET receipt_digest=printf('%064d',0);",
            "DROP TRIGGER automation_events_no_update; UPDATE automation_events SET event_json=json_set(event_json,'$.summary','PRIVATE_DOCTOR_TAMPER');",
            "UPDATE automation_runs SET familiar_id='PRIVATE_DOCTOR_TAMPER';",
            "DROP TRIGGER automation_attempts_terminal_immutable; UPDATE automation_attempts SET occurrence_fence_generation=2;",
        ] {
            let (_root, conn) = fixture();
            conn.execute_batch(sql).unwrap();
            assert_eq!(inspect(&conn).unwrap(), Observation::Inconsistent(1));
        }
    }
    #[test]
    fn numeric_affinity_cannot_alias_a_text_identity() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE numeric_keys(id INTEGER); INSERT INTO numeric_keys VALUES(1);",
        )
        .unwrap();
        for key in ["1", "01"] {
            assert_eq!(matching_rows(&conn, "numeric_keys", key).unwrap(), 0);
        }
        conn.execute_batch("CREATE TABLE text_keys(id TEXT COLLATE NOCASE); INSERT INTO text_keys VALUES('01'),('ABC');").unwrap();
        assert_eq!(matching_rows(&conn, "text_keys", "01").unwrap(), 1);
        assert_eq!(matching_rows(&conn, "text_keys", "1").unwrap(), 0);
        assert_eq!(matching_rows(&conn, "text_keys", "abc").unwrap(), 0);
    }
    #[test]
    fn case_variant_revision_cannot_supply_the_exact_identity_commitment() {
        let (_root, conn) = fixture();
        conn.execute_batch("ALTER TABLE automation_rich_definition_revisions RENAME TO saved_revisions;
            CREATE TABLE automation_rich_definition_revisions(automation_id TEXT COLLATE NOCASE, revision INTEGER, integrity TEXT);
            INSERT INTO automation_rich_definition_revisions
                SELECT upper(automation_id), automation_revision, definition_digest FROM automation_receipts;
            INSERT INTO automation_rich_definition_revisions
                SELECT automation_id, automation_revision, printf('%064d',0) FROM automation_runs;
            UPDATE automation_runs SET definition_digest=printf('%064d',0);").unwrap();
        assert_eq!(inspect(&conn).unwrap(), Observation::Inconsistent(1));
    }
    #[test]
    fn ignores_temp_shadows_for_every_commitment_table() {
        for table in [
            "automation_receipts",
            "automation_events",
            "automation_runs",
            "automation_attempts",
            "automation_rich_definition_revisions",
        ] {
            let (_root, conn) = fixture();
            conn.execute_batch(&format!("CREATE TEMP TABLE {table}(PRIVATE_shadow TEXT);"))
                .unwrap();
            assert_eq!(
                inspect(&conn).unwrap(),
                Observation::Verified(1),
                "temporary table shadow changed the main-store verdict"
            );
        }
    }
    #[test]
    fn restores_the_length_limit_after_success_and_invalid_commitments() {
        let (_root, conn) = fixture();
        let limit = conn.limit(Limit::SQLITE_LIMIT_LENGTH).unwrap();
        assert_eq!(inspect(&conn).unwrap(), Observation::Verified(1));
        assert_eq!(conn.limit(Limit::SQLITE_LIMIT_LENGTH).unwrap(), limit);
        conn.execute_batch("DROP TRIGGER automation_receipts_no_update; UPDATE automation_receipts SET receipt_digest=printf('%064d',0);").unwrap();
        assert_eq!(inspect(&conn).unwrap(), Observation::Inconsistent(1));
        assert_eq!(conn.limit(Limit::SQLITE_LIMIT_LENGTH).unwrap(), limit);
    }
    #[test]
    fn never_raises_a_stricter_caller_limit_and_restores_after_budget_refusal() {
        let (_root, conn) = fixture();
        conn.execute_batch("DROP TRIGGER automation_attempts_terminal_immutable;")
            .unwrap();
        conn.execute(
            "UPDATE automation_attempts SET state_reason=?1",
            ["PRIVATE_BOUNDED_REASON".repeat(2000)],
        )
        .unwrap();
        let original = conn
            .set_limit(Limit::SQLITE_LIMIT_LENGTH, 32 * 1024)
            .unwrap();
        assert_eq!(inspect(&conn).unwrap(), Observation::BudgetExceeded);
        assert_eq!(conn.limit(Limit::SQLITE_LIMIT_LENGTH).unwrap(), 32 * 1024);
        conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, original)
            .unwrap();
    }
    #[test]
    fn doctor_reports_verified_base_commitments_without_claiming_authority_evidence() {
        let (_root, conn) = fixture();
        let report = crate::automations::doctor::inspect(&conn, chrono::Utc::now()).unwrap();
        let value = serde_json::to_value(report).unwrap();
        let delivery = value["observations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["category"] == "delivery_receipts")
            .unwrap();
        assert_eq!(delivery["facts"]["receipt_commitments_checked"], 1);
        assert_eq!(delivery["facts"]["invalid_receipt_commitments"], 0);
        assert_eq!(
            delivery["gaps"],
            serde_json::json!(["receipt_authority_not_verified"])
        );
    }
    #[test]
    fn doctor_reports_corruption_without_disclosing_receipt_body() {
        let (_root, conn) = fixture();
        conn.execute_batch("DROP TRIGGER automation_receipts_no_update; UPDATE automation_receipts SET receipt_json=json_set(receipt_json,'$.privacy.notes','PRIVATE_DOCTOR_TAMPER');").unwrap();
        let report = crate::automations::doctor::inspect(&conn, chrono::Utc::now()).unwrap();
        let value = serde_json::to_value(report).unwrap();
        assert!(!value.to_string().contains("PRIVATE_DOCTOR_TAMPER"));
        let delivery = value["observations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["category"] == "delivery_receipts")
            .unwrap();
        assert_eq!(delivery["facts"]["receipt_commitments_checked"], 1);
        assert_eq!(delivery["facts"]["invalid_receipt_commitments"], 1);
        assert_eq!(
            delivery["gaps"],
            serde_json::json!([
                "receipt_commitments_inconsistent",
                "receipt_authority_not_verified"
            ])
        );
    }
    #[test]
    fn report_rejects_forged_commitment_counts_and_gap_claims() {
        use crate::automations::doctor::{Category, Fact, Gap};
        let (_root, conn) = fixture();
        let base = crate::automations::doctor::inspect(&conn, chrono::Utc::now()).unwrap();
        for mutate in [
            (|o: &mut crate::automations::doctor::Observation| {
                o.facts.insert(Fact::ReceiptCommitmentsChecked, 2);
            }) as fn(&mut crate::automations::doctor::Observation),
            |o| {
                o.facts.insert(Fact::InvalidReceiptCommitments, 1);
            },
            |o| {
                o.facts.insert(Fact::ReceiptCommitmentsChecked, 257);
                o.facts.insert(Fact::StoredReceipts, 257);
            },
            |o| o.gaps.clear(),
            |o| {
                o.gaps = vec![
                    Gap::ReceiptCommitmentsInconsistent,
                    Gap::ReceiptAuthorityNotVerified,
                ]
            },
            |o| {
                o.gaps = vec![
                    Gap::ReceiptCommitmentsUnavailable,
                    Gap::ReceiptAuthorityNotVerified,
                ]
            },
        ] {
            let mut report = base.clone();
            let delivery = report
                .observations
                .iter_mut()
                .find(|o| o.category == Category::DeliveryReceipts)
                .unwrap();
            mutate(delivery);
            assert!(
                report.validate().is_err(),
                "forged commitment evidence was accepted"
            );
        }
        let mut legacy = base;
        let delivery = legacy
            .observations
            .iter_mut()
            .find(|o| o.category == Category::DeliveryReceipts)
            .unwrap();
        delivery.facts.remove(&Fact::ReceiptCommitmentsChecked);
        delivery.facts.remove(&Fact::InvalidReceiptCommitments);
        delivery.gaps = vec![Gap::ReceiptIntegrityNotVerified];
        legacy.validate().unwrap();
    }
    #[test]
    fn bounds_the_whole_body_cohort_before_parsing_any_partial_prefix() {
        for size in [8, 9] {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(
                "CREATE TABLE automation_receipts(id,event_id,run_id,attempt_id,receipt_json);",
            )
            .unwrap();
            for n in 0..size {
                conn.execute(
                    "INSERT INTO automation_receipts VALUES(?1,?2,?3,?4,?5)",
                    rusqlite::params![
                        format!("receipt-{n}"),
                        format!("evt{n:029}"),
                        format!("run-{n}"),
                        format!("attempt-{n}"),
                        " ".repeat(MAX_BODY_BYTES as usize)
                    ],
                )
                .unwrap();
            }
            assert_eq!(
                inspect(&conn).unwrap(),
                if size == 9 {
                    Observation::BudgetExceeded
                } else {
                    Observation::Unavailable
                }
            );
        }
    }
    #[test]
    fn refuses_oversized_event_bodies_before_the_existing_validator_reads_them() {
        let (_root, conn) = fixture();
        conn.execute_batch("DROP TRIGGER automation_events_no_update;")
            .unwrap();
        conn.execute("UPDATE automation_events SET event_json=json_set(event_json,'$.summary',?1) WHERE event_id IN (SELECT event_id FROM main.automation_receipts)", ["PRIVATE_BOUNDED_EVENT".repeat(MAX_BODY_BYTES as usize / 10)]).unwrap();
        let limit = conn.limit(Limit::SQLITE_LIMIT_LENGTH).unwrap();
        assert_eq!(inspect(&conn).unwrap(), Observation::BudgetExceeded);
        assert_eq!(conn.limit(Limit::SQLITE_LIMIT_LENGTH).unwrap(), limit);
    }
    #[test]
    fn refuses_nontext_keys_before_reading_evidence() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE automation_receipts(id,event_id,run_id,attempt_id,receipt_json); INSERT INTO automation_receipts VALUES(12,'evt00000000000000000000000000001','run-key','attempt-key','{}');").unwrap();
        assert_eq!(inspect(&conn).unwrap(), Observation::InvalidMetadata);
    }
    #[test]
    fn refuses_an_over_budget_cohort_before_validating_a_partial_prefix() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE automation_receipts(id,event_id,run_id,attempt_id,receipt_json);",
        )
        .unwrap();
        for n in 0..=MAX_ROWS {
            conn.execute(
                "INSERT INTO automation_receipts VALUES(?1,?2,?3,?4,'{}')",
                rusqlite::params![
                    format!("receipt-{n}"),
                    format!("evt{n:029}"),
                    format!("run-{n}"),
                    format!("attempt-{n}")
                ],
            )
            .unwrap();
        }
        assert_eq!(inspect(&conn).unwrap(), Observation::BudgetExceeded);
    }
}
