//! Audit-only inventory, not proposal recovery or execution authority.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use coven_threads_core::{
    AuditEventType, Channel, ProposalApprovalAuditDetail, ProposalId,
    ProposalWindowCloseAuditDetail, SurfaceId, ThreadId, WardAuditRecord, WindowCloseReason,
    WriterId, WARD_AUDIT_SCHEMA_STATE_CURRENT_V020, WARD_AUDIT_SCHEMA_STATE_SQL,
};
use rusqlite::{types::ValueRef, Connection, Row};
use serde::Serialize;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

const MAX_DECODED_BYTES: usize = 32 * 1024 * 1024;
const MAX_ARTIFACT_ENTRIES: usize = 4096;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Census {
    format: &'static str,
    complete: bool,
    through_audit_id: i64,
    scanned_history_rows: u32,
    unresolved_histories: usize,
    artifact_observation: &'static str,
    unattributed_artifact_entries: usize,
    windows: Vec<Window>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Window {
    proposal_id: Option<Uuid>,
    classification: Classification,
    opening_rows: Vec<i64>,
    terminal_rows: Vec<i64>,
    apply_intent_rows: Vec<i64>,
    close_reason: Option<WindowCloseReason>,
    issues: Vec<Issue>,
    artifacts: Artifacts,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Classification {
    TypedTerminalRecorded,
    InconsistentHistory,
    UnprovableApply,
    OpenUnverified,
    QuarantinedOpening,
    UntrustedArtifacts,
    OrphanedOpening,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Issue {
    InvalidRecord,
    MultipleOpenings,
    MultipleTerminals,
    MissingTypedClose,
    TerminalBeforeOpening,
    ApplyIntentOrderMismatch,
    FamiliarScopeMismatch,
    TargetScopeMismatch,
    ChannelScopeMismatch,
}

#[derive(Debug, Default, Serialize)]
struct Artifacts {
    pending: usize,
    claims: usize,
    quarantine: usize,
    untrusted: usize,
}

#[derive(Default)]
struct History {
    openings: Vec<CheckedRow>,
    terminals: Vec<CheckedRow>,
    intents: Vec<i64>,
}

struct CheckedRow {
    id: i64,
    familiar: String,
    valid: bool,
    close: Option<WindowCloseReason>,
    scope: Option<Scope>,
}

struct Scope {
    targets: Vec<SurfaceId>,
    channel: Option<Channel>,
}

pub(crate) fn run(home: &Path, max_rows: u32, json: bool) -> Result<()> {
    let report = load(home, max_rows)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Audit-only census through row {}: {} opened-window histories, {} unresolved",
            report.through_audit_id,
            report.windows.len(),
            report.unresolved_histories
        );
        println!("Pending artifacts are non-atomic, unverified observations; no repair or authority is granted.");
        for window in &report.windows {
            let proposal = window
                .proposal_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "(unbound)".to_owned());
            println!(
                "{proposal}  {:?}  openings={:?} terminals={:?} issues={:?}",
                window.classification, window.opening_rows, window.terminal_rows, window.issues
            );
        }
    }
    Ok(())
}

pub(crate) fn load(home: &Path, max_rows: u32) -> Result<Census> {
    ensure!(
        (1..=100_000).contains(&max_rows),
        "max-audit-rows must be 1..=100000"
    );
    let conn = crate::store::open_existing_store_read_only(&home.join(crate::STORE_FILE_NAME))?
        .context("audit census requires an existing Coven store")?;
    // All audit reads, including schema classification, share one SQLite snapshot.
    let tx = conn.unchecked_transaction()?;
    let report = inspect(&tx, home, max_rows)?;
    tx.commit()?;
    Ok(report)
}

