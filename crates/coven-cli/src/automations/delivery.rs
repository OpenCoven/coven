//! Routine run delivery and settlement (coven#816).
//!
//! Dispatch records that a run *started*; this module finishes it. The
//! reconciliation pass watches every running ledger row, reads the terminal
//! outcome from the Coven session store (the same normalized stream every
//! session produces), captures a bounded log, and — when the definition
//! configures an output target — atomically commits the final assistant
//! payload. Coven, not the model, performs the delivery, and a failed output
//! commit fails the run visibly instead of reporting success.

use std::path::{Path, PathBuf};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    sync::{LazyLock, Mutex, MutexGuard},
};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::runs::{record_run_finish, RunFinish, LOG_ENTRY_MAX_CHARS};

/// How many trailing normalized-stream events a bounded log captures. The
/// budget keeps the newest events; older entries are replaced by a marker.
const BOUNDED_LOG_EVENT_LIMIT: usize = 200;

/// Terminal session statuses, mirroring
/// `store::update_session_terminal_if_active`.
const SESSION_TERMINAL_STATUSES: [&str; 6] = [
    "completed",
    "failed",
    "cancelled",
    "killed",
    "idle",
    "orphaned",
];

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    pub settled_succeeded: usize,
    pub settled_failed: usize,
    pub still_running: usize,
    pub failures: Vec<String>,
}

fn is_terminal_session_status(status: &str) -> bool {
    SESSION_TERMINAL_STATUSES.contains(&status)
}

/// Captures a bounded JSON log of the session's normalized stream: the newest
/// entries that fit the ledger's per-run budget, prefixed by a truncation
/// marker when older entries were dropped. `None` when the session recorded
/// nothing.
pub fn capture_bounded_log(conn: &Connection, session_id: &str) -> Option<String> {
    let total: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE session_id = ?1",
            params![session_id],
            |row| row.get::<_, i64>(0),
        )
        .ok()?
        .try_into()
        .ok()?;
    if total == 0 {
        return None;
    }
    let mut statement = conn
        .prepare(
            "SELECT kind, payload_json, created_at FROM events
             WHERE session_id = ?1
             ORDER BY rowid DESC
             LIMIT ?2",
        )
        .ok()?;
    let rows = statement
        .query_map(params![session_id, BOUNDED_LOG_EVENT_LIMIT as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .ok()?;
    let mut kept_newest_first = Vec::new();
    for row in rows {
        let (kind, payload_json, created_at) = row.ok()?;
        let payload = serde_json::from_str::<Value>(&payload_json).unwrap_or(Value::Null);
        let serialized = serde_json::to_string(&json!({
            "kind": kind,
            "createdAt": created_at,
            "payload": payload,
        }))
        .ok()?;
        kept_newest_first.push(serialized);
        let dropped = total.saturating_sub(kept_newest_first.len());
        if bounded_log_json_len(&kept_newest_first, dropped) > LOG_ENTRY_MAX_CHARS {
            kept_newest_first.pop();
            break;
        }
    }
    let dropped = total.saturating_sub(kept_newest_first.len());
    let marker = (dropped > 0)
        .then(|| {
            serde_json::to_string(&json!({
                "kind": "logTruncated",
                "droppedEntries": dropped,
            }))
        })
        .transpose()
        .ok()?;
    let element_count = kept_newest_first.len() + usize::from(marker.is_some());
    let mut log = String::with_capacity(bounded_log_json_len(&kept_newest_first, dropped));
    log.push('[');
    let mut written = 0;
    if let Some(marker) = marker {
        log.push_str(&marker);
        written += 1;
    }
    for entry in kept_newest_first.iter().rev() {
        if written > 0 {
            log.push(',');
        }
        log.push_str(entry);
        written += 1;
    }
    debug_assert_eq!(written, element_count);
    log.push(']');
    (log.len() <= LOG_ENTRY_MAX_CHARS).then_some(log)
}

fn bounded_log_json_len(kept_newest_first: &[String], dropped: usize) -> usize {
    let marker_len = if dropped > 0 {
        serde_json::to_string(&json!({
            "kind": "logTruncated",
            "droppedEntries": dropped,
        }))
        .map(|marker| marker.len())
        .unwrap_or(usize::MAX)
    } else {
        0
    };
    let elements = kept_newest_first.len() + usize::from(dropped > 0);
    2_usize
        .saturating_add(marker_len)
        .saturating_add(kept_newest_first.iter().map(String::len).sum::<usize>())
        .saturating_add(elements.saturating_sub(1))
}

/// Streams every ordered output chunk into a synced sibling spool while
/// hashing incrementally. Memory use is bounded by one stored event payload,
/// regardless of the session's aggregate output size.
#[cfg(test)]
fn prepare_delivery_spool(
    conn: &Connection,
    session_id: &str,
    target: &str,
) -> Result<Option<DeliveryPlan>, String> {
    let spool_path = new_delivery_spool_path(target)?;
    stream_session_output_to_spool(conn, session_id, target, spool_path)
}

fn new_delivery_spool_path(target: &str) -> Result<PathBuf, String> {
    let target_path = Path::new(target);
    let parent = output_parent(target_path);
    std::fs::create_dir_all(&parent).map_err(|error| {
        format!(
            "output commit failed: cannot create {}: {error}",
            parent.display()
        )
    })?;
    let file_name = target_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| "output commit failed: output target has no file name".to_string())?;
    Ok(parent.join(format!(
        ".coven-delivery-{}-{file_name}",
        uuid::Uuid::new_v4()
    )))
}

fn stream_session_output_to_spool(
    conn: &Connection,
    session_id: &str,
    target: &str,
    spool_path: PathBuf,
) -> Result<Option<DeliveryPlan>, String> {
    let mut spool = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&spool_path)
        .map_err(|error| {
            format!(
                "output commit failed: cannot create {}: {error}",
                spool_path.display()
            )
        })?;
    let mut statement = conn
        .prepare(
            "SELECT kind, payload_json FROM events
             WHERE session_id = ?1
             ORDER BY rowid ASC",
        )
        .map_err(|error| format!("failed to prepare output stream: {error}"))?;
    let rows = statement
        .query_map(params![session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("failed to read output stream: {error}"))?;
    let mut byte_len = 0_u64;
    let result = (|| -> Result<(), String> {
        for row in rows {
            let (kind, payload_json) =
                row.map_err(|error| format!("failed to read output event: {error}"))?;
            if is_output_loss_marker(&kind) {
                return Err(format!(
                    "output incomplete: session {session_id} contains loss marker `{kind}`"
                ));
            }
            if kind != "output" {
                continue;
            }
            let payload: Value = serde_json::from_str(&payload_json)
                .map_err(|error| format!("output event payload is invalid: {error}"))?;
            if let Some(data) = payload.get("data").and_then(Value::as_str) {
                if data.is_empty() {
                    continue;
                }

                let bytes = data.as_bytes();
                spool.write_all(bytes).map_err(|error| {
                    format!(
                        "output commit failed: cannot write {}: {error}",
                        spool_path.display()
                    )
                })?;
                byte_len = byte_len.saturating_add(bytes.len() as u64);
            }
        }
        spool.sync_all().map_err(|error| {
            format!(
                "output commit failed: cannot sync {}: {error}",
                spool_path.display()
            )
        })
    })();
    if let Err(error) = result {
        drop(spool);
        let _ = std::fs::remove_file(&spool_path);
        return Err(error);
    }
    drop(spool);
    if byte_len == 0 {
        let _ = std::fs::remove_file(&spool_path);
        return Ok(None);
    }
    let mut digest = Sha256::new();
    digest.update((target.len() as u64).to_be_bytes());
    digest.update(target.as_bytes());
    digest.update(byte_len.to_be_bytes());
    let digest_result = (|| -> Result<String, String> {
        let mut spool_reader = std::fs::File::open(&spool_path).map_err(|error| {
            format!(
                "output commit failed: cannot reopen {} for hashing: {error}",
                spool_path.display()
            )
        })?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = spool_reader.read(&mut buffer).map_err(|error| {
                format!(
                    "output commit failed: cannot hash {}: {error}",
                    spool_path.display()
                )
            })?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        let hash = digest.finalize();
        let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(format!("sha256:{hex}"))
    })();
    let digest = match digest_result {
        Ok(digest) => digest,
        Err(error) => {
            let _ = std::fs::remove_file(&spool_path);
            return Err(error);
        }
    };
    Ok(Some(DeliveryPlan {
        target: target.to_string(),
        spool_path,
        digest,
        byte_len,
    }))
}

fn is_output_loss_marker(kind: &str) -> bool {
    matches!(
        kind,
        "output_truncated" | "gap" | "output_gap" | "event_gap" | "stream_gap"
    ) || kind.ends_with("_gap")
}

struct SpoolLedgerTarget<'a> {
    run_id: &'a str,
    occurrence_id: &'a str,
    target: &'a str,
}

static DELIVERY_PROCESS_ID: LazyLock<String> =
    LazyLock::new(|| format!("delivery-process-{}", uuid::Uuid::new_v4()));
static SETTLEMENT_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
struct SettlementPhaseHook {
    run_id: String,
    phase: &'static str,
    reached: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
static SETTLEMENT_PHASE_HOOK: Mutex<Option<SettlementPhaseHook>> = Mutex::new(None);

fn delivery_process_id() -> &'static str {
    DELIVERY_PROCESS_ID.as_str()
}

fn lock_settlement() -> Result<MutexGuard<'static, ()>, String> {
    SETTLEMENT_LOCK
        .lock()
        .map_err(|_| "automation settlement lock is poisoned".to_string())
}

#[cfg(test)]
fn install_settlement_phase_hook(hook: SettlementPhaseHook) {
    *SETTLEMENT_PHASE_HOOK.lock().unwrap() = Some(hook);
}

#[cfg(test)]
fn pause_settlement_phase_for_test(run_id: &str, phase: &'static str) {
    let hook = {
        let mut installed = SETTLEMENT_PHASE_HOOK.lock().unwrap();
        if installed
            .as_ref()
            .is_some_and(|hook| hook.run_id == run_id && hook.phase == phase)
        {
            installed.take()
        } else {
            None
        }
    };
    if let Some(hook) = hook {
        hook.reached.send(()).unwrap();
        hook.release.recv().unwrap();
    }
}

