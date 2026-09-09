use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::{LazyLock, Mutex, MutexGuard};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::config::{atomic_replace_private, ensure_private_mobile_dir, validate_private_file};

const AUDIT_FILE: &str = "audit.jsonl";
const AUDIT_LOCK_FILE: &str = ".audit.lock";
const MAX_AUDIT_BYTES: u64 = 4 * 1024 * 1024;
static AUDIT_WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct AuditStoreLock {
    _process: MutexGuard<'static, ()>,
    file: std::fs::File,
}

impl AuditStoreLock {
    fn acquire(coven_home: &Path) -> Result<Self> {
        let directory = ensure_private_mobile_dir(coven_home)?;
        let process = AUDIT_WRITE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile audit lock was poisoned"))?;
        let lock_path = directory.join(AUDIT_LOCK_FILE);
        let file = crate::state_lock::open_lock_file(&lock_path)?;
        file.lock_exclusive()
            .with_context(|| format!("failed to lock {}", lock_path.display()))?;
        Ok(Self {
            _process: process,
            file,
        })
    }
}

impl Drop for AuditStoreLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileAuditEvent {
    GatewayStarted,
    GatewayStopped,
    PairingCreated,
    PairingCompleted,
    PairingRejected,
    DeviceRenamed,
    DeviceGrantReissued,
    DeviceAuthorizationReenrolled,
    DeviceLostRevoked,
    DeviceCompromiseRevoked,
    DeviceRevoked,
    AuthenticationRejected,
    RateLimited,
    StepUpVerified,
    StepUpRejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeviceRotationAuditTransition {
    pub transition_id: Uuid,
    pub source_device_id: Uuid,
    pub replacement_device_id: Uuid,
    pub source_grant_id: Uuid,
    pub source_revocation_epoch: u64,
    pub replacement_grant_id: Uuid,
    pub replacement_revocation_epoch: u64,
    pub occurred_at: DateTime<Utc>,
    pub audited_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub authorization_key_cleanup_completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeviceGrantReissueAuditTransition {
    pub transition_id: Uuid,
    pub device_id: Uuid,
    pub requested_policy_digest: String,
    pub previous_grant_id: Uuid,
    pub previous_revocation_epoch: u64,
    pub replacement_grant_id: Uuid,
    pub replacement_revocation_epoch: u64,
    pub occurred_at: DateTime<Utc>,
    pub audited_at: Option<DateTime<Utc>>,
}

pub(crate) struct AuditDeliveryReceipt {
    transition_id: Uuid,
}

impl AuditDeliveryReceipt {
    pub(crate) fn transition_id(&self) -> Uuid {
        self.transition_id
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MobileAuditRecord {
    timestamp: DateTime<Utc>,
    event: MobileAuditEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transition_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_device_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replacement_device_id: Option<Uuid>,
}

pub fn append_event(
    coven_home: &Path,
    timestamp: DateTime<Utc>,
    event: MobileAuditEvent,
    device_id: Option<Uuid>,
) -> Result<()> {
    let _guard = AuditStoreLock::acquire(coven_home)?;
    append_record_locked(
        coven_home,
        &MobileAuditRecord {
            timestamp,
            event,
            device_id,
            transition_id: None,
            source_device_id: None,
            replacement_device_id: None,
        },
    )
}

pub(crate) fn append_rotation_event(
    coven_home: &Path,
    transition: &DeviceRotationAuditTransition,
) -> Result<AuditDeliveryReceipt> {
    let _guard = AuditStoreLock::acquire(coven_home)?;
    append_transition_record_locked(
        coven_home,
        transition.transition_id,
        &MobileAuditRecord {
            timestamp: transition.occurred_at,
            event: MobileAuditEvent::DeviceAuthorizationReenrolled,
            device_id: None,
            transition_id: Some(transition.transition_id),
            source_device_id: Some(transition.source_device_id),
            replacement_device_id: Some(transition.replacement_device_id),
        },
    )
}

pub(crate) fn append_grant_reissue_event(
    coven_home: &Path,
    transition: &DeviceGrantReissueAuditTransition,
) -> Result<AuditDeliveryReceipt> {
    let _guard = AuditStoreLock::acquire(coven_home)?;
    append_transition_record_locked(
        coven_home,
        transition.transition_id,
        &MobileAuditRecord {
            timestamp: transition.occurred_at,
            event: MobileAuditEvent::DeviceGrantReissued,
            device_id: Some(transition.device_id),
            transition_id: Some(transition.transition_id),
            source_device_id: None,
            replacement_device_id: None,
        },
    )
}

fn append_transition_record_locked(
    coven_home: &Path,
    transition_id: Uuid,
    record: &MobileAuditRecord,
) -> Result<AuditDeliveryReceipt> {
    let path = ensure_private_mobile_dir(coven_home)?.join(AUDIT_FILE);
    let mut existing = Vec::new();
    if path.exists() {
        validate_private_file(&path)?;
        existing =
            std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
        if existing.len() as u64 >= MAX_AUDIT_BYTES {
            existing.clear();
        }
        let (found_transition, recovered_suffix) =
            audit_prefix_contains_transition_or_recovers_suffix(
                &mut existing,
                transition_id,
                &path,
            )?;
        if found_transition {
            if recovered_suffix {
                atomic_replace_private(&path, &existing)?;
            }
            return Ok(AuditDeliveryReceipt { transition_id });
        }
    }
    serde_json::to_writer(&mut existing, record)
        .context("failed to encode mobile transition audit event")?;
    existing.push(b'\n');
    atomic_replace_private(&path, &existing)?;
    Ok(AuditDeliveryReceipt { transition_id })
}

fn audit_prefix_contains_transition_or_recovers_suffix(
    bytes: &mut Vec<u8>,
    transition_id: Uuid,
    path: &Path,
) -> Result<(bool, bool)> {
    let mut cursor = 0;
    let mut found_transition = false;
    while cursor < bytes.len() {
        let Some(relative_newline) = bytes[cursor..].iter().position(|byte| *byte == b'\n') else {
            match serde_json::from_slice::<MobileAuditRecord>(&bytes[cursor..]) {
                Ok(record) => {
                    if record.transition_id == Some(transition_id) {
                        found_transition = true;
                    }
                    bytes.push(b'\n');
                }
                Err(_) => bytes.truncate(cursor),
            }
            return Ok((found_transition, true));
        };
        let line_end = cursor + relative_newline;
        match serde_json::from_slice::<MobileAuditRecord>(&bytes[cursor..line_end]) {
            Ok(record) => {
                if record.transition_id == Some(transition_id) {
                    found_transition = true;
                }
                cursor = line_end + 1;
            }
            Err(_) if line_end + 1 == bytes.len() => {
                bytes.truncate(cursor);
                return Ok((found_transition, true));
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to parse complete audit record before later evidence in {}",
                        path.display()
                    )
                });
            }
        }
    }
    Ok((found_transition, false))
}

fn append_record_locked(coven_home: &Path, record: &MobileAuditRecord) -> Result<()> {
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
    serde_json::to_writer(&mut file, record).context("failed to encode mobile audit event")?;
    file.write_all(b"\n")
        .context("failed to finish mobile audit event")?;
    file.sync_data()
        .context("failed to sync mobile audit event")?;
    validate_private_file(&path)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::thread;
    use std::time::Duration;

    use fs2::FileExt;

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
    fn step_up_audit_events_remain_coarse() {
        let temp = tempfile::tempdir().unwrap();
        for event in [
            MobileAuditEvent::StepUpVerified,
            MobileAuditEvent::StepUpRejected,
        ] {
            append_event(
                temp.path(),
                DateTime::from_timestamp(1_785_326_400, 0).unwrap(),
                event,
                Some(Uuid::from_u128(1)),
            )
            .unwrap();
        }
        let lines = std::fs::read_to_string(temp.path().join("mobile/audit.jsonl")).unwrap();
        assert!(lines.contains("\"event\":\"step_up_verified\""));
        assert!(lines.contains("\"event\":\"step_up_rejected\""));
        assert!(!lines.contains("challenge"));
        assert!(!lines.contains("assuranceClass"));
        assert!(!lines.contains("authorizationKey"));
    }

    #[test]
    fn ordinary_and_rotation_writers_share_one_interprocess_lock_without_dropping_events() {
        let temp = tempfile::tempdir().unwrap();
        let mobile = ensure_private_mobile_dir(temp.path()).unwrap();
        let lock_path = mobile.join(".audit.lock");
        let held_lock = crate::state_lock::open_lock_file(&lock_path).unwrap();
        held_lock.lock_exclusive().unwrap();
        let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        let first = DeviceRotationAuditTransition {
            transition_id: Uuid::from_u128(10),
            source_device_id: Uuid::from_u128(1),
            replacement_device_id: Uuid::from_u128(2),
            source_grant_id: Uuid::from_u128(11),
            source_revocation_epoch: 1,
            replacement_grant_id: Uuid::from_u128(12),
            replacement_revocation_epoch: 1,
            occurred_at: now,
            audited_at: None,
            authorization_key_cleanup_completed_at: None,
        };
        let second = DeviceRotationAuditTransition {
            transition_id: Uuid::from_u128(20),
            source_device_id: Uuid::from_u128(3),
            replacement_device_id: Uuid::from_u128(4),
            source_grant_id: Uuid::from_u128(21),
            source_revocation_epoch: 1,
            replacement_grant_id: Uuid::from_u128(22),
            replacement_revocation_epoch: 1,
            occurred_at: now,
            audited_at: None,
            authorization_key_cleanup_completed_at: None,
        };
        let ordinary_home = temp.path().to_path_buf();
        let ordinary = thread::spawn(move || {
            append_event(
                &ordinary_home,
                now,
                MobileAuditEvent::DeviceRenamed,
                Some(Uuid::from_u128(5)),
            )
        });
        let first_home = temp.path().to_path_buf();
        let first_writer = thread::spawn(move || append_rotation_event(&first_home, &first));
        let second_home = temp.path().to_path_buf();
        let second_writer = thread::spawn(move || append_rotation_event(&second_home, &second));

        thread::sleep(Duration::from_millis(100));
        assert!(!ordinary.is_finished());
        assert!(!first_writer.is_finished());
        assert!(!second_writer.is_finished());

        held_lock.unlock().unwrap();
        ordinary.join().unwrap().unwrap();
        first_writer.join().unwrap().unwrap();
        second_writer.join().unwrap().unwrap();

        let audit = fs::read_to_string(mobile.join(AUDIT_FILE)).unwrap();
        assert_eq!(audit.lines().count(), 3);
        assert!(audit.contains("\"event\":\"device_renamed\""));
        assert!(audit.contains(&Uuid::from_u128(10).to_string()));
        assert!(audit.contains(&Uuid::from_u128(20).to_string()));
    }

    #[test]
    fn transition_retry_discards_only_a_truncated_final_audit_suffix() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        append_event(
            temp.path(),
            now,
            MobileAuditEvent::DeviceRenamed,
            Some(Uuid::from_u128(1)),
        )
        .unwrap();
        let path = temp.path().join("mobile").join(AUDIT_FILE);
        let transition = DeviceRotationAuditTransition {
            transition_id: Uuid::from_u128(10),
            source_device_id: Uuid::from_u128(2),
            replacement_device_id: Uuid::from_u128(3),
            source_grant_id: Uuid::from_u128(11),
            source_revocation_epoch: 1,
            replacement_grant_id: Uuid::from_u128(12),
            replacement_revocation_epoch: 1,
            occurred_at: now,
            audited_at: None,
            authorization_key_cleanup_completed_at: None,
        };
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(
            format!(
                "{{\"timestamp\":\"2026-01-01T00:00:00Z\",\"event\":\"device_authorization_reenrolled\",\"transitionId\":\"{}",
                transition.transition_id
            )
            .as_bytes(),
        )
        .unwrap();
        file.sync_data().unwrap();

        append_rotation_event(temp.path(), &transition).unwrap();
        append_rotation_event(temp.path(), &transition).unwrap();

        let audit = fs::read_to_string(path).unwrap();
        assert_eq!(audit.lines().count(), 2);
        assert!(audit.contains("\"event\":\"device_renamed\""));
        assert_eq!(
            audit
                .matches("\"event\":\"device_authorization_reenrolled\"")
                .count(),
            1
        );
        assert_eq!(
            audit.matches(&transition.transition_id.to_string()).count(),
            1
        );
    }