fn inspect(conn: &Connection, home: &Path, max_rows: u32) -> Result<Census> {
    let schema: String = conn.query_row(WARD_AUDIT_SCHEMA_STATE_SQL, [], |row| row.get(0))?;
    ensure!(
        schema == WARD_AUDIT_SCHEMA_STATE_CURRENT_V020,
        "audit census requires current_v020 ward_audit; found {schema}; no migration performed"
    );
    let through_audit_id = conn.query_row(
        "SELECT COALESCE(MAX(id), 0) FROM main.ward_audit",
        [],
        |row| row.get(0),
    )?;
    let mut statement = conn.prepare(
        "SELECT id, event_type, proposal_id, familiar_id, ward_version, ward_hash,
                CAST(tier AS TEXT), decision, approver, diff_hash, detail,
                files_touched, channel, thread_id, submitted_at, decided_at
         FROM main.ward_audit
         WHERE event_type IN ('proposal_window_opened', 'proposal_approved',
                              'proposal_rejected', 'proposal_vetoed', 'proposal_expired')
            OR decision = 'proposal-apply-intent'
         ORDER BY id LIMIT ?1",
    )?;
    let mut rows = statement.query([i64::from(max_rows) + 1])?;
    let mut histories = BTreeMap::<(Option<Uuid>, Option<i64>), History>::new();
    let mut scanned_history_rows = 0;
    let mut decoded_bytes = 0;
    while let Some(row) = rows.next()? {
        ensure!(
            scanned_history_rows < max_rows,
            "audit census exceeds --max-audit-rows={max_rows}; no complete report produced"
        );
        for column in 0..row.as_ref().column_count() {
            decoded_bytes += match row.get_ref(column)? {
                ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len(),
                _ => 8,
            };
        }
        ensure!(
            decoded_bytes <= MAX_DECODED_BYTES,
            "audit census exceeds 32 MiB decoding budget; no complete report produced"
        );
        scanned_history_rows += 1;
        let id: i64 = row.get(0)?;
        let event: String = row.get(1)?;
        let proposal = row
            .get::<_, Option<String>>(2)?
            .as_deref()
            .and_then(|id| Uuid::parse_str(id).ok());
        // Unbound rows must not accidentally join unrelated NULL proposal ids.
        let unbound_row = proposal.is_none().then_some(id);
        let history = histories.entry((proposal, unbound_row)).or_default();
        if row.get::<_, String>(7)? == "proposal-apply-intent" {
            history.intents.push(id);
        }
        if event == "validation_verdict" {
            continue;
        }
        let familiar: String = row.get(3)?;
        let (valid, close, scope) = match decode_record(row) {
            Ok(mut record)
                if !familiar.trim().is_empty() && record.validate_event_detail().is_ok() =>
            {
                let close = window_close(&record)?;
                record.files_touched.sort();
                record.files_touched.dedup();
                (
                    true,
                    close,
                    Some(Scope {
                        targets: record.files_touched,
                        channel: record.channel,
                    }),
                )
            }
            // Malformed stored data is reported by row id, without exposing
            // private detail text through a parser's error message.
            Ok(_) | Err(_) => (false, None, None),
        };
        let checked = CheckedRow {
            id,
            familiar,
            valid,
            close,
            scope,
        };
        if event == "proposal_window_opened" {
            history.openings.push(checked);
        } else {
            history.terminals.push(checked);
        }
    }
    let (mut artifacts, unattributed_artifact_entries) = observe_artifacts(home)?;
    let windows: Vec<_> = histories
        .into_iter()
        .filter(|(_, history)| !history.openings.is_empty())
        .map(|((proposal_id, _), history)| {
            let artifacts = proposal_id
                .and_then(|id| artifacts.remove(&id))
                .unwrap_or_default();
            classify(proposal_id, history, artifacts)
        })
        .collect();
    let unresolved_histories = windows
        .iter()
        .filter(|window| !matches!(window.classification, Classification::TypedTerminalRecorded))
        .count();
    Ok(Census {
        format: "coven.ward-window-census.v1",
        complete: true,
        through_audit_id,
        scanned_history_rows,
        unresolved_histories,
        artifact_observation: "non_atomic_unverified_names_only",
        unattributed_artifact_entries,
        windows,
    })
}

