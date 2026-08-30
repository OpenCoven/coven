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
static AUDIT_WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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
}
