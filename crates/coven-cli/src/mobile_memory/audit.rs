use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use super::config::{atomic_replace_private, ensure_private_mobile_dir, validate_private_file};

const AUDIT_FILE: &str = "audit.jsonl";
const MAX_AUDIT_BYTES: u64 = 4 * 1024 * 1024;
const AUDIT_OUTBOX_FILE: &str = "audit-outbox.json";
/// Bound on undelivered cancellation tokens when the audit file stays
/// unhealthy: the outbox must not grow without limit.
const MAX_PENDING_AUDIT_EVENTS: usize = 64;
static AUDIT_WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static AUDIT_OUTBOX_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileAuditEvent {
    GatewayStarted,
    GatewayStopped,
    PairingCreated,
    PairingCompleted,
    PairingCancelled,
    PairingRejected,
    DeviceRevoked,
    AuthenticationRejected,
    RateLimited,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MobileAuditRecord {
    timestamp: DateTime<Utc>,
    event: MobileAuditEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_id: Option<Uuid>,
}

pub fn append_event(
    coven_home: &Path,
    timestamp: DateTime<Utc>,
    event: MobileAuditEvent,
    device_id: Option<Uuid>,
) -> Result<()> {
    let _guard = AUDIT_WRITE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("mobile audit lock was poisoned"))?;
    let directory = ensure_private_mobile_dir(coven_home)?;
    let path = directory.join(AUDIT_FILE);
    if path.exists() {
        validate_private_file(&path)?;
        if path
            .metadata()
            .with_context(|| format!("failed to inspect {}", path.display()))?
            .len()
            >= MAX_AUDIT_BYTES
        {
            atomic_replace_private(&path, b"")?;
        }
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    serde_json::to_writer(
        &mut file,
        &MobileAuditRecord {
            timestamp,
            event,
            device_id,
        },
    )
    .context("failed to encode mobile audit event")?;
    file.write_all(b"\n")
        .context("failed to finish mobile audit event")?;
    file.sync_data()
        .context("failed to sync mobile audit event")?;
    validate_private_file(&path)
}

/// Read the pending cancellation tokens, tolerating an absent or empty file.
fn read_pending_pairing_cancellations(coven_home: &Path) -> Result<Vec<Uuid>> {
    let path = ensure_private_mobile_dir(coven_home)?.join(AUDIT_OUTBOX_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    validate_private_file(&path)?;
    let raw = std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_slice(&raw).context("mobile audit outbox is malformed")
}

fn write_pending_pairing_cancellations(coven_home: &Path, pending: &[Uuid]) -> Result<()> {
    let path = ensure_private_mobile_dir(coven_home)?.join(AUDIT_OUTBOX_FILE);
    let encoded =
        serde_json::to_vec_pretty(pending).context("failed to encode mobile audit outbox")?;
    atomic_replace_private(&path, &encoded)
}

/// Persist an idempotent audit-pending token for a cancelled pairing.
///
/// Returns whether a new token was recorded; a token for the same pairing is
/// never duplicated. The token is the durable memory that a
/// `pairing_cancelled` audit record is still owed, so a failed
/// [`append_event`] can be retried by any later terminal replay of that
/// pairing instead of being lost.
pub fn record_pending_pairing_cancelled(coven_home: &Path, pairing_id: Uuid) -> Result<bool> {
    let _guard = AUDIT_OUTBOX_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("mobile audit outbox lock was poisoned"))?;
    let mut pending = read_pending_pairing_cancellations(coven_home)?;
    if pending.contains(&pairing_id) {
        return Ok(false);
    }
    if pending.len() >= MAX_PENDING_AUDIT_EVENTS {
        return Ok(false);
    }
    pending.push(pairing_id);
    write_pending_pairing_cancellations(coven_home, &pending)?;
    Ok(true)
}

/// Drop a pairing's pending token once its audit record was delivered.
pub fn remove_pending_pairing_cancelled(coven_home: &Path, pairing_id: Uuid) -> Result<()> {
    let _guard = AUDIT_OUTBOX_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("mobile audit outbox lock was poisoned"))?;
    let pending = read_pending_pairing_cancellations(coven_home)?;
    if !pending.contains(&pairing_id) {
        return Ok(());
    }
    let remaining: Vec<Uuid> = pending.into_iter().filter(|id| *id != pairing_id).collect();
    write_pending_pairing_cancellations(coven_home, &remaining)
}