fn decode_record(row: &Row<'_>) -> Result<WardAuditRecord> {
    let channel = row
        .get::<_, Option<String>>(12)?
        .map(|value| match value.as_str() {
            "deliberate" => Ok(Channel::Deliberate),
            "forced" => Ok(Channel::Forced),
            "serialization" => Ok(Channel::Serialization),
            "mutation" => Ok(Channel::Mutation),
            _ => bail!("invalid stored channel"),
        })
        .transpose()?;
    Ok(WardAuditRecord {
        event_type: serde_json::from_value(serde_json::Value::String(row.get(1)?))?,
        proposal_id: row
            .get::<_, Option<String>>(2)?
            .map(|id| Uuid::parse_str(&id).map(ProposalId))
            .transpose()?,
        familiar_id: crate::threads_gate::familiar_weave_id(&row.get::<_, String>(3)?),
        ward_version: row.get(4)?,
        ward_hash: row.get(5)?,
        tier: row.get(6)?,
        decision: row.get(7)?,
        approver: row.get::<_, Option<String>>(8)?.map(WriterId),
        diff_hash: row.get(9)?,
        detail: row.get(10)?,
        files_touched: serde_json::from_str(&row.get::<_, String>(11)?)?,
        channel,
        thread_id: row
            .get::<_, Option<String>>(13)?
            .map(|id| Uuid::parse_str(&id).map(ThreadId))
            .transpose()?,
        submitted_at: OffsetDateTime::parse(&row.get::<_, String>(14)?, &Rfc3339)?,
        decided_at: OffsetDateTime::parse(&row.get::<_, String>(15)?, &Rfc3339)?,
    })
}

fn window_close(record: &WardAuditRecord) -> Result<Option<WindowCloseReason>> {
    // A rejected/vetoed no-window row is individually valid in core. It does
    // not close an actual opening. Approval stores its close one level deeper.
    let Some(detail) = record.detail.as_deref() else {
        return Ok(None);
    };
    let close = match record.event_type {
        AuditEventType::ProposalApproved => {
            serde_json::from_str::<ProposalApprovalAuditDetail>(detail)?.window_close
        }
        AuditEventType::ProposalRejected | AuditEventType::ProposalVetoed => Some(
            serde_json::from_str::<ProposalWindowCloseAuditDetail>(detail)?,
        ),
        _ => None,
    };
    Ok(close.map(|close| close.reason))
}

fn classify(proposal_id: Option<Uuid>, history: History, artifacts: Artifacts) -> Window {
    let mut issues = Vec::new();
    if proposal_id.is_none()
        || history
            .openings
            .iter()
            .chain(&history.terminals)
            .any(|row| !row.valid)
    {
        issues.push(Issue::InvalidRecord);
    }
    if history.openings.len() != 1 {
        issues.push(Issue::MultipleOpenings);
    }
    if history.terminals.len() > 1 {
        issues.push(Issue::MultipleTerminals);
    }
    if history.terminals.iter().any(|row| row.close.is_none()) {
        issues.push(Issue::MissingTypedClose);
    }
    let opening = &history.openings[0];
    if history.terminals.iter().any(|row| row.id < opening.id) {
        issues.push(Issue::TerminalBeforeOpening);
    }
    if history
        .intents
        .iter()
        .any(|id| *id <= opening.id || history.terminals.iter().any(|terminal| *id >= terminal.id))
    {
        issues.push(Issue::ApplyIntentOrderMismatch);
    }
    if history
        .openings
        .iter()
        .chain(&history.terminals)
        .any(|row| row.familiar != opening.familiar)
    {
        issues.push(Issue::FamiliarScopeMismatch);
    }
    if let Some(opened_scope) = &opening.scope {
        let terminal_scopes: Vec<_> = history
            .terminals
            .iter()
            .filter_map(|row| row.scope.as_ref())
            .collect();
        if terminal_scopes
            .iter()
            .any(|scope| scope.targets != opened_scope.targets)
        {
            issues.push(Issue::TargetScopeMismatch);
        }
        if terminal_scopes.iter().any(|scope| {
            matches!((opened_scope.channel, scope.channel), (Some(opened), Some(closed)) if opened != closed)
        }) {
            issues.push(Issue::ChannelScopeMismatch);
        }
    }
    let classification = if !issues.is_empty() {
        Classification::InconsistentHistory
    } else if !history.terminals.is_empty() {
        Classification::TypedTerminalRecorded
    } else if !history.intents.is_empty() {
        Classification::UnprovableApply
    } else if artifacts.pending + artifacts.claims > 0 {
        Classification::OpenUnverified
    } else if artifacts.quarantine > 0 {
        Classification::QuarantinedOpening
    } else if artifacts.untrusted > 0 {
        Classification::UntrustedArtifacts
    } else {
        Classification::OrphanedOpening
    };
    Window {
        proposal_id,
        classification,
        opening_rows: history.openings.iter().map(|row| row.id).collect(),
        terminal_rows: history.terminals.iter().map(|row| row.id).collect(),
        apply_intent_rows: history.intents,
        close_reason: history.terminals.first().and_then(|row| row.close.clone()),
        issues,
        artifacts,
    }
}