    #[test]
    fn transition_retry_preserves_and_deduplicates_valid_final_record_without_newline() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        let path = ensure_private_mobile_dir(temp.path())
            .unwrap()
            .join(AUDIT_FILE);
        let ordinary = serde_json::to_vec(&MobileAuditRecord {
            timestamp: now,
            event: MobileAuditEvent::DeviceRenamed,
            device_id: Some(Uuid::from_u128(1)),
            transition_id: None,
            source_device_id: None,
            replacement_device_id: None,
        })
        .unwrap();
        atomic_replace_private(&path, &ordinary).unwrap();
        let transition = DeviceRotationAuditTransition {
            transition_id: Uuid::from_u128(10),
            source_device_id: Uuid::from_u128(2),
            replacement_device_id: Uuid::from_u128(3),
            source_grant_id: Uuid::from_u128(11),
            source_revocation_epoch: 1,
            replacement_grant_id: Uuid::from_u128(12),
            replacement_revocation_epoch: 1,
            occurred_at: now,
            audited_at: None,
            authorization_key_cleanup_completed_at: None,
        };

        append_rotation_event(temp.path(), &transition).unwrap();
        let mut without_final_newline = fs::read(&path).unwrap();
        assert_eq!(without_final_newline.pop(), Some(b'\n'));
        atomic_replace_private(&path, &without_final_newline).unwrap();
        append_rotation_event(temp.path(), &transition).unwrap();