#[cfg(not(test))]
fn pause_settlement_phase_for_test(_run_id: &str, _phase: &'static str) {}

fn prepare_tracked_delivery_spool(
    conn: &Connection,
    ledger: SpoolLedgerTarget<'_>,
    session_id: &str,
) -> Result<Option<DeliveryPlan>, String> {
    let spool_path = new_delivery_spool_path(ledger.target)?;
    begin_spool_preparation(conn, &ledger, &spool_path)?;
    pause_settlement_phase_for_test(ledger.run_id, "preparing");
    let prepared =
        stream_session_output_to_spool(conn, session_id, ledger.target, spool_path.clone());
    match prepared {
        Ok(Some(plan)) => {
            if let Err(error) = mark_spool_synced(conn, &ledger, &plan) {
                let spool_path = plan.spool_path.clone();
                drop(plan);
                if remove_recorded_spool(&spool_path, ledger.target).is_ok() {
                    clear_spool_ledger(conn, &ledger, &spool_path)?;
                }
                return Err(error);
            }
            pause_settlement_phase_for_test(ledger.run_id, "spooled");
            Ok(Some(plan))
        }
        Ok(None) => {
            clear_spool_ledger(conn, &ledger, &spool_path)?;
            Ok(None)
        }
        Err(error) => {
            if remove_recorded_spool(&spool_path, ledger.target).is_ok() {
                clear_spool_ledger(conn, &ledger, &spool_path)?;
            }
            Err(error)
        }
    }
}

fn begin_spool_preparation(
    conn: &Connection,
    ledger: &SpoolLedgerTarget<'_>,
    spool_path: &Path,
) -> Result<(), String> {
    let spool_path = spool_path.to_string_lossy();
    let owner = delivery_process_id();
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin spool preparation: {error}"))?;
    let run_changed = transaction
        .execute(
            "UPDATE automation_runs
                 SET delivery_spool_path = ?2,
                     delivery_spool_state = 'preparing',
                     delivery_spool_owner = ?3,
                     delivery_spool_digest = NULL,
                     delivery_spool_bytes = NULL
                 WHERE id = ?1
                   AND status = 'running'
                   AND delivery_state = 'none'
                   AND delivery_spool_state = 'none'",
            params![ledger.run_id, spool_path, owner],
        )
        .map_err(|error| format!("failed to record run spool preparation: {error}"))?;
    let occurrence_changed = transaction
        .execute(
            "UPDATE automation_occurrences
                 SET delivery_spool_path = ?2,
                     delivery_spool_state = 'preparing',
                     delivery_spool_owner = ?3,
                     delivery_spool_digest = NULL,
                     delivery_spool_bytes = NULL
                 WHERE id = ?1
                   AND state IN ('claimed', 'running')
                   AND delivery_state = 'none'
                   AND delivery_spool_state = 'none'",
            params![ledger.occurrence_id, spool_path, owner],
        )
        .map_err(|error| format!("failed to record occurrence spool preparation: {error}"))?;
    if run_changed != 1 || occurrence_changed != 1 {
        return Err("spool preparation CAS rejected".to_string());
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit spool preparation: {error}"))
}

fn mark_spool_synced(
    conn: &Connection,
    ledger: &SpoolLedgerTarget<'_>,
    plan: &DeliveryPlan,
) -> Result<(), String> {
    let spool_path = plan.spool_path.to_string_lossy();
    let owner = delivery_process_id();
    let spool_bytes = i64::try_from(plan.byte_len)
        .map_err(|_| "delivery spool exceeds SQLite integer range".to_string())?;
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin synced spool update: {error}"))?;
    let run_changed = transaction
        .execute(
            "UPDATE automation_runs
                 SET delivery_spool_state = 'spooled',
                     delivery_spool_digest = ?3,
                     delivery_spool_bytes = ?4
                 WHERE id = ?1
                   AND delivery_spool_state = 'preparing'
                   AND delivery_spool_path = ?2
                   AND delivery_spool_owner = ?5",
            params![ledger.run_id, spool_path, plan.digest, spool_bytes, owner],
        )
        .map_err(|error| format!("failed to record synced run spool: {error}"))?;
    let occurrence_changed = transaction
        .execute(
            "UPDATE automation_occurrences
                 SET delivery_spool_state = 'spooled',
                     delivery_spool_digest = ?3,
                     delivery_spool_bytes = ?4
                 WHERE id = ?1
                   AND delivery_spool_state = 'preparing'
                   AND delivery_spool_path = ?2
                   AND delivery_spool_owner = ?5",
            params![
                ledger.occurrence_id,
                spool_path,
                plan.digest,
                spool_bytes,
                owner
            ],
        )
        .map_err(|error| format!("failed to record synced occurrence spool: {error}"))?;
    if run_changed != 1 || occurrence_changed != 1 {
        return Err("synced spool update CAS rejected".to_string());
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit synced spool update: {error}"))
}

fn clear_spool_ledger(
    conn: &Connection,
    ledger: &SpoolLedgerTarget<'_>,
    spool_path: &Path,
) -> Result<(), String> {
    let spool_path = spool_path.to_string_lossy();
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin spool cleanup: {error}"))?;
    let run_changed = transaction
        .execute(
            "UPDATE automation_runs
                 SET delivery_spool_path = NULL,
                     delivery_spool_state = 'none',
                     delivery_spool_owner = NULL,
                     delivery_spool_digest = NULL,
                     delivery_spool_bytes = NULL
                 WHERE id = ?1 AND delivery_spool_path = ?2",
            params![ledger.run_id, spool_path],
        )
        .map_err(|error| format!("failed to clear run spool ledger: {error}"))?;
    let occurrence_changed = transaction
        .execute(
            "UPDATE automation_occurrences
                 SET delivery_spool_path = NULL,
                     delivery_spool_state = 'none',
                     delivery_spool_owner = NULL,
                     delivery_spool_digest = NULL,
                     delivery_spool_bytes = NULL
                 WHERE id = ?1 AND delivery_spool_path = ?2",
            params![ledger.occurrence_id, spool_path],
        )
        .map_err(|error| format!("failed to clear occurrence spool ledger: {error}"))?;
    if run_changed != 1 || occurrence_changed != 1 {
        return Err("spool cleanup CAS rejected".to_string());
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit spool cleanup: {error}"))
}

fn remove_recorded_spool(spool_path: &Path, target: &str) -> Result<(), String> {
    if take_spool_remove_failure_for_test() {
        return Err("synthetic delivery spool removal failure".to_string());
    }
    let file_name = spool_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "recorded delivery spool has no UTF-8 file name".to_string())?;
    let suffix = file_name
        .strip_prefix(".coven-delivery-")
        .ok_or_else(|| "recorded delivery spool has an invalid name".to_string())?;
    let suffix_bytes = suffix.as_bytes();
    let uuid = suffix_bytes
        .get(..36)
        .and_then(|bytes| std::str::from_utf8(bytes).ok());
    if suffix_bytes.get(36) != Some(&b'-')
        || suffix_bytes.get(37..).is_none_or(|bytes| bytes.is_empty())
        || uuid.is_none_or(|uuid| uuid::Uuid::parse_str(uuid).is_err())
        || spool_path.parent() != Some(output_parent(Path::new(target)).as_path())
    {
        return Err("recorded delivery spool is outside its target directory".to_string());
    }
    let metadata = match std::fs::symlink_metadata(spool_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "failed to inspect recorded delivery spool {}: {error}",
                spool_path.display()
            ));
        }
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing to remove non-regular delivery spool {}",
            spool_path.display()
        ));
    }
    std::fs::remove_file(spool_path).map_err(|error| {
        format!(
            "failed to remove recorded delivery spool {}: {error}",
            spool_path.display()
        )
    })
}

struct OrphanedSpool {
    run_id: String,
    occurrence_id: String,
    target: String,
    spool_path: String,
    spool_state: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct SpoolRecoveryReport {
    pub removed: usize,
    pub degraded: Vec<String>,
}

fn reconcile_orphaned_delivery_spools_for_owner(
    conn: &Connection,
    current_owner: &str,
) -> Result<SpoolRecoveryReport, String> {
    let rows = {
        let mut statement = conn
            .prepare(
                "SELECT r.id, o.id, r.output_target,
                            r.delivery_spool_path, r.delivery_spool_state, r.delivery_spool_owner,
                            o.delivery_spool_path, o.delivery_spool_state, o.delivery_spool_owner
                     FROM automation_runs AS r
                     JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                     WHERE r.delivery_spool_state IN ('preparing', 'spooled')
                        OR o.delivery_spool_state IN ('preparing', 'spooled')
                     ORDER BY r.id",
            )
            .map_err(|error| format!("failed to prepare orphan spool reconciliation: {error}"))?;
        let mapped = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })
            .map_err(|error| format!("failed to read orphan spool reconciliation: {error}"))?;
        mapped
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| format!("failed to decode orphan spool reconciliation: {error}"))?
    };
    let mut report = SpoolRecoveryReport::default();
    for (
        run_id,
        occurrence_id,
        target,
        run_path,
        run_state,
        run_owner,
        occurrence_path,
        occurrence_state,
        occurrence_owner,
    ) in rows
    {
        let owner = run_owner
            .filter(|owner| !owner.is_empty())
            .unwrap_or_default();
        if owner == current_owner {
            continue;
        }
        let recovery = (|| -> Result<(), String> {
            if owner.is_empty()
                || occurrence_owner.as_deref() != Some(owner.as_str())
                || occurrence_path != run_path
                || occurrence_state != run_state
            {
                return Err(format!(
                    "run {run_id} and occurrence {occurrence_id} have inconsistent spool evidence"
                ));
            }
            let target =
                target.ok_or_else(|| format!("run {run_id} spool has no output target"))?;
            let spool_path = run_path
                .ok_or_else(|| format!("run {run_id} spool state `{run_state}` has no path"))?;
            let spool = OrphanedSpool {
                run_id: run_id.clone(),
                occurrence_id: occurrence_id.clone(),
                target,
                spool_path,
                spool_state: run_state,
            };
            if !matches!(spool.spool_state.as_str(), "preparing" | "spooled") {
                return Err(format!(
                    "run {} has invalid spool state `{}`",
                    spool.run_id, spool.spool_state
                ));
            }
            let spool_path = PathBuf::from(&spool.spool_path);
            remove_recorded_spool(&spool_path, &spool.target)?;
            clear_spool_ledger(
                conn,
                &SpoolLedgerTarget {
                    run_id: &spool.run_id,
                    occurrence_id: &spool.occurrence_id,
                    target: &spool.target,
                },
                &spool_path,
            )?;
            Ok(())
        })();
        match recovery {
            Ok(()) => report.removed += 1,
            Err(error) => {
                let diagnostic = format!("delivery spool recovery degraded: {error}");
                mark_spool_recovery_degraded(conn, &run_id, &occurrence_id, &diagnostic)?;
                report.degraded.push(diagnostic);
            }
        }
    }
    Ok(report)
}