fn observe_artifacts(home: &Path) -> Result<(BTreeMap<Uuid, Artifacts>, usize)> {
    let mut artifacts = BTreeMap::<Uuid, Artifacts>::new();
    let mut entries = 0;
    let mut unattributed = 0;
    for (directory, quarantine) in [
        (home.join("pending"), false),
        (home.join("pending/quarantine"), true),
    ] {
        match fs::symlink_metadata(&directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("observing pending artifact directory"),
            Ok(metadata) => ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "pending artifact directory must be a real directory"
            ),
        }
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            entries += 1;
            ensure!(
                entries <= MAX_ARTIFACT_ENTRIES,
                "audit census exceeds 4096 artifact entries; no complete report produced"
            );
            let name = entry.file_name();
            if !quarantine
                && matches!(
                    name.to_str(),
                    Some("quarantine" | ".quota.lock" | ".scheduler-cursor")
                )
            {
                continue;
            }
            let Some((id, suffix)) = name.to_str().and_then(artifact_id) else {
                unattributed += 1;
                continue;
            };
            let item = artifacts.entry(id).or_default();
            if !entry.file_type()?.is_file() {
                item.untrusted += 1;
            } else if quarantine {
                item.quarantine += 1;
            } else {
                match suffix {
                    "" => item.pending += 1,
                    ".approve.deciding" | ".reject.deciding" => item.claims += 1,
                    _ => item.untrusted += 1,
                }
            }
        }
    }
    Ok((artifacts, unattributed))
}