        let audit = fs::read_to_string(path).unwrap();
        assert_eq!(audit.lines().count(), 2);
        assert!(audit.contains("\"event\":\"device_renamed\""));
        assert_eq!(
            audit
                .matches("\"event\":\"device_authorization_reenrolled\"")
                .count(),
            1
        );
        assert!(audit.ends_with('\n'));
    }

    #[test]
    fn transition_retry_fails_closed_on_middle_corruption_and_preserves_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        append_event(
            temp.path(),
            now,
            MobileAuditEvent::DeviceRenamed,
            Some(Uuid::from_u128(1)),
        )
        .unwrap();
        let path = temp.path().join("mobile").join(AUDIT_FILE);
        let valid_tail = serde_json::to_string(&MobileAuditRecord {
            timestamp: now,
            event: MobileAuditEvent::DeviceRevoked,
            device_id: Some(Uuid::from_u128(4)),
            transition_id: None,
            source_device_id: None,
            replacement_device_id: None,
        })
        .unwrap();
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{{truncated").unwrap();
        writeln!(file, "{valid_tail}").unwrap();
        file.sync_data().unwrap();
        let before = fs::read(&path).unwrap();
        let transition = DeviceRotationAuditTransition {
            transition_id: Uuid::from_u128(10),
            source_device_id: Uuid::from_u128(2),
            replacement_device_id: Uuid::from_u128(3),
            source_grant_id: Uuid::from_u128(11),
            source_revocation_epoch: 1,
            replacement_grant_id: Uuid::from_u128(12),
            replacement_revocation_epoch: 1,
            occurred_at: now,
            audited_at: None,
            authorization_key_cleanup_completed_at: None,
        };

        assert!(append_rotation_event(temp.path(), &transition).is_err());
        assert_eq!(fs::read(path).unwrap(), before);
    }
}