/// Retry delivery of every pending cancellation token. Best-effort: tokens
/// whose append fails stay pending for the next terminal replay, and delivery
/// is therefore at-least-once. Returns how many records were delivered.
pub fn flush_pending_pairing_cancellations(coven_home: &Path) -> usize {
    let _guard = match AUDIT_OUTBOX_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("mobile audit outbox lock was poisoned"))
    {
        Ok(guard) => guard,
        Err(_) => return 0,
    };
    let pending = match read_pending_pairing_cancellations(coven_home) {
        Ok(pending) => pending,
        Err(_) => return 0,
    };
    if pending.is_empty() {
        return 0;
    }
    let mut delivered = Vec::new();
    for pairing_id in &pending {
        match append_event(
            coven_home,
            Utc::now(),
            MobileAuditEvent::PairingCancelled,
            None,
        ) {
            Ok(()) => delivered.push(*pairing_id),
            // The next terminal replay retries; nothing is lost by leaving
            // the token in place.
            Err(_) => {}
        }
    }
    if !delivered.is_empty() {
        let remaining: Vec<Uuid> = pending
            .into_iter()
            .filter(|id| !delivered.contains(id))
            .collect();
        // A failed rewrite keeps tokens that were already delivered, so a
        // later replay may append a duplicate `pairing_cancelled` record; the
        // log stays truthful, just at-least-once.
        let _ = write_pending_pairing_cancellations(coven_home, &remaining);
    }
    delivered.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_records_only_allowed_coarse_fields() {
        let temp = tempfile::tempdir().unwrap();
        append_event(
            temp.path(),
            DateTime::from_timestamp(1_785_326_400, 0).unwrap(),
            MobileAuditEvent::AuthenticationRejected,
            Some(Uuid::from_u128(1)),
        )
        .unwrap();
        let line = std::fs::read_to_string(temp.path().join("mobile/audit.jsonl")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 3);
        for forbidden in [
            "path",
            "memoryId",
            "endpoint",
            "fingerprint",
            "nonce",
            "signature",
            "body",
        ] {
            assert!(value.get(forbidden).is_none());
        }
    }

    #[test]
    fn pending_cancellation_tokens_are_idempotent_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let first = Uuid::from_u128(1);
        assert!(record_pending_pairing_cancelled(temp.path(), first).unwrap());
        // A token for the same pairing is never duplicated.
        assert!(!record_pending_pairing_cancelled(temp.path(), first).unwrap());
        for index in 2..(MAX_PENDING_AUDIT_EVENTS as u128 + 2) {
            let added =
                record_pending_pairing_cancelled(temp.path(), Uuid::from_u128(index)).unwrap();
            if index <= MAX_PENDING_AUDIT_EVENTS as u128 {
                assert!(added, "token {index} should fit the outbox bound");
            } else {
                assert!(!added, "token {index} must be refused past the bound");
            }
        }
        let raw = std::fs::read_to_string(temp.path().join("mobile/audit-outbox.json")).unwrap();
        assert!(raw.contains(&first.to_string()));
    }

    #[test]
    fn flushed_cancellation_tokens_deliver_exactly_once() {
        let temp = tempfile::tempdir().unwrap();
        let pairing = Uuid::from_u128(7);
        assert!(record_pending_pairing_cancelled(temp.path(), pairing).unwrap());

        assert_eq!(flush_pending_pairing_cancellations(temp.path()), 1);
        let line = std::fs::read_to_string(temp.path().join("mobile/audit.jsonl")).unwrap();
        assert!(line.contains("pairing_cancelled"));
        let outbox = std::fs::read_to_string(temp.path().join("mobile/audit-outbox.json")).unwrap();
        assert_eq!(outbox.trim(), "[]");
        // A later terminal replay finds nothing pending and must not duplicate
        // the record.
        assert_eq!(flush_pending_pairing_cancellations(temp.path()), 0);
        let line = std::fs::read_to_string(temp.path().join("mobile/audit.jsonl")).unwrap();
        assert_eq!(line.matches("pairing_cancelled").count(), 1);
    }

    #[test]
    fn removing_a_pending_token_leaves_others_queued() {
        let temp = tempfile::tempdir().unwrap();
        let kept = Uuid::from_u128(2);
        assert!(record_pending_pairing_cancelled(temp.path(), Uuid::from_u128(1)).unwrap());
        assert!(record_pending_pairing_cancelled(temp.path(), kept).unwrap());
        remove_pending_pairing_cancelled(temp.path(), Uuid::from_u128(1)).unwrap();
        // Removing an unknown token is a no-op, not an error.
        remove_pending_pairing_cancelled(temp.path(), Uuid::from_u128(99)).unwrap();
        let outbox = std::fs::read_to_string(temp.path().join("mobile/audit-outbox.json")).unwrap();
        assert!(!outbox.contains("00000000-0000-0000-0000-000000000001"));
        assert!(outbox.contains(&kept.to_string()));
    }

    #[test]
    fn failed_delivery_keeps_the_token_pending() {
        let temp = tempfile::tempdir().unwrap();
        let pairing = Uuid::from_u128(3);
        assert!(record_pending_pairing_cancelled(temp.path(), pairing).unwrap());
        // A symlinked audit file fails the private-file validation and the
        // O_NOFOLLOW open, modelling a delivery failure without breaking the
        // (still writable) mobile directory that holds the outbox.
        #[cfg(unix)]
        std::os::unix::fs::symlink("/dev/null", temp.path().join("mobile/audit.jsonl")).unwrap();
        assert_eq!(flush_pending_pairing_cancellations(temp.path()), 0);
        let outbox = std::fs::read_to_string(temp.path().join("mobile/audit-outbox.json")).unwrap();
        assert!(outbox.contains(&pairing.to_string()));
    }
}