fn mark_spool_recovery_degraded(
    conn: &Connection,
    run_id: &str,
    occurrence_id: &str,
    diagnostic: &str,
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin degraded spool recovery: {error}"))?;
    let run_changed = transaction
        .execute(
            "UPDATE automation_runs
             SET status = CASE WHEN status = 'running' THEN 'failed' ELSE status END,
                 delivery_state = CASE
                     WHEN delivery_state = 'committed' THEN 'committed'
                     ELSE 'ambiguous'
                 END,
                 delivery_error = CASE
                     WHEN delivery_error IS NULL OR trim(delivery_error) = '' THEN ?2
                     ELSE delivery_error || '; ' || ?2
                 END,
                 delivery_spool_state = 'degraded',
                 finished_at = CASE
                     WHEN status = 'running' THEN COALESCE(finished_at, ?3)
                     ELSE finished_at
                 END
             WHERE id = ?1",
            params![run_id, diagnostic, now],
        )
        .map_err(|error| format!("failed to degrade run spool recovery: {error}"))?;
    let occurrence_changed = transaction
        .execute(
            "UPDATE automation_occurrences
             SET state = CASE
                     WHEN state IN ('claimed', 'running') THEN 'failed'
                     ELSE state
                 END,
                 failure_reason = CASE
                     WHEN failure_reason IS NULL OR trim(failure_reason) = '' THEN ?2
                     ELSE failure_reason || '; ' || ?2
                 END,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 delivery_state = CASE
                     WHEN delivery_state = 'committed' THEN 'committed'
                     ELSE 'ambiguous'
                 END,
                 delivery_error = CASE
                     WHEN delivery_error IS NULL OR trim(delivery_error) = '' THEN ?2
                     ELSE delivery_error || '; ' || ?2
                 END,
                 delivery_spool_state = 'degraded',
                 updated_at = ?3
             WHERE id = ?1",
            params![occurrence_id, diagnostic, now],
        )
        .map_err(|error| format!("failed to degrade occurrence spool recovery: {error}"))?;
    if run_changed != 1 || occurrence_changed != 1 {
        return Err("degraded spool recovery CAS rejected".to_string());
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit degraded spool recovery: {error}"))
}

pub(crate) fn reconcile_orphaned_delivery_spools(
    conn: &Connection,
) -> Result<SpoolRecoveryReport, String> {
    let _guard = lock_settlement()?;
    reconcile_orphaned_delivery_spools_for_owner(conn, delivery_process_id())
}

/// Atomically commits `payload` to `target`: the bytes land in a temp file in
/// the target's directory and are renamed into place, so readers never see a
/// partial file. Every failure is reported as `output commit failed: …`.
#[cfg(test)]
pub fn deliver_output(target: &str, payload: &str) -> Result<(), String> {
    deliver_output_detailed(target, payload).map_err(|failure| failure.message)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryIoStage {
    BeforeRename,
    AfterRename,
}

struct DeliveryIoFailure {
    stage: DeliveryIoStage,
    message: String,
}

#[cfg(test)]
fn deliver_output_detailed(target: &str, payload: &str) -> Result<(), DeliveryIoFailure> {
    let target_path = Path::new(target);
    let parent = output_parent(target_path);
    std::fs::create_dir_all(&parent).map_err(|error| DeliveryIoFailure {
        stage: DeliveryIoStage::BeforeRename,
        message: format!(
            "output commit failed: cannot create {}: {error}",
            parent.display()
        ),
    })?;
    let file_name = target_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| DeliveryIoFailure {
            stage: DeliveryIoStage::BeforeRename,
            message: "output commit failed: output target has no file name".to_string(),
        })?;
    let temp = parent.join(format!(
        ".coven-delivery-{}-{file_name}",
        uuid::Uuid::new_v4()
    ));
    let result = write_atomically(&temp, target_path, payload);
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
fn write_atomically(temp: &Path, target: &Path, payload: &str) -> Result<(), DeliveryIoFailure> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(temp)
        .map_err(|error| DeliveryIoFailure {
            stage: DeliveryIoStage::BeforeRename,
            message: format!(
                "output commit failed: cannot create {}: {error}",
                temp.display()
            ),
        })?;
    file.write_all(payload.as_bytes())
        .map_err(|error| DeliveryIoFailure {
            stage: DeliveryIoStage::BeforeRename,
            message: format!(
                "output commit failed: cannot write {}: {error}",
                temp.display()
            ),
        })?;
    file.sync_all().map_err(|error| DeliveryIoFailure {
        stage: DeliveryIoStage::BeforeRename,
        message: format!(
            "output commit failed: cannot sync {}: {error}",
            temp.display()
        ),
    })?;
    drop(file);
    replace_output_file(temp, target).map_err(|error| DeliveryIoFailure {
        stage: DeliveryIoStage::BeforeRename,
        message: format!(
            "output commit failed: cannot rename {} → {}: {error}",
            temp.display(),
            target.display()
        ),
    })?;
    sync_parent_directory(target).map_err(|message| DeliveryIoFailure {
        stage: DeliveryIoStage::AfterRename,
        message,
    })
}

fn output_parent(target: &Path) -> PathBuf {
    target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

#[cfg(not(windows))]
fn replace_output_file(temp: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(temp, target)
}

#[cfg(windows)]
fn replace_output_file(temp: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    fn wide_path(path: &Path) -> std::io::Result<Vec<u16>> {
        let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if wide.contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "output path contains an interior NUL",
            ));
        }
        wide.push(0);
        Ok(wide)
    }

    let temp = wide_path(temp)?;
    let target = wide_path(target)?;
    // SAFETY: both paths are owned, NUL-terminated UTF-16 buffers that remain
    // alive for the call. WRITE_THROUGH is the Windows durability boundary.
    let moved = unsafe {
        MoveFileExW(
            temp.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
fn delivery_commit_strategy() -> &'static str {
    if cfg!(windows) {
        "windows-write-through-replace"
    } else if cfg!(unix) {
        "unix-file-and-directory-sync"
    } else {
        "unsupported-ambiguous"
    }
}

#[cfg(unix)]
fn sync_parent_directory(target: &Path) -> Result<(), String> {
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if take_parent_sync_failure_for_test() {
        return Err(format!(
            "output commit failed: cannot sync directory {}: synthetic parent sync failure",
            parent.display()
        ));
    }
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            format!(
                "output commit failed: cannot sync directory {}: {error}",
                parent.display()
            )
        })
}

#[cfg(windows)]
fn sync_parent_directory(_target: &Path) -> Result<(), String> {
    if take_parent_sync_failure_for_test() {
        return Err(
            "output commit failed: cannot sync directory: synthetic parent sync failure"
                .to_string(),
        );
    }
    // `replace_output_file` uses MOVEFILE_WRITE_THROUGH after syncing the
    // staged file. Windows does not expose a portable directory-handle flush;
    // the write-through replacement is the platform durability boundary.
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn sync_parent_directory(_target: &Path) -> Result<(), String> {
    Err("output commit failed: platform has no durable directory commit primitive".to_string())
}

#[cfg(test)]
thread_local! {
    static FAIL_PARENT_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAIL_SPOOL_REMOVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn set_parent_sync_failure_for_test(enabled: bool) {
    FAIL_PARENT_SYNC.set(enabled);
}

#[cfg(test)]
fn take_parent_sync_failure_for_test() -> bool {
    FAIL_PARENT_SYNC.replace(false)
}

#[cfg(not(test))]
fn take_parent_sync_failure_for_test() -> bool {
    false
}

#[cfg(test)]
fn set_spool_remove_failure_for_test(enabled: bool) {
    FAIL_SPOOL_REMOVE.set(enabled);
}

#[cfg(test)]
fn take_spool_remove_failure_for_test() -> bool {
    FAIL_SPOOL_REMOVE.replace(false)
}

#[cfg(not(test))]
fn take_spool_remove_failure_for_test() -> bool {
    false
}

struct Settlement {
    status: &'static str,
    exit_code: Option<i64>,
    log: Option<String>,
    output_commit: Option<String>,
    reason: Option<String>,
    delivery: Option<DeliveryPlan>,
    delivery_failure: bool,
    delivery_failure_ambiguous: bool,
    delivery_required: bool,
}

struct DeliveryPlan {
    target: String,
    spool_path: PathBuf,
    digest: String,
    byte_len: u64,
}

impl Drop for DeliveryPlan {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.spool_path);
    }
}

struct RunningLedgerRow {
    run_id: String,
    automation_id: String,
    session_id: Option<String>,
    occurrence_id: Option<String>,
    occurrence_state: Option<String>,
    occurrence_failure: Option<String>,
    output_target: Option<String>,
    deadline_at: Option<String>,
    delivery_state: String,
    delivery_token: Option<String>,
    delivery_digest: Option<String>,
    termination_state: String,
}

fn running_ledger_rows(conn: &Connection) -> Result<Vec<RunningLedgerRow>, String> {
    let mut statement = conn
        .prepare(
            "SELECT r.id, r.automation_id, r.session_id, r.occurrence_id,
                    o.state, o.failure_reason, r.output_target, r.deadline_at,
                    r.delivery_state, r.delivery_token, r.delivery_digest,
                    r.termination_state
             FROM automation_runs AS r
             LEFT JOIN automation_occurrences AS o ON o.id = r.occurrence_id
             WHERE r.status = 'running'",
        )
        .map_err(|error| format!("failed to list running runs: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(RunningLedgerRow {
                run_id: row.get(0)?,
                automation_id: row.get(1)?,
                session_id: row.get(2)?,
                occurrence_id: row.get(3)?,
                occurrence_state: row.get(4)?,
                occurrence_failure: row.get(5)?,
                output_target: row.get(6)?,
                deadline_at: row.get(7)?,
                delivery_state: row.get(8)?,
                delivery_token: row.get(9)?,
                delivery_digest: row.get(10)?,
                termination_state: row.get(11)?,
            })
        })
        .map_err(|error| format!("failed to list running runs: {error}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|error| format!("failed to read running run: {error}"))?);
    }
    Ok(out)
}

/// Builds the settlement for a run whose session exists, or `None` while the
/// session is still live (or has no sessions row yet — the occurrence lease
/// bounds how long that can block the routine).
enum SessionObservation {
    Pending,
    DeadlineUnresolved,
    Terminal(Settlement),
}

struct SessionSettlementTarget<'a> {
    run_id: &'a str,
    occurrence_id: Option<&'a str>,
    session_id: &'a str,
    output_target: Option<&'a str>,
    delivery_state: &'a str,
}

fn session_settlement(
    conn: &Connection,
    target: SessionSettlementTarget<'_>,
    deadline: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<SessionObservation, String> {
    let SessionSettlementTarget {
        run_id,
        occurrence_id,
        session_id,
        output_target,
        delivery_state,
    } = target;
    let session = crate::store::get_session(conn, session_id)
        .map_err(|error| format!("failed to read session {session_id}: {error:#}"))?;
    let Some(session) = session else {
        if now >= deadline {
            return Ok(SessionObservation::DeadlineUnresolved);
        }
        return Ok(SessionObservation::Pending);
    };
    if !is_terminal_session_status(&session.status) {
        if now >= deadline {
            return Ok(SessionObservation::DeadlineUnresolved);
        }
        return Ok(SessionObservation::Pending);
    }

    let log = capture_bounded_log(conn, session_id);
    let terminal_at = crate::store::get_session_terminal_at(conn, session_id)
        .map_err(|error| {
            format!("failed to read session {session_id} terminal timestamp: {error:#}")
        })?
        .ok_or_else(|| {
            format!("terminal session {session_id} has no immutable terminal timestamp")
        })?;
    let completed_at = chrono::DateTime::parse_from_rfc3339(&terminal_at)
        .map(|instant| instant.with_timezone(&Utc))
        .map_err(|error| format!("session {session_id} has invalid completion time: {error}"))?;
    if completed_at > deadline {
        return Ok(SessionObservation::Terminal(timeout_settlement(
            session.exit_code.map(i64::from),
            log,
            session_id,
        )));
    }
    let exit_zero = session.exit_code.unwrap_or(0) == 0;
    let succeeded = matches!(session.status.as_str(), "completed" | "idle") && exit_zero;
    if !succeeded {
        let reason = match session.exit_code {
            Some(code) => format!("session {} (exit code {code})", session.status),
            None => format!("session {}", session.status),
        };
        return Ok(SessionObservation::Terminal(Settlement {
            status: "failed",
            exit_code: session.exit_code.map(i64::from),
            log,
            output_commit: None,
            reason: Some(reason),
            delivery: None,
            delivery_failure: false,
            delivery_failure_ambiguous: false,
            delivery_required: false,
        }));
    }

    // The run succeeded at the runtime. Delivery is Coven's job: commit the
    // final assistant payload to the configured target, and a failed commit
    // fails the run visibly (never reported as success).
    let (
        status,
        reason,
        output_commit,
        delivery,
        delivery_failure,
        delivery_failure_ambiguous,
        delivery_required,
    ) = match output_target {
        None => ("succeeded", None, None, None, false, false, false),
        Some(_target) if delivery_state == "pending" => {
            ("succeeded", None, None, None, false, false, true)
        }
        Some(target) => {
            let occurrence_id = occurrence_id
                .ok_or_else(|| format!("run {run_id} has no occurrence for delivery"))?;
            match prepare_tracked_delivery_spool(
                conn,
                SpoolLedgerTarget {
                    run_id,
                    occurrence_id,
                    target,
                },
                session_id,
            ) {
                Err(error) => {
                    let ambiguous = error.starts_with("output incomplete:");
                    ("failed", Some(error), None, None, true, ambiguous, false)
                }
                Ok(None) => (
                    "failed",
                    Some(format!(
                    "output commit failed: no assistant output captured for session {session_id}"
                )),
                    None,
                    None,
                    true,
                    false,
                    false,
                ),
                Ok(Some(plan)) => ("succeeded", None, None, Some(plan), false, false, true),
            }
        }
    };
    Ok(SessionObservation::Terminal(Settlement {
        status,
        exit_code: session.exit_code.map(i64::from),
        log,
        output_commit,
        reason,
        delivery,
        delivery_failure,
        delivery_failure_ambiguous,
        delivery_required,
    }))
}

fn timeout_settlement(exit_code: Option<i64>, log: Option<String>, session_id: &str) -> Settlement {
    Settlement {
        status: "failed",
        exit_code,
        log,
        output_commit: None,
        reason: Some(format!(
            "deadline exceeded before session {session_id} completed"
        )),
        delivery: None,
        delivery_failure: false,
        delivery_failure_ambiguous: false,
        delivery_required: false,
    }
}

struct TerminationTarget<'a> {
    run_id: &'a str,
    occurrence_id: &'a str,
    session_id: Option<&'a str>,
    state: &'a str,
}