fn artifact_id(name: &str) -> Option<(Uuid, &str)> {
    let (stem, suffix) = name.rsplit_once(".json")?;
    let start = stem.len().checked_sub(36)?;
    let prefix = stem.get(..start)?;
    if !prefix.is_empty() && !prefix.ends_with('-') {
        return None;
    }
    Uuid::parse_str(stem.get(start..)?)
        .ok()
        .map(|id| (id, suffix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use coven_threads_core::{ProposalWindowAuditDetail, WARD_AUDIT_SCHEMA_SQL};
    use rusqlite::params;
    use serde_json::json;

    fn store() -> Result<(tempfile::TempDir, Connection)> {
        let home = tempfile::tempdir()?;
        let conn = Connection::open(home.path().join(crate::STORE_FILE_NAME))?;
        conn.execute_batch(WARD_AUDIT_SCHEMA_SQL)?;
        Ok((home, conn))
    }

    fn opening() -> Result<String> {
        let now = OffsetDateTime::parse("2026-09-13T00:00:00Z", &Rfc3339)?;
        Ok(serde_json::to_string(&ProposalWindowAuditDetail {
            approval_path_label: "familiar_review".to_owned(),
            deadline: now + time::Duration::minutes(5),
            earliest_close: now + time::Duration::minutes(1),
            evidence_replay_hash_hex: "00".repeat(32),
            affected_regions: Vec::new(),
        })?)
    }

    fn append(
        conn: &Connection,
        id: &str,
        event: &str,
        detail: Option<&str>,
        familiar: &str,
    ) -> Result<i64> {
        conn.execute(
            "INSERT INTO ward_audit (
                event_type, proposal_id, familiar_id, ward_hash, decision,
                approver, detail, files_touched, channel, submitted_at, decided_at
             ) VALUES (?1, ?2, ?3, ?4, 'synthetic-census-only', 'synthetic-reviewer',
                       ?5, '[\"reviewed/skill.md\"]', 'mutation',
                       '2026-09-13T00:00:00Z', '2026-09-13T00:05:00Z')",
            params![event, id, familiar, vec![0_u8; 32], detail],
        )?;
        Ok(conn.last_insert_rowid())
    }

    fn close(reason: WindowCloseReason) -> Result<(String, String)> {
        let replay_hash_matched = match reason {
            WindowCloseReason::Applied => Some(true),
            WindowCloseReason::Vetoed | WindowCloseReason::Superseded => None,
            _ => Some(false),
        };
        let event = match reason {
            WindowCloseReason::Applied => "proposal_approved",
            WindowCloseReason::Vetoed => "proposal_vetoed",
            _ => "proposal_rejected",
        };
        let detail = ProposalWindowCloseAuditDetail {
            reason,
            replay_hash_matched,
            rationale: None,
        };
        let detail = if event == "proposal_approved" {
            serde_json::to_string(&ProposalApprovalAuditDetail {
                approval_path_label: "familiar_review".to_owned(),
                rationale: None,
                window_close: Some(detail),
            })?
        } else {
            serde_json::to_string(&detail)?
        };
        Ok((event.to_owned(), detail))
    }

    #[test]
    fn all_normative_closes_are_typed_and_human_no_window_is_excluded() -> Result<()> {
        let (home, conn) = store()?;
        let reasons = [
            WindowCloseReason::Applied,
            WindowCloseReason::Vetoed,
            WindowCloseReason::EvidenceDiverged,
            WindowCloseReason::RevalidationFailed,
            WindowCloseReason::Superseded,
        ];
        for (index, reason) in reasons.iter().enumerate() {
            let id = Uuid::from_u128(index as u128 + 1).to_string();
            append(
                &conn,
                &id,
                "proposal_window_opened",
                Some(&opening()?),
                "sage",
            )?;
            let (event, detail) = close(reason.clone())?;
            append(&conn, &id, &event, Some(&detail), "sage")?;
        }
        let human =
            json!({"approval_path_label": "human_review", "rationale": null, "window_close": null})
                .to_string();
        append(
            &conn,
            &Uuid::new_v4().to_string(),
            "proposal_approved",
            Some(&human),
            "sage",
        )?;
        drop(conn);
        let bytes = fs::read(home.path().join(crate::STORE_FILE_NAME))?;
        let report = load(home.path(), 100)?;
        assert_eq!(report.windows.len(), 5);
        for (window, reason) in report.windows.iter().zip(reasons) {
            assert!(matches!(
                window.classification,
                Classification::TypedTerminalRecorded
            ));
            assert_eq!(window.close_reason, Some(reason));
            assert!(window.issues.is_empty());
        }
        assert_eq!(fs::read(home.path().join(crate::STORE_FILE_NAME))?, bytes);
        assert!(!home.path().join("pending").exists());
        Ok(())
    }

    #[test]
    fn null_and_human_closes_do_not_close_openings() -> Result<()> {
        let (home, conn) = store()?;
        for (event, detail) in [
            ("proposal_rejected", None),
            ("proposal_vetoed", None),
            (
                "proposal_approved",
                Some(
                    r#"{"approval_path_label":"human_review","window_close":null,"rationale":null}"#,
                ),
            ),
        ] {
            let id = Uuid::new_v4().to_string();
            // Current insert guards prohibit an untyped terminal AFTER an
            // opening. Seed reversed legacy history without disabling them.
            append(&conn, &id, event, detail, "sage")?;
            append(
                &conn,
                &id,
                "proposal_window_opened",
                Some(&opening()?),
                "sage",
            )?;
        }
        let report = load(home.path(), 100)?;
        for window in report.windows {
            assert!(matches!(
                window.classification,
                Classification::InconsistentHistory
            ));
            assert!(window
                .issues
                .iter()
                .any(|issue| matches!(issue, Issue::MissingTypedClose)));
        }
        Ok(())
    }

    #[test]
    fn duplicate_uuid_spellings_scope_and_order_are_inconsistent() -> Result<()> {
        let (home, conn) = store()?;
        let (event, detail) = close(WindowCloseReason::Superseded)?;
        let id = Uuid::from_u128(0xabcdef).to_string();
        append(
            &conn,
            &id,
            &event,
            Some(&detail),
            "other-synthetic-familiar",
        )?;
        append(
            &conn,
            &id,
            "proposal_window_opened",
            Some(&opening()?),
            "sage",
        )?;
        append(
            &conn,
            &id.to_uppercase(),
            "proposal_window_opened",
            Some(&opening()?),
            "sage",
        )?;
        append(&conn, &id.to_uppercase(), &event, Some(&detail), "sage")?;
        let report = load(home.path(), 100)?;
        assert_eq!(report.windows.len(), 1);
        let issues = &report.windows[0].issues;
        for expected in [
            Issue::MultipleOpenings,
            Issue::MultipleTerminals,
            Issue::TerminalBeforeOpening,
            Issue::FamiliarScopeMismatch,
        ] {
            assert!(issues.contains(&expected), "{issues:?}");
        }
        Ok(())
    }

    #[test]
    fn malformed_expired_and_wrong_replay_legacy_details_never_certify_close() -> Result<()> {
        let (home, conn) = store()?;
        for detail in [
            "not-json-private-marker",
            r#"{"reason":"expired","replay_hash_matched":null,"rationale":null}"#,
            r#"{"reason":"revalidation_failed","replay_hash_matched":true,"rationale":null}"#,
            r#"{"reason":"vetoed","replay_hash_matched":null,"rationale":null}"#,
        ] {
            let id = Uuid::new_v4().to_string();
            append(&conn, &id, "proposal_rejected", Some(detail), "sage")?;
            append(
                &conn,
                &id,
                "proposal_window_opened",
                Some(&opening()?),
                "sage",
            )?;
        }
        let report = load(home.path(), 100)?;
        assert_eq!(report.unresolved_histories, 4);
        for window in &report.windows {
            assert!(window.issues.contains(&Issue::InvalidRecord));
            assert!(window.close_reason.is_none());
        }
        assert!(!serde_json::to_string(&report)?.contains("not-json-private-marker"));
        Ok(())
    }

    #[test]
    fn changed_target_and_channel_context_are_reported_without_disclosing_paths() -> Result<()> {
        let (home, conn) = store()?;
        let id = Uuid::new_v4().to_string();
        append(
            &conn,
            &id,
            "proposal_window_opened",
            Some(&opening()?),
            "sage",
        )?;
        let (_, detail) = close(WindowCloseReason::Superseded)?;
        conn.execute(
            "INSERT INTO ward_audit (
                event_type, proposal_id, familiar_id, ward_hash, decision, detail,
                files_touched, channel, submitted_at, decided_at
             ) VALUES ('proposal_rejected', ?1, 'sage', X'00', 'rejected', ?2,
                       '[\"other-private-target\"]', 'forced',
                       '2026-09-13T00:00:00Z', '2026-09-13T00:05:00Z')",
            params![id, detail],
        )?;
        let report = load(home.path(), 100)?;
        assert!(report.windows[0]
            .issues
            .contains(&Issue::TargetScopeMismatch));
        assert!(report.windows[0]
            .issues
            .contains(&Issue::ChannelScopeMismatch));
        assert!(!serde_json::to_string(&report)?.contains("other-private-target"));
        Ok(())
    }

    #[test]
    fn decoding_budget_is_enforced_before_cloning_large_columns() -> Result<()> {
        let (home, conn) = store()?;
        conn.execute(
            "INSERT INTO ward_audit (
                event_type, proposal_id, familiar_id, ward_hash, decision, detail,
                files_touched, submitted_at, decided_at
             ) VALUES ('proposal_window_opened', ?1, 'sage', zeroblob(?2),
                       'window-opened', ?3, '[]',
                       '2026-09-13T00:00:00Z', '2026-09-13T00:00:00Z')",
            params![
                Uuid::new_v4().to_string(),
                i64::try_from(MAX_DECODED_BYTES + 1)?,
                opening()?
            ],
        )?;
        assert!(load(home.path(), 100)
            .unwrap_err()
            .to_string()
            .contains("32 MiB decoding budget"));
        Ok(())
    }

    #[test]
    fn malformed_stored_context_is_explicit_and_private_detail_is_not_output() -> Result<()> {
        let (home, conn) = store()?;
        append(
            &conn,
            "not-a-uuid",
            "proposal_window_opened",
            Some(&opening()?),
            "sage",
        )?;
        let id = Uuid::new_v4().to_string();
        append(
            &conn,
            &id,
            "proposal_window_opened",
            Some(&opening()?),
            "sage",
        )?;
        let (_, detail) = close(WindowCloseReason::Superseded)?;
        conn.execute(
            "INSERT INTO ward_audit (
                event_type, proposal_id, familiar_id, ward_hash, decision, detail,
                files_touched, submitted_at, decided_at
             ) VALUES ('proposal_rejected', ?1, 'sage', X'00', 'rejected', ?2,
                       'not-json-private-marker', 'invalid-time', 'invalid-time')",
            params![id, detail],
        )?;
        let report = load(home.path(), 100)?;
        assert_eq!(report.windows.len(), 2);
        assert!(report
            .windows
            .iter()
            .all(|window| matches!(window.classification, Classification::InconsistentHistory)));
        let output = serde_json::to_string(&report)?;
        assert!(!output.contains("not-json-private-marker"));
        assert!(!output.contains("not-a-uuid"));
        assert!(output.contains("invalid_record"));
        Ok(())
    }

    #[test]
    fn missing_unknown_and_over_budget_stores_never_return_empty_success() -> Result<()> {
        let home = tempfile::tempdir()?;
        let absent = home.path().join("absent");
        assert!(load(&absent, 100)
            .unwrap_err()
            .to_string()
            .contains("existing Coven store"));
        assert!(!absent.exists());
        let conn = Connection::open(home.path().join(crate::STORE_FILE_NAME))?;
        assert!(load(home.path(), 100)
            .unwrap_err()
            .to_string()
            .contains("found missing"));
        conn.execute_batch("CREATE TABLE ward_audit (id INTEGER)")?;
        let bytes = fs::read(home.path().join(crate::STORE_FILE_NAME))?;
        assert!(load(home.path(), 100)
            .unwrap_err()
            .to_string()
            .contains("found unknown"));
        assert_eq!(fs::read(home.path().join(crate::STORE_FILE_NAME))?, bytes);

        let (home, conn) = store()?;
        let id = Uuid::new_v4().to_string();
        append(
            &conn,
            &id,
            "proposal_window_opened",
            Some(&opening()?),
            "sage",
        )?;
        append(
            &conn,
            &id,
            "proposal_window_opened",
            Some(&opening()?),
            "sage",
        )?;
        assert!(load(home.path(), 1)
            .unwrap_err()
            .to_string()
            .contains("no complete report"));
        assert_eq!(load(home.path(), 2)?.scanned_history_rows, 2);
        Ok(())
    }

    #[test]
    fn snapshot_excludes_concurrent_terminal_append() -> Result<()> {
        let (home, conn) = store()?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let id = Uuid::new_v4().to_string();
        let opened = append(
            &conn,
            &id,
            "proposal_window_opened",
            Some(&opening()?),
            "sage",
        )?;
        let read =
            crate::store::open_existing_store_read_only(&home.path().join(crate::STORE_FILE_NAME))?
                .context("reader")?;
        let tx = read.unchecked_transaction()?;
        let _: i64 = tx.query_row("SELECT COUNT(*) FROM ward_audit", [], |row| row.get(0))?;
        let (event, detail) = close(WindowCloseReason::Superseded)?;
        append(&conn, &id, &event, Some(&detail), "sage")?;
        let snapshot = inspect(&tx, home.path(), 100)?;
        assert_eq!(snapshot.through_audit_id, opened);
        assert!(snapshot.windows[0].terminal_rows.is_empty());
        tx.commit()?;
        assert_eq!(load(home.path(), 100)?.windows[0].terminal_rows.len(), 1);
        Ok(())
    }

    #[test]
    fn artifacts_are_bounded_unverified_names_not_execution_authority() -> Result<()> {
        let (home, conn) = store()?;
        let id = Uuid::new_v4().to_string();
        append(
            &conn,
            &id,
            "proposal_window_opened",
            Some(&opening()?),
            "sage",
        )?;
        let pending = home.path().join("pending");
        fs::create_dir_all(pending.join("quarantine"))?;
        let path = pending.join(format!("sage-{id}.json.approve.deciding"));
        fs::write(&path, "not a valid proposal")?;
        let report = load(home.path(), 100)?;
        assert!(matches!(
            report.windows[0].classification,
            Classification::OpenUnverified
        ));
        assert_eq!(report.windows[0].artifacts.claims, 1);
        assert_eq!(fs::read_to_string(&path)?, "not a valid proposal");
        fs::rename(
            &path,
            pending
                .join("quarantine")
                .join(format!("sage-{id}.json.invalid.fixture")),
        )?;
        assert!(matches!(
            load(home.path(), 100)?.windows[0].classification,
            Classification::QuarantinedOpening
        ));
        for index in 0..MAX_ARTIFACT_ENTRIES {
            fs::write(pending.join(format!("unattributed-{index}")), "")?;
        }
        assert!(load(home.path(), 100)
            .unwrap_err()
            .to_string()
            .contains("4096 artifact entries"));
        Ok(())
    }

    #[test]
    fn canonical_pending_and_claim_names_are_observed_without_a_familiar_prefix() -> Result<()> {
        let (home, conn) = store()?;
        let id = Uuid::new_v4().to_string();
        append(
            &conn,
            &id,
            "proposal_window_opened",
            Some(&opening()?),
            "sage",
        )?;
        let pending = home.path().join("pending");
        fs::create_dir(&pending)?;
        for prefix in ["", "sage-"] {
            for suffix in ["", ".approve.deciding", ".reject.deciding", ".unknown"] {
                let path = pending.join(format!("{prefix}{id}.json{suffix}"));
                fs::write(&path, "unverified proposal candidate")?;
                let report = load(home.path(), 100)?;
                let window = &report.windows[0];
                if suffix == ".unknown" {
                    assert!(
                        matches!(window.classification, Classification::UntrustedArtifacts),
                        "{report:?}"
                    );
                    assert_eq!(window.artifacts.untrusted, 1);
                } else {
                    assert!(
                        matches!(window.classification, Classification::OpenUnverified),
                        "{report:?}"
                    );
                    assert_eq!(window.artifacts.pending, usize::from(suffix.is_empty()));
                    assert_eq!(window.artifacts.claims, usize::from(!suffix.is_empty()));
                }
                assert_eq!(report.unattributed_artifact_entries, 0);
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    #[test]
    fn apply_intents_must_follow_the_opening_and_precede_any_terminal() -> Result<()> {
        for order in ["before", "after", "between"] {
            let (home, conn) = store()?;
            let id = Uuid::new_v4().to_string();
            let intent = || -> Result<()> {
                conn.execute(
                    "INSERT INTO ward_audit (
                        event_type, proposal_id, familiar_id, ward_hash, decision,
                        files_touched, submitted_at, decided_at
                     ) VALUES ('validation_verdict', ?1, 'sage', X'00',
                               'proposal-apply-intent', '[]',
                               '2026-09-13T00:00:00Z', '2026-09-13T00:05:00Z')",
                    [&id],
                )?;
                Ok(())
            };
            if order == "before" {
                intent()?;
            }
            append(
                &conn,
                &id,
                "proposal_window_opened",
                Some(&opening()?),
                "sage",
            )?;
            if order == "between" {
                intent()?;
            }
            let (event, detail) = close(WindowCloseReason::Applied)?;
            append(&conn, &id, &event, Some(&detail), "sage")?;
            if order == "after" {
                intent()?;
            }
            let report = load(home.path(), 100)?;
            if order == "between" {
                assert!(
                    matches!(
                        report.windows[0].classification,
                        Classification::TypedTerminalRecorded
                    ),
                    "{report:?}"
                );
            } else {
                assert!(
                    matches!(
                        report.windows[0].classification,
                        Classification::InconsistentHistory
                    ),
                    "{report:?}"
                );
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_pending_directory_is_not_followed() -> Result<()> {
        let (home, _conn) = store()?;
        let outside = tempfile::tempdir()?;
        std::os::unix::fs::symlink(outside.path(), home.path().join("pending"))?;
        assert!(load(home.path(), 100)
            .unwrap_err()
            .to_string()
            .contains("real directory"));
        assert_eq!(fs::read_dir(outside.path())?.count(), 0);
        Ok(())
    }
}