fn request_timeout_termination(
    conn: &Connection,
    runtime: &dyn crate::api::SessionRuntime,
    target: TerminationTarget<'_>,
    now: DateTime<Utc>,
) -> Result<Option<String>, String> {
    if target.state == "kill_requested" || target.state == "ambiguous" {
        return Ok(None);
    }
    if target.state != "none" {
        return Err(format!(
            "run {} has invalid termination state `{}`",
            target.run_id, target.state
        ));
    }
    let requested_at = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin timeout termination request: {error}"))?;
    let run_changed = transaction
        .execute(
            "UPDATE automation_runs
             SET termination_state = 'kill_requested',
                 termination_requested_at = ?2,
                 termination_error = NULL
             WHERE id = ?1
               AND status = 'running'
               AND termination_state = 'none'",
            params![target.run_id, requested_at],
        )
        .map_err(|error| format!("failed to reserve run termination: {error}"))?;
    let occurrence_changed = transaction
        .execute(
            "UPDATE automation_occurrences
             SET termination_state = 'kill_requested',
                 termination_requested_at = ?2,
                 termination_error = NULL
             WHERE id = ?1
               AND state IN ('claimed', 'running')
               AND termination_state = 'none'",
            params![target.occurrence_id, requested_at],
        )
        .map_err(|error| format!("failed to reserve occurrence termination: {error}"))?;
    if run_changed != 1 || occurrence_changed != 1 {
        return Err("timeout termination reservation CAS rejected".to_string());
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit timeout termination request: {error}"))?;

    let Some(session_id) = target.session_id else {
        let reason = "timeout termination is ambiguous because no session is attached".to_string();
        mark_timeout_ambiguous(conn, target.run_id, target.occurrence_id, &reason)?;
        return Ok(Some(reason));
    };
    match runtime.kill_session(session_id) {
        Ok(()) => Ok(None),
        Err(error) => {
            let reason = format!("timeout termination could not be proven: {error:#}");
            mark_timeout_ambiguous(conn, target.run_id, target.occurrence_id, &reason)?;
            Ok(Some(reason))
        }
    }
}

fn mark_timeout_ambiguous(
    conn: &Connection,
    run_id: &str,
    occurrence_id: &str,
    reason: &str,
) -> Result<(), String> {
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin ambiguous timeout update: {error}"))?;
    let run_changed = transaction
        .execute(
            "UPDATE automation_runs
             SET termination_state = 'ambiguous',
                 termination_error = ?2
             WHERE id = ?1
               AND status = 'running'
               AND termination_state = 'kill_requested'",
            params![run_id, reason],
        )
        .map_err(|error| format!("failed to mark run termination ambiguous: {error}"))?;
    let occurrence_changed = transaction
        .execute(
            "UPDATE automation_occurrences
             SET termination_state = 'ambiguous',
                 termination_error = ?2
             WHERE id = ?1
               AND state IN ('claimed', 'running')
               AND termination_state = 'kill_requested'",
            params![occurrence_id, reason],
        )
        .map_err(|error| format!("failed to mark occurrence termination ambiguous: {error}"))?;
    if run_changed != 1 || occurrence_changed != 1 {
        return Err("ambiguous timeout update CAS rejected".to_string());
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit ambiguous timeout update: {error}"))
}

struct DeliveryReservation {
    token: String,
    digest: String,
}

enum DeliveryTerminalState {
    Committed,
    Failed(String),
    Ambiguous(String),
}

struct SettlementTarget<'a> {
    run_id: &'a str,
    occurrence_id: Option<&'a str>,
    occurrence_state: Option<&'a str>,
    occurrence_failure: Option<&'a str>,
    session_id: Option<String>,
}

fn reserve_delivery(
    conn: &Connection,
    target: &SettlementTarget<'_>,
    plan: &DeliveryPlan,
) -> Result<DeliveryReservation, String> {
    let occurrence_id = target
        .occurrence_id
        .ok_or_else(|| format!("run {} has no occurrence for delivery", target.run_id))?;
    let reservation = DeliveryReservation {
        token: format!("delivery-{}", uuid::Uuid::new_v4()),
        digest: plan.digest.clone(),
    };
    let spool_path = plan.spool_path.to_string_lossy();
    let spool_owner = delivery_process_id();
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin delivery reservation: {error}"))?;
    let run_changed = transaction
        .execute(
            "UPDATE automation_runs
             SET delivery_state = 'pending',
                 delivery_token = ?2,
                 delivery_digest = ?3,
                 delivery_error = NULL
             WHERE id = ?1
               AND status = 'running'
               AND delivery_state = 'none'
               AND delivery_spool_state = 'spooled'
               AND delivery_spool_path = ?4
               AND delivery_spool_digest = ?3
               AND delivery_spool_owner = ?5",
            params![
                target.run_id,
                reservation.token,
                reservation.digest,
                spool_path,
                spool_owner
            ],
        )
        .map_err(|error| format!("failed to reserve run delivery: {error}"))?;
    if run_changed != 1 {
        return Err(format!(
            "delivery reservation CAS rejected for run {}",
            target.run_id
        ));
    }
    let occurrence_changed = transaction
        .execute(
            "UPDATE automation_occurrences
             SET delivery_state = 'pending',
                 delivery_token = ?2,
                 delivery_digest = ?3,
                 delivery_error = NULL
             WHERE id = ?1
               AND delivery_state = 'none'
               AND delivery_spool_state = 'spooled'
               AND delivery_spool_path = ?4
               AND delivery_spool_digest = ?3
               AND delivery_spool_owner = ?5
               AND (
                   state IN ('claimed', 'running')
                   OR (state = 'failed' AND failure_reason = 'lease expired')
               )",
            params![
                occurrence_id,
                reservation.token,
                reservation.digest,
                spool_path,
                spool_owner
            ],
        )
        .map_err(|error| format!("failed to reserve occurrence delivery: {error}"))?;
    if occurrence_changed != 1 {
        return Err(format!(
            "delivery reservation CAS rejected for occurrence {occurrence_id}"
        ));
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit delivery reservation: {error}"))?;
    Ok(reservation)
}

fn finalize_reserved_delivery(
    conn: &Connection,
    target: SettlementTarget<'_>,
    mut settlement: Settlement,
    reservation: &DeliveryReservation,
    terminal: DeliveryTerminalState,
    output_target: &str,
    now: DateTime<Utc>,
) -> Result<(&'static str, Option<String>), String> {
    let occurrence_id = target
        .occurrence_id
        .ok_or_else(|| format!("run {} has no occurrence for delivery", target.run_id))?;
    let (delivery_state, status, reason, output_commit) = match terminal {
        DeliveryTerminalState::Committed => (
            "committed",
            "succeeded",
            None,
            Some(output_target.to_string()),
        ),
        DeliveryTerminalState::Failed(reason) => ("failed", "failed", Some(reason), None),
        DeliveryTerminalState::Ambiguous(reason) => ("ambiguous", "failed", Some(reason), None),
    };
    settlement.status = status;
    settlement.reason = reason.clone();
    settlement.output_commit = output_commit;
    settlement.delivery = None;

    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin delivery finalization: {error}"))?;
    let run_changed = transaction
        .execute(
            "UPDATE automation_runs
             SET delivery_state = ?4,
                 delivery_error = ?5,
                 delivery_spool_path = NULL,
                 delivery_spool_state = 'none',
                 delivery_spool_owner = NULL,
                 delivery_spool_digest = NULL,
                 delivery_spool_bytes = NULL
             WHERE id = ?1
               AND status = 'running'
               AND delivery_state = 'pending'
               AND delivery_token = ?2
               AND delivery_digest = ?3",
            params![
                target.run_id,
                reservation.token,
                reservation.digest,
                delivery_state,
                reason
            ],
        )
        .map_err(|error| format!("failed to finalize run delivery reservation: {error}"))?;
    if run_changed != 1 {
        return Err(format!(
            "delivery finalization CAS rejected for run {}",
            target.run_id
        ));
    }
    let occurrence_changed = transaction
        .execute(
            "UPDATE automation_occurrences
             SET state = ?4,
                 failure_reason = ?5,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 delivery_state = ?6,
                 delivery_error = ?5,
                 delivery_spool_path = NULL,
                 delivery_spool_state = 'none',
                 delivery_spool_owner = NULL,
                 delivery_spool_digest = NULL,
                 delivery_spool_bytes = NULL,
                 updated_at = ?7
             WHERE id = ?1
               AND delivery_state = 'pending'
               AND delivery_token = ?2
               AND delivery_digest = ?3
               AND (
                   state IN ('claimed', 'running')
                   OR (state = 'failed' AND failure_reason = 'lease expired')
               )",
            params![
                occurrence_id,
                reservation.token,
                reservation.digest,
                status,
                reason,
                delivery_state,
                now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            ],
        )
        .map_err(|error| format!("failed to finalize occurrence delivery reservation: {error}"))?;
    if occurrence_changed != 1 {
        return Err(format!(
            "delivery finalization CAS rejected for occurrence {occurrence_id}"
        ));
    }
    let finished = record_run_finish(
        &transaction,
        target.run_id,
        RunFinish {
            status,
            exit_code: settlement.exit_code,
            session_id: target.session_id,
            log_json: settlement.log,
            output_commit: settlement.output_commit,
        },
        now,
    )
    .map_err(|error| format!("failed to finalize delivered run: {error:#}"))?;
    if !finished {
        return Err(format!(
            "run {} changed during delivery finalization",
            target.run_id
        ));
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit delivery finalization: {error}"))?;
    Ok((status, reason))
}

fn settle_reserved_delivery(
    conn: &Connection,
    target: SettlementTarget<'_>,
    settlement: Settlement,
    plan: DeliveryPlan,
    now: DateTime<Utc>,
) -> Result<(&'static str, Option<String>), String> {
    if plan.byte_len == 0 {
        return Err("prepared delivery spool is empty".to_string());
    }
    let reservation = match reserve_delivery(conn, &target, &plan) {
        Ok(reservation) => reservation,
        Err(error) => {
            let occurrence_id = target
                .occurrence_id
                .ok_or_else(|| format!("run {} has no occurrence for delivery", target.run_id))?;
            let spool_path = plan.spool_path.clone();
            let output_target = plan.target.clone();
            drop(plan);
            if remove_recorded_spool(&spool_path, &output_target).is_ok() {
                clear_spool_ledger(
                    conn,
                    &SpoolLedgerTarget {
                        run_id: target.run_id,
                        occurrence_id,
                        target: &output_target,
                    },
                    &spool_path,
                )?;
            }
            return Err(error);
        }
    };
    pause_settlement_phase_for_test(target.run_id, "pending");
    let terminal = match commit_delivery_spool(&plan) {
        Ok(()) => DeliveryTerminalState::Committed,
        Err(failure) if failure.stage == DeliveryIoStage::BeforeRename => {
            DeliveryTerminalState::Failed(failure.message)
        }
        Err(failure) => DeliveryTerminalState::Ambiguous(failure.message),
    };
    finalize_reserved_delivery(
        conn,
        target,
        settlement,
        &reservation,
        terminal,
        &plan.target,
        now,
    )
}

fn commit_delivery_spool(plan: &DeliveryPlan) -> Result<(), DeliveryIoFailure> {
    let target = Path::new(&plan.target);
    replace_output_file(&plan.spool_path, target).map_err(|error| DeliveryIoFailure {
        stage: DeliveryIoStage::BeforeRename,
        message: format!(
            "output commit failed: cannot rename {} → {}: {error}",
            plan.spool_path.display(),
            target.display()
        ),
    })?;
    sync_parent_directory(target).map_err(|message| DeliveryIoFailure {
        stage: DeliveryIoStage::AfterRename,
        message,
    })
}

fn settle_interrupted_delivery(
    conn: &Connection,
    target: SettlementTarget<'_>,
    mut settlement: Settlement,
    token: String,
    digest: String,
    output_target: &str,
    now: DateTime<Utc>,
) -> Result<(&'static str, Option<String>), String> {
    settlement.delivery = None;
    let reason = "delivery outcome ambiguous after an interrupted pending reservation".to_string();
    finalize_reserved_delivery(
        conn,
        target,
        settlement,
        &DeliveryReservation { token, digest },
        DeliveryTerminalState::Ambiguous(reason),
        output_target,
        now,
    )
}

fn settle_delivery_preparation_failure(
    conn: &Connection,
    target: SettlementTarget<'_>,
    settlement: Settlement,
    now: DateTime<Utc>,
) -> Result<(&'static str, Option<String>), String> {
    let occurrence_id = target
        .occurrence_id
        .ok_or_else(|| format!("run {} has no occurrence for delivery", target.run_id))?;
    let reason = settlement
        .reason
        .clone()
        .ok_or_else(|| "delivery preparation failed without a reason".to_string())?;
    let delivery_state = if settlement.delivery_failure_ambiguous {
        "ambiguous"
    } else {
        "failed"
    };
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin delivery failure settlement: {error}"))?;
    let run_changed = transaction
        .execute(
            "UPDATE automation_runs
             SET delivery_state = ?3,
                 delivery_error = ?2
             WHERE id = ?1
               AND status = 'running'
               AND delivery_state = 'none'",
            params![target.run_id, reason, delivery_state],
        )
        .map_err(|error| format!("failed to record run delivery failure: {error}"))?;
    let occurrence_changed = transaction
        .execute(
            "UPDATE automation_occurrences
             SET state = 'failed',
                 failure_reason = ?2,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 delivery_state = ?4,
                 delivery_error = ?2,
                 updated_at = ?3
             WHERE id = ?1
               AND delivery_state = 'none'
               AND (
                   state IN ('claimed', 'running')
                   OR (state = 'failed' AND failure_reason = 'lease expired')
               )",
            params![
                occurrence_id,
                reason,
                now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                delivery_state
            ],
        )
        .map_err(|error| format!("failed to record occurrence delivery failure: {error}"))?;
    if run_changed != 1 || occurrence_changed != 1 {
        return Err("delivery failure settlement CAS rejected".to_string());
    }
    let finished = record_run_finish(
        &transaction,
        target.run_id,
        RunFinish {
            status: "failed",
            exit_code: settlement.exit_code,
            session_id: target.session_id,
            log_json: settlement.log,
            output_commit: None,
        },
        now,
    )
    .map_err(|error| {
        format!("failed to finish run after delivery preparation failure: {error:#}")
    })?;
    if !finished {
        return Err(format!(
            "run {} changed during delivery failure settlement",
            target.run_id
        ));
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit delivery failure settlement: {error}"))?;
    Ok(("failed", Some(reason)))
}

fn settle_linked_state(
    conn: &Connection,
    target: SettlementTarget<'_>,
    settlement: Settlement,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin run settlement: {error}"))?;

    if let Some(occurrence_id) = target.occurrence_id {
        let recoverable_lease_failure = target.occurrence_state == Some("failed")
            && target.occurrence_failure == Some("lease expired");
        let unsettled = matches!(target.occurrence_state, Some("claimed" | "running"));
        if !unsettled && !recoverable_lease_failure {
            return Err(format!(
                "contradictory terminal occurrence state `{}` for running run {}",
                target.occurrence_state.unwrap_or("missing"),
                target.run_id
            ));
        }
        let changed = transaction
            .execute(
                "UPDATE automation_occurrences
                 SET state = ?2,
                     failure_reason = ?3,
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     updated_at = ?4
                 WHERE id = ?1
                   AND (
                       state IN ('claimed', 'running')
                       OR (state = 'failed' AND failure_reason = 'lease expired')
                   )",
                params![
                    occurrence_id,
                    settlement.status,
                    settlement.reason,
                    now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                ],
            )
            .map_err(|error| format!("failed to settle occurrence with run: {error}"))?;
        if changed != 1 {
            return Err(format!(
                "occurrence {occurrence_id} changed during run settlement"
            ));
        }
    }
    let finished = record_run_finish(
        &transaction,
        target.run_id,
        RunFinish {
            status: settlement.status,
            exit_code: settlement.exit_code,
            session_id: target.session_id,
            log_json: settlement.log,
            output_commit: settlement.output_commit,
        },
        now,
    )
    .map_err(|error| format!("failed to settle run: {error:#}"))?;
    if !finished {
        return Err(format!("run {} changed during settlement", target.run_id));
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit run settlement: {error}"))
}

/// Settles every running ledger row whose work has finished. A row settles
/// when its session reached a terminal state, or when its occurrence already
/// settled through lease recovery. The matching occurrence settles alongside
/// the ledger row; nothing here ever reports a run success that did not
/// actually happen.
#[cfg(test)]
pub fn settle_finished_runs(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<ReconcileReport, String> {
    settle_finished_runs_with_runtime(conn, &crate::api::NoopSessionRuntime, now)
}

pub fn settle_finished_runs_with_runtime(
    conn: &Connection,
    runtime: &dyn crate::api::SessionRuntime,
    now: DateTime<Utc>,
) -> Result<ReconcileReport, String> {
    let _guard = lock_settlement()?;
    let mut report = ReconcileReport::default();
    let spool_recovery = reconcile_orphaned_delivery_spools_for_owner(conn, delivery_process_id())?;
    report.failures.extend(spool_recovery.degraded);

    for row in running_ledger_rows(conn)? {
        let RunningLedgerRow {
            run_id,
            automation_id,
            session_id,
            occurrence_id,
            occurrence_state,
            occurrence_failure,
            output_target,
            deadline_at,
            delivery_state,
            delivery_token,
            delivery_digest,
            termination_state,
        } = row;

        let deadline_at =
            deadline_at.ok_or_else(|| format!("running run {run_id} has no pinned deadline"))?;
        let deadline = chrono::DateTime::parse_from_rfc3339(&deadline_at)
            .map(|instant| instant.with_timezone(&Utc))
            .map_err(|error| format!("running run {run_id} has invalid deadline: {error}"))?;
        let observation = match session_id.as_deref() {
            Some(session_id) => session_settlement(
                conn,
                SessionSettlementTarget {
                    run_id: &run_id,
                    occurrence_id: occurrence_id.as_deref(),
                    session_id,
                    output_target: output_target.as_deref(),
                    delivery_state: &delivery_state,
                },
                deadline,
                now,
            )?,
            None if now >= deadline => SessionObservation::DeadlineUnresolved,
            None => SessionObservation::Pending,
        };
        let mut settlement = match observation {
            SessionObservation::Terminal(settlement) => Some(settlement),
            SessionObservation::Pending => None,
            SessionObservation::DeadlineUnresolved
                if occurrence_state.as_deref() == Some("failed") =>
            {
                Some(Settlement {
                    status: "failed",
                    exit_code: None,
                    log: None,
                    output_commit: None,
                    reason: Some(
                        occurrence_failure
                            .clone()
                            .filter(|reason| !reason.trim().is_empty())
                            .unwrap_or_else(|| "lease expired".to_string()),
                    ),
                    delivery: None,
                    delivery_failure: false,
                    delivery_failure_ambiguous: false,
                    delivery_required: false,
                })
            }
            SessionObservation::DeadlineUnresolved
                if occurrence_state.as_deref() == Some("succeeded") =>
            {
                Some(Settlement {
                    status: "failed",
                    exit_code: None,
                    log: None,
                    output_commit: None,
                    reason: Some("occurrence settled without a run result".to_string()),
                    delivery: None,
                    delivery_failure: false,
                    delivery_failure_ambiguous: false,
                    delivery_required: false,
                })
            }
            SessionObservation::DeadlineUnresolved => {
                let occurrence_id = occurrence_id
                    .as_deref()
                    .ok_or_else(|| format!("timed-out run {run_id} has no occurrence fence"))?;
                if let Some(failure) = request_timeout_termination(
                    conn,
                    runtime,
                    TerminationTarget {
                        run_id: &run_id,
                        occurrence_id,
                        session_id: session_id.as_deref(),
                        state: &termination_state,
                    },
                    now,
                )? {
                    report.failures.push(format!("{automation_id}: {failure}"));
                }
                report.still_running += 1;
                continue;
            }
        };
        if settlement.is_none() {
            if occurrence_state.as_deref() == Some("failed") {
                let reason = occurrence_failure
                    .clone()
                    .filter(|reason| !reason.trim().is_empty())
                    .unwrap_or_else(|| "lease expired".to_string());
                settlement = Some(Settlement {
                    status: "failed",
                    exit_code: None,
                    log: None,
                    output_commit: None,
                    reason: Some(reason),
                    delivery: None,
                    delivery_failure: false,
                    delivery_failure_ambiguous: false,
                    delivery_required: false,
                });
            } else if occurrence_state.as_deref() == Some("succeeded") {
                // Defensive: an occurrence cannot legitimately settle success
                // before its run does. Record a visible failure rather than
                // inventing a result.
                settlement = Some(Settlement {
                    status: "failed",
                    exit_code: None,
                    log: None,
                    output_commit: None,
                    reason: Some("occurrence settled without a run result".to_string()),
                    delivery: None,
                    delivery_failure: false,
                    delivery_failure_ambiguous: false,
                    delivery_required: false,
                });
            }
        }

        let Some(mut settlement) = settlement else {
            report.still_running += 1;
            continue;
        };

        let target = SettlementTarget {
            run_id: &run_id,
            occurrence_id: occurrence_id.as_deref(),
            occurrence_state: occurrence_state.as_deref(),
            occurrence_failure: occurrence_failure.as_deref(),
            session_id: session_id.clone(),
        };
        let (status, reason) = if settlement.delivery_failure {
            settle_delivery_preparation_failure(conn, target, settlement, now)?
        } else if settlement.delivery_required && delivery_state == "pending" {
            let delivery_reservation_id = delivery_token
                .ok_or_else(|| format!("pending delivery for run {run_id} has no token"))?;
            let digest = delivery_digest
                .ok_or_else(|| format!("pending delivery for run {run_id} has no digest"))?;
            let output_target = output_target
                .as_deref()
                .ok_or_else(|| format!("pending delivery for run {run_id} has no target"))?;
            settle_interrupted_delivery(
                conn,
                target,
                settlement,
                delivery_reservation_id,
                digest,
                output_target,
                now,
            )?
        } else {
            match settlement.delivery.take() {
                Some(plan) if delivery_state == "none" => {
                    settle_reserved_delivery(conn, target, settlement, plan, now)?
                }
                Some(_) => {
                    return Err(format!(
                        "running run {run_id} has invalid delivery state `{delivery_state}`"
                    ));
                }
                None if delivery_state == "none" => {
                    let status = settlement.status;
                    let reason = settlement.reason.clone();
                    settle_linked_state(conn, target, settlement, now)?;
                    (status, reason)
                }
                None => {
                    return Err(format!(
                    "run {run_id} has delivery state `{delivery_state}` without a delivery plan"
                ));
                }
            }
        };
        if status == "succeeded" {
            report.settled_succeeded += 1;
        } else {
            report.settled_failed += 1;
            if let Some(reason) = reason {
                report.failures.push(format!("{automation_id}: {reason}"));
            }
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::definition::RoutineDefinition;
    use crate::automations::store::insert_definition;
    use crate::store::{initialize_store, insert_event, insert_session, SessionRecord};
    use serde_json::json;
    use std::cell::Cell;

    #[derive(Default)]
    struct FailingKillRuntime {
        kills: Cell<usize>,
    }

    impl crate::api::SessionRuntime for FailingKillRuntime {
        fn launch_session(&self, _launch: &crate::api::SessionLaunch) -> anyhow::Result<()> {
            Ok(())
        }

        fn send_input(
            &self,
            _session_id: &str,
            _payload: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
            self.kills.set(self.kills.get() + 1);
            anyhow::bail!("synthetic unproven kill")
        }
    }

    fn temp_store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        (temp, conn)
    }

    fn definition(id: &str, output_target: Option<&str>) -> RoutineDefinition {
        let mut value = json!({
            "schemaVersion": 1,
            "id": id,
            "name": id,
            "status": "ACTIVE",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "cwd": "/work/project",
            "prompt": "Do the thing."
        });
        if let Some(output_target) = output_target {
            let object = value.as_object_mut().unwrap();
            object.insert("outputTarget".to_string(), json!(output_target));
        }
        RoutineDefinition::from_json(&value).unwrap()
    }

    fn session_record(conn: &Connection, id: &str, status: &str, exit_code: Option<i32>) {
        insert_session(
            conn,
            &SessionRecord {
                id: id.to_string(),
                project_root: "/work/project".to_string(),
                harness: "coven-code".to_string(),
                title: "routine run".to_string(),
                status: status.to_string(),
                exit_code,
                archived_at: None,
                created_at: "2026-08-28T09:00:00Z".to_string(),
                updated_at: "2026-08-28T09:05:00Z".to_string(),
                conversation_id: None,
                familiar_id: None,
                execution_binding: None,
                labels: Vec::new(),
                visibility: "private".to_string(),
                external: false,
                transcript_path: None,
            },
        )
        .unwrap();
    }

    fn event(conn: &Connection, session_id: &str, kind: &str, data: &str) {
        static EVENT_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let event_number = EVENT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        insert_event(
            conn,
            &crate::store::EventRecord {
                seq: 0,
                id: format!("event-{event_number}"),
                session_id: session_id.to_string(),
                kind: kind.to_string(),
                payload_json: json!({ "data": data }).to_string(),
                created_at: "2026-08-28T09:01:00Z".to_string(),
            },
        )
        .unwrap();
    }

    /// Seeds a live run: claimed occurrence + running ledger row, as dispatch
    /// leaves them before reconciliation.
    fn live_run(conn: &Connection, automation_id: &str, session_id: &str) -> String {
        let now = "2026-08-28T09:00:00.000Z";
        let record = super::super::store::get_definition(conn, automation_id)
            .unwrap()
            .unwrap();
        let snapshot = super::super::store::definition_snapshot(&record).unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at,
                 attempt, definition_revision, definition_digest, definition_json,
                 output_target, deadline_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'running', 'daemon', '2026-08-28T10:00:00.000Z', 1,
                     ?4, ?5, ?6, ?7, '2026-08-28T10:00:00.000Z', ?3, ?3)",
            rusqlite::params![
                format!("occ-{session_id}"),
                automation_id,
                now,
                snapshot.revision,
                snapshot.digest,
                snapshot.definition_json,
                snapshot.output_target
            ],
        )
        .unwrap();
        let run_id = format!("run-{session_id}");
        record_run_start_raw(conn, &run_id, automation_id, session_id, now);
        run_id
    }

    fn record_run_start_raw(
        conn: &Connection,
        run_id: &str,
        automation_id: &str,
        session_id: &str,
        now: &str,
    ) {
        let record = super::super::store::get_definition(conn, automation_id)
            .unwrap()
            .unwrap();
        let snapshot = super::super::store::definition_snapshot(&record).unwrap();
        conn.execute(
            "INSERT INTO automation_runs
                (id, automation_id, occurrence_id, session_id, familiar_id, runtime,
                 status, definition_revision, definition_digest, definition_json,
                 output_target, deadline_at, started_at)
             VALUES (?1, ?2, ?3, ?4, 'charm', 'coven-code', 'running',
                     ?5, ?6, ?7, ?8, '2026-08-28T10:00:00.000Z', ?9)",
            rusqlite::params![
                run_id,
                automation_id,
                format!("occ-{session_id}"),
                session_id,
                snapshot.revision,
                snapshot.digest,
                snapshot.definition_json,
                snapshot.output_target,
                now
            ],
        )
        .unwrap();
    }

    #[test]
    fn deliver_output_replaces_atomically_without_temp_litter() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("out").join("payload.md");
        deliver_output(target.to_str().unwrap(), "first").unwrap();
        deliver_output(target.to_str().unwrap(), "second version").unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "second version");
        let entries: Vec<_> = std::fs::read_dir(temp.path().join("out"))
            .unwrap()
            .collect();
        assert_eq!(entries.len(), 1, "no temp files may remain");
    }

    #[test]
    fn deliver_output_failure_is_visible() {
        let temp = tempfile::tempdir().unwrap();
        let blocker = temp.path().join("blocker");
        std::fs::write(&blocker, "a file, not a directory").unwrap();
        let target = blocker.join("payload.md");

        let error = deliver_output(target.to_str().unwrap(), "payload").unwrap_err();
        assert!(error.contains("output commit failed"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn bare_relative_target_syncs_the_current_directory_without_creating_artifacts() {
        sync_parent_directory(Path::new("bare-output.md")).unwrap();
        assert!(!Path::new("bare-output.md").exists());
    }

    #[test]
    fn bounded_log_keeps_the_tail_within_the_budget() {
        let (_temp, conn) = temp_store();
        session_record(&conn, "session-1", "completed", Some(0));
        let huge = "x".repeat(50 * 1024);
        event(&conn, "session-1", "output", &huge);
        event(&conn, "session-1", "output", &huge);
        event(&conn, "session-1", "output", "final answer");

        let log = capture_bounded_log(&conn, "session-1").unwrap();
        assert!(
            log.chars().count() <= LOG_ENTRY_MAX_CHARS,
            "log is bounded: {}",
            log.chars().count()
        );
        assert!(log.contains("logTruncated"), "{log}");
        assert!(log.contains("final answer"), "tail is kept: {log}");
    }

    #[test]
    fn bounded_log_reports_events_dropped_before_the_query_window() {
        let (_temp, conn) = temp_store();
        session_record(&conn, "session-window", "completed", Some(0));
        for index in 0..205 {
            event(&conn, "session-window", "output", &format!("event-{index}"));
        }

        let log = capture_bounded_log(&conn, "session-window").unwrap();
        let parsed: Value = serde_json::from_str(&log).unwrap();

        assert_eq!(parsed[0]["kind"], "logTruncated");
        assert_eq!(parsed[0]["droppedEntries"], 5);
        assert!(log.len() <= LOG_ENTRY_MAX_CHARS);
    }

    #[test]
    fn bounded_log_huge_event_is_valid_json_with_exact_truncation_marker() {
        let (_temp, conn) = temp_store();
        session_record(&conn, "session-huge", "completed", Some(0));
        event(
            &conn,
            "session-huge",
            "output",
            &"x".repeat(2 * 1024 * 1024),
        );

        let log = capture_bounded_log(&conn, "session-huge").unwrap();
        let parsed: Value = serde_json::from_str(&log).unwrap();

        assert!(log.len() <= LOG_ENTRY_MAX_CHARS);
        assert_eq!(parsed.as_array().unwrap().len(), 1);
        assert_eq!(parsed[0]["kind"], "logTruncated");
        assert_eq!(parsed[0]["droppedEntries"], 1);
    }

    #[test]
    fn bounded_log_near_limit_remains_valid_json_without_raw_truncation() {
        let (_temp, conn) = temp_store();
        session_record(&conn, "session-near-limit", "completed", Some(0));
        event(
            &conn,
            "session-near-limit",
            "output",
            &"x".repeat(63 * 1024),
        );

        let log = capture_bounded_log(&conn, "session-near-limit").unwrap();
        let parsed: Value = serde_json::from_str(&log).unwrap();

        assert!(log.len() <= LOG_ENTRY_MAX_CHARS);
        assert_eq!(parsed.as_array().unwrap().len(), 1);
        assert_eq!(
            parsed[0]["payload"]["data"].as_str().unwrap().len(),
            63 * 1024
        );
    }

    #[test]
    fn spooled_output_reconstructs_every_ordered_output_chunk() {
        let target_dir = tempfile::tempdir().unwrap();
        let target = target_dir.path().join("output.txt");
        let (_temp, conn) = temp_store();
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "first ");
        event(&conn, "session-1", "input", "keep going");
        event(&conn, "session-1", "output", "second ");
        for _ in 0..205 {
            event(&conn, "session-1", "output", "x");
        }
        event(&conn, "session-1", "output", " final");

        let expected = format!("first second {} final", "x".repeat(205));
        let prepared = prepare_delivery_spool(&conn, "session-1", target.to_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&prepared.spool_path).unwrap(),
            expected
        );
        assert!(
            prepare_delivery_spool(&conn, "session-missing", target.to_str().unwrap())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn spooled_output_handles_many_large_chunks_without_aggregate_buffer() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("large-output.bin");
        let (_temp, conn) = temp_store();
        session_record(&conn, "large-session", "completed", Some(0));
        let chunk = "x".repeat(8 * 1024);
        for _ in 0..512 {
            event(&conn, "large-session", "output", &chunk);
        }

        let prepared = prepare_delivery_spool(&conn, "large-session", target.to_str().unwrap())
            .unwrap()
            .expect("large output spool");

        assert_eq!(prepared.byte_len, 512 * 8 * 1024);
        assert_eq!(
            std::fs::metadata(&prepared.spool_path).unwrap().len(),
            prepared.byte_len
        );
        assert!(prepared.digest.starts_with("sha256:"));
        assert!(!target.exists());
    }

    #[test]
    fn delivery_digest_is_invariant_to_event_chunk_boundaries_and_binds_target() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("same-target.txt");
        let other_target = temp.path().join("other-target.txt");
        let (_temp, conn) = temp_store();
        session_record(&conn, "chunked-a", "completed", Some(0));
        session_record(&conn, "chunked-b", "completed", Some(0));
        event(&conn, "chunked-a", "output", "a");
        event(&conn, "chunked-a", "output", "bc");
        event(&conn, "chunked-b", "output", "ab");
        event(&conn, "chunked-b", "output", "c");

        let first = prepare_delivery_spool(&conn, "chunked-a", target.to_str().unwrap())
            .unwrap()
            .unwrap();
        let second = prepare_delivery_spool(&conn, "chunked-b", target.to_str().unwrap())
            .unwrap()
            .unwrap();
        let rebound = prepare_delivery_spool(&conn, "chunked-b", other_target.to_str().unwrap())
            .unwrap()
            .unwrap();

        assert_eq!(first.digest, second.digest);
        assert_ne!(first.digest, rebound.digest);
        assert_eq!(std::fs::read(&first.spool_path).unwrap(), b"abc");
        assert_eq!(std::fs::read(&second.spool_path).unwrap(), b"abc");
    }

    #[test]
    fn restart_removes_preparing_spool_but_not_unrelated_files() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let unrelated = temp.path().join(".coven-delivery-unrelated");
        std::fs::write(&unrelated, "keep").unwrap();
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        let spool_path = new_delivery_spool_path(target.to_str().unwrap()).unwrap();
        let ledger = SpoolLedgerTarget {
            run_id: "run-session-1",
            occurrence_id: "occ-session-1",
            target: target.to_str().unwrap(),
        };
        begin_spool_preparation(&conn, &ledger, &spool_path).unwrap();
        let mut spool = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&spool_path)
            .unwrap();
        spool.write_all(b"crash point").unwrap();
        spool.sync_all().unwrap();
        drop(spool);

        reconcile_orphaned_delivery_spools_for_owner(&conn, "restart-owner").unwrap();

        assert!(!spool_path.exists());
        assert_eq!(std::fs::read_to_string(&unrelated).unwrap(), "keep");
        let states: (String, String) = conn
            .query_row(
                "SELECT o.delivery_spool_state, r.delivery_spool_state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(states, ("none".to_string(), "none".to_string()));
    }

    #[test]
    fn restart_removes_synced_spool_before_reservation() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let unrelated = temp.path().join("ordinary-file");
        std::fs::write(&unrelated, "keep").unwrap();
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "spooled output");
        let plan = prepare_tracked_delivery_spool(
            &conn,
            SpoolLedgerTarget {
                run_id: "run-session-1",
                occurrence_id: "occ-session-1",
                target: target.to_str().unwrap(),
            },
            "session-1",
        )
        .unwrap()
        .unwrap();
        let spool_path = plan.spool_path.clone();
        std::mem::forget(plan);

        reconcile_orphaned_delivery_spools_for_owner(&conn, "restart-owner").unwrap();

        assert!(!spool_path.exists());
        assert_eq!(std::fs::read_to_string(&unrelated).unwrap(), "keep");
        assert!(!target.exists());
        let states: (String, String) = conn
            .query_row(
                "SELECT o.delivery_spool_state, r.delivery_spool_state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(states, ("none".to_string(), "none".to_string()));
    }

    #[test]
    fn restart_removes_reserved_spool_before_rename_and_marks_delivery_ambiguous() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let unrelated = temp.path().join(".coven-delivery-untracked");
        std::fs::write(&unrelated, "keep").unwrap();
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "reserved output");
        let plan = prepare_tracked_delivery_spool(
            &conn,
            SpoolLedgerTarget {
                run_id: "run-session-1",
                occurrence_id: "occ-session-1",
                target: target.to_str().unwrap(),
            },
            "session-1",
        )
        .unwrap()
        .unwrap();
        reserve_delivery(
            &conn,
            &SettlementTarget {
                run_id: "run-session-1",
                occurrence_id: Some("occ-session-1"),
                occurrence_state: Some("running"),
                occurrence_failure: None,
                session_id: Some("session-1".to_string()),
            },
            &plan,
        )
        .unwrap();
        let spool_path = plan.spool_path.clone();
        std::mem::forget(plan);

        reconcile_orphaned_delivery_spools_for_owner(&conn, "restart-owner").unwrap();
        let report = settle_finished_runs(&conn, Utc::now()).unwrap();

        assert!(!spool_path.exists());
        assert!(!target.exists());
        assert_eq!(std::fs::read_to_string(&unrelated).unwrap(), "keep");
        assert_eq!(report.settled_failed, 1);
        let states: (String, String, String, String) = conn
            .query_row(
                "SELECT o.state, o.delivery_state, r.status, r.delivery_state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            states,
            (
                "failed".to_string(),
                "ambiguous".to_string(),
                "failed".to_string(),
                "ambiguous".to_string()
            )
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_symlink_spool_is_left_untouched_and_only_affected_run_degrades() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let unrelated = temp.path().join("unrelated.txt");
        std::fs::write(&unrelated, "keep").unwrap();
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        insert_definition(&conn, &definition("healthy", None)).unwrap();
        live_run(&conn, "daily", "session-1");
        live_run(&conn, "healthy", "healthy-session");
        let spool_path = new_delivery_spool_path(target.to_str().unwrap()).unwrap();
        begin_spool_preparation(
            &conn,
            &SpoolLedgerTarget {
                run_id: "run-session-1",
                occurrence_id: "occ-session-1",
                target: target.to_str().unwrap(),
            },
            &spool_path,
        )
        .unwrap();
        symlink(&unrelated, &spool_path).unwrap();

        let report = reconcile_orphaned_delivery_spools_for_owner(&conn, "restart-owner").unwrap();

        assert_eq!(report.degraded.len(), 1);
        assert!(spool_path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&unrelated).unwrap(), "keep");
        let affected: (String, String, String) = conn
            .query_row(
                "SELECT o.state, r.status, r.delivery_spool_state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            affected,
            (
                "failed".to_string(),
                "failed".to_string(),
                "degraded".to_string()
            )
        );
        let healthy: String = conn
            .query_row(
                "SELECT status FROM automation_runs WHERE id = 'run-healthy-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(healthy, "running");
    }

    #[test]
    fn undeletable_spool_is_marked_degraded_without_aborting_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "orphaned spool");
        let plan = prepare_tracked_delivery_spool(
            &conn,
            SpoolLedgerTarget {
                run_id: "run-session-1",
                occurrence_id: "occ-session-1",
                target: target.to_str().unwrap(),
            },
            "session-1",
        )
        .unwrap()
        .unwrap();
        let spool_path = plan.spool_path.clone();
        std::mem::forget(plan);
        set_spool_remove_failure_for_test(true);

        let report = reconcile_orphaned_delivery_spools_for_owner(&conn, "restart-owner").unwrap();

        assert_eq!(report.removed, 0);
        assert_eq!(report.degraded.len(), 1);
        assert!(spool_path.exists());
        let states: (String, String, String, String) = conn
            .query_row(
                "SELECT o.state, o.delivery_state, r.status, r.delivery_spool_state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            states,
            (
                "failed".to_string(),
                "ambiguous".to_string(),
                "failed".to_string(),
                "degraded".to_string()
            )
        );
    }

    #[test]
    fn concurrent_reconcilers_serialize_preparing_spooled_and_pending_delivery() {
        for phase in ["preparing", "spooled", "pending"] {
            let temp = tempfile::tempdir().unwrap();
            let store_path = temp.path().join("store.sqlite");
            initialize_store(&store_path).unwrap();
            let conn = crate::store::open_store(&store_path).unwrap();
            let target = temp.path().join(format!("{phase}.txt"));
            let automation_id = format!("concurrent-{phase}");
            let session_id = format!("session-{phase}");
            let run_id = format!("run-{session_id}");
            insert_definition(
                &conn,
                &definition(&automation_id, Some(target.to_str().unwrap())),
            )
            .unwrap();
            live_run(&conn, &automation_id, &session_id);
            session_record(&conn, &session_id, "completed", Some(0));
            event(&conn, &session_id, "output", phase);
            drop(conn);

            let (reached_tx, reached_rx) = std::sync::mpsc::sync_channel(0);
            let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
            install_settlement_phase_hook(SettlementPhaseHook {
                run_id: run_id.clone(),
                phase,
                reached: reached_tx,
                release: release_rx,
            });

            let first_path = store_path.clone();
            let first = std::thread::spawn(move || {
                let conn = crate::store::open_store(&first_path).unwrap();
                settle_finished_runs_with_runtime(
                    &conn,
                    &crate::api::NoopSessionRuntime,
                    Utc::now(),
                )
            });
            reached_rx.recv().unwrap();

            let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
            let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
            let second_path = store_path.clone();
            let second = std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                let conn = crate::store::open_store(&second_path).unwrap();
                let result = settle_finished_runs_with_runtime(
                    &conn,
                    &crate::api::NoopSessionRuntime,
                    Utc::now(),
                );
                done_tx.send(()).unwrap();
                result
            });
            started_rx.recv().unwrap();
            std::thread::yield_now();
            assert!(
                matches!(
                    done_rx.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Empty)
                ),
                "second reconciler must wait while `{phase}` is live"
            );

            release_tx.send(()).unwrap();
            let first_report = first.join().unwrap().unwrap();
            let second_report = second.join().unwrap().unwrap();
            assert_eq!(
                first_report.settled_succeeded + second_report.settled_succeeded,
                1
            );
            assert_eq!(std::fs::read_to_string(&target).unwrap(), phase);
            let conn = crate::store::open_store(&store_path).unwrap();
            let states: (String, String) = conn
                .query_row(
                    "SELECT status, delivery_state FROM automation_runs WHERE id = ?1",
                    params![run_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(states, ("succeeded".to_string(), "committed".to_string()));
        }
    }

    #[test]
    fn delivery_commit_strategy_matches_platform_durability_contract() {
        let expected = if cfg!(windows) {
            "windows-write-through-replace"
        } else if cfg!(unix) {
            "unix-file-and-directory-sync"
        } else {
            "unsupported-ambiguous"
        };
        assert_eq!(delivery_commit_strategy(), expected);
    }

    #[test]
    fn settlement_uses_the_output_target_pinned_when_the_run_started() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_target = first.path().join("payload.md");
        let second_target = second.path().join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(
            &conn,
            &definition("daily", Some(first_target.to_str().unwrap())),
        )
        .unwrap();
        live_run(&conn, "daily", "session-1");
        super::super::store::update_definition(
            &conn,
            &definition("daily", Some(second_target.to_str().unwrap())),
        )
        .unwrap();
        super::super::store::delete_definition(&conn, "daily").unwrap();
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "immutable delivery");

        let report = settle_finished_runs(
            &conn,
            chrono::DateTime::parse_from_rfc3339("2026-08-28T09:30:00.000Z")
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap();

        assert_eq!(report.settled_succeeded, 1);
        assert_eq!(
            std::fs::read_to_string(&first_target).unwrap(),
            "immutable delivery"
        );
        assert!(
            !second_target.exists(),
            "an in-flight run must never reload a revised delivery target"
        );
    }

    #[test]
    fn settle_delivers_output_and_records_success() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("out").join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "the delivered payload");

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();
        assert_eq!(report.settled_succeeded, 1);
        assert_eq!(report.settled_failed, 0);

        let run: (
            String,
            Option<i64>,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT status, exit_code, output_commit, delivery_state,
                        delivery_token, delivery_digest
                 FROM automation_runs WHERE id = 'run-session-1'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(run.0, "succeeded");
        assert_eq!(run.1, Some(0));
        assert_eq!(run.2.as_deref(), Some(target.to_str().unwrap()));
        assert_eq!(run.3, "committed");
        assert!(run
            .4
            .as_deref()
            .is_some_and(|token| token.starts_with("delivery-")));
        assert!(run
            .5
            .as_deref()
            .is_some_and(|digest| digest.starts_with("sha256:")));
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "the delivered payload"
        );

        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "succeeded");
    }

    #[test]
    fn output_loss_marker_refuses_partial_delivery_as_ambiguous() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "surviving prefix");
        event(&conn, "session-1", "output_truncated", "dropped output");
        event(&conn, "session-1", "output", "surviving suffix");

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();

        assert_eq!(report.settled_failed, 1);
        assert!(!target.exists());
        let states: (String, String, String, String) = conn
            .query_row(
                "SELECT o.state, o.delivery_state, r.status, r.delivery_state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            states,
            (
                "failed".to_string(),
                "ambiguous".to_string(),
                "failed".to_string(),
                "ambiguous".to_string()
            )
        );
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.contains("incomplete")));
    }

    #[test]
    fn failed_output_commit_fails_the_run_visibly() {
        let temp = tempfile::tempdir().unwrap();
        let blocker = temp.path().join("blocker");
        std::fs::write(&blocker, "a file, not a directory").unwrap();
        let target = blocker.join("payload.md");

        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "the delivered payload");

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();
        assert_eq!(report.settled_failed, 1);
        assert!(
            report
                .failures
                .iter()
                .any(|failure| failure.contains("output commit failed")),
            "{report:?}"
        );

        let (status, delivery_state): (String, String) = conn
            .query_row(
                "SELECT status, delivery_state
                 FROM automation_runs WHERE id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            status, "failed",
            "a failed delivery must not report success"
        );
        assert_eq!(delivery_state, "failed");
        let reason: String = conn
            .query_row(
                "SELECT failure_reason FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(reason.contains("output commit failed"), "{reason}");
    }

    #[test]
    fn delivery_reservation_cas_failure_prevents_file_write() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "must not be delivered");
        conn.execute_batch(
            "CREATE TRIGGER reject_delivery_reservation
             BEFORE UPDATE OF delivery_state ON automation_runs
             WHEN NEW.delivery_state = 'pending'
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic reservation rejection');
             END;",
        )
        .unwrap();

        let error = settle_finished_runs(&conn, Utc::now()).unwrap_err();

        assert!(error.contains("reservation"), "{error}");
        assert!(!target.exists(), "I/O must follow a committed reservation");
        let states: (String, String) = conn
            .query_row(
                "SELECT o.state, r.status
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(states, ("running".to_string(), "running".to_string()));
    }

    #[test]
    fn pending_delivery_retry_becomes_ambiguous_without_repeating_io() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "must not be replayed");
        conn.execute(
            "UPDATE automation_runs
             SET delivery_state = 'pending',
                 delivery_token = 'delivery-token',
                 delivery_digest = 'sha256:pending'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE automation_occurrences
             SET delivery_state = 'pending',
                 delivery_token = 'delivery-token',
                 delivery_digest = 'sha256:pending'",
            [],
        )
        .unwrap();

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();

        assert_eq!(report.settled_failed, 1);
        assert!(
            !target.exists(),
            "an uncertain prior delivery is never repeated"
        );
        let states: (String, String, String, String) = conn
            .query_row(
                "SELECT o.state, o.delivery_state, r.status, r.delivery_state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            states,
            (
                "failed".to_string(),
                "ambiguous".to_string(),
                "failed".to_string(),
                "ambiguous".to_string()
            )
        );
        let second = settle_finished_runs(&conn, Utc::now()).unwrap();
        assert_eq!(second.settled_succeeded + second.settled_failed, 0);
        assert!(!target.exists());
    }

    #[test]
    fn delivery_finalization_cas_failure_is_not_replayed_silently() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "delivered once");
        conn.execute_batch(
            "CREATE TRIGGER reject_delivery_finalization
             BEFORE UPDATE OF delivery_state ON automation_runs
             WHEN OLD.delivery_state = 'pending' AND NEW.delivery_state = 'committed'
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic finalization rejection');
             END;",
        )
        .unwrap();

        let error = settle_finished_runs(&conn, Utc::now()).unwrap_err();

        assert!(error.contains("finalize"), "{error}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "delivered once");
        std::fs::write(&target, "external edit after uncertain commit").unwrap();
        conn.execute_batch("DROP TRIGGER reject_delivery_finalization")
            .unwrap();
        let report = settle_finished_runs(&conn, Utc::now()).unwrap();
        assert_eq!(report.settled_failed, 1);
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "external edit after uncertain commit",
            "retry must not repeat an unrecorded delivery"
        );
    }

    #[test]
    fn post_rename_parent_sync_failure_is_recorded_as_ambiguous() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(
            &conn,
            "session-1",
            "output",
            "renamed but not directory-synced",
        );
        set_parent_sync_failure_for_test(true);

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();

        assert_eq!(report.settled_failed, 1);
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "renamed but not directory-synced"
        );
        let states: (String, String, String, String) = conn
            .query_row(
                "SELECT o.state, o.delivery_state, r.status, r.delivery_state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            states,
            (
                "failed".to_string(),
                "ambiguous".to_string(),
                "failed".to_string(),
                "ambiguous".to_string()
            )
        );
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.contains("sync directory")));
    }

    #[test]
    fn failed_session_fails_the_run_without_delivery() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "failed", Some(1));
        event(&conn, "session-1", "output", "partial output");

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();
        assert_eq!(report.settled_failed, 1);

        let (status, exit_code, output_commit): (String, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT status, exit_code, output_commit FROM automation_runs WHERE id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(exit_code, Some(1));
        assert_eq!(output_commit, None);
        assert!(!target.exists(), "nothing is delivered for a failed run");
    }

    #[test]
    fn recovered_occurrence_settles_the_ledger_as_failed() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", None)).unwrap();
        live_run(&conn, "daily", "session-1");
        // Lease recovery already failed the occurrence (no sessions row was
        // ever written — the daemon died mid-run).
        conn.execute(
            "UPDATE automation_occurrences SET state = 'failed',
                 failure_reason = 'lease expired'",
            [],
        )
        .unwrap();

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();
        assert_eq!(report.settled_failed, 1);

        let (status, log): (String, Option<String>) = conn
            .query_row(
                "SELECT status, log_json FROM automation_runs WHERE id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "failed");
        assert!(log.is_none(), "no stream exists to capture");
    }

    #[test]
    fn completion_before_deadline_repairs_a_stale_lease_failure_consistently() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", None)).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        conn.execute(
            "UPDATE automation_occurrences
             SET state = 'failed', failure_reason = 'lease expired'
             WHERE automation_id = 'daily'",
            [],
        )
        .unwrap();

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();

        assert_eq!(report.settled_succeeded, 1);
        let (occurrence, run): (String, String) = conn
            .query_row(
                "SELECT o.state, r.status
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE o.automation_id = 'daily'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (occurrence.as_str(), run.as_str()),
            ("succeeded", "succeeded")
        );
    }

    #[test]
    fn archival_after_deadline_does_not_rewrite_pre_deadline_completion_time() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", None)).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "running", None);
        crate::store::update_session_status(
            &conn,
            "session-1",
            "completed",
            Some(0),
            "2026-08-28T09:05:00.000Z",
        )
        .unwrap();
        crate::store::archive_session(&conn, "session-1", "2026-08-28T10:30:00.000Z").unwrap();
        crate::store::summon_session(&conn, "session-1", "2026-08-28T10:40:00.000Z").unwrap();

        let report = settle_finished_runs(
            &conn,
            chrono::DateTime::parse_from_rfc3339("2026-08-28T11:00:00.000Z")
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap();

        assert_eq!(report.settled_succeeded, 1);
        let states: (String, String) = conn
            .query_row(
                "SELECT o.state, r.status
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(states, ("succeeded".to_string(), "succeeded".to_string()));
    }

    #[test]
    fn completion_after_deadline_fails_occurrence_and_run_consistently() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", None)).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "running", None);
        crate::store::update_session_status(
            &conn,
            "session-1",
            "completed",
            Some(0),
            "2026-08-28T10:05:00.000Z",
        )
        .unwrap();

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();

        assert_eq!(report.settled_failed, 1);
        let (occurrence, run, reason): (String, String, String) = conn
            .query_row(
                "SELECT o.state, r.status, o.failure_reason
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE o.automation_id = 'daily'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((occurrence.as_str(), run.as_str()), ("failed", "failed"));
        assert!(reason.contains("deadline"), "{reason}");
    }

    #[test]
    fn unproven_timeout_kill_keeps_fence_and_records_ambiguity_without_retrying() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", None)).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "running", None);
        let runtime = FailingKillRuntime::default();
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-28T11:00:00.000Z")
            .unwrap()
            .with_timezone(&Utc);

        let first = settle_finished_runs_with_runtime(&conn, &runtime, now).unwrap();
        let second = settle_finished_runs_with_runtime(&conn, &runtime, now).unwrap();

        assert_eq!(runtime.kills.get(), 1, "ambiguous kill is not repeated");
        assert_eq!(first.still_running, 1);
        assert_eq!(second.still_running, 1);
        let states: (String, String, String, String) = conn
            .query_row(
                "SELECT o.state, o.termination_state, r.status, r.termination_state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            states,
            (
                "running".to_string(),
                "ambiguous".to_string(),
                "running".to_string(),
                "ambiguous".to_string()
            )
        );
        assert!(first
            .failures
            .iter()
            .any(|failure| failure.contains("could not be proven")));
    }

    #[test]
    fn contradictory_terminal_occurrence_is_reported_not_silently_overwritten() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", None)).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        conn.execute(
            "UPDATE automation_occurrences
             SET state = 'failed', failure_reason = 'operator decision'
             WHERE automation_id = 'daily'",
            [],
        )
        .unwrap();

        let error = settle_finished_runs(&conn, Utc::now()).unwrap_err();

        assert!(error.contains("contradictory"), "{error}");
        let status: String = conn
            .query_row(
                "SELECT status FROM automation_runs WHERE id = 'run-session-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "running");
    }

    #[test]
    fn still_running_rows_are_left_alone() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", None)).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "running", None);

        let report = settle_finished_runs(
            &conn,
            chrono::DateTime::parse_from_rfc3339("2026-08-28T09:30:00.000Z")
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap();
        assert_eq!(report.still_running, 1);
        let status: String = conn
            .query_row(
                "SELECT status FROM automation_runs WHERE id = 'run-session-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "running");
    }
}
