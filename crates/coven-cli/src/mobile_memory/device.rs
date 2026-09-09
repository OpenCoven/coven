use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::assurance::AssuranceClass;
use super::audit::{
    append_event, append_grant_reissue_event, append_rotation_event,
    DeviceGrantReissueAuditTransition, DeviceRotationAuditTransition, MobileAuditEvent,
};
use super::grant::{
    AssuranceLevel, DeviceGrantRestrictions, DeviceScope, GrantTransportConstraint,
};
use super::registry::{
    DeviceAuthorizationRecord, DeviceGrantReissueRequest, DeviceGrantRevision, DeviceRegistry,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceGrantPolicy {
    pub scopes: Vec<DeviceScope>,
    pub restrictions: DeviceGrantRestrictions,
    pub minimum_assurance: AssuranceLevel,
    pub expires_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceGrantPolicyIdentity<'a> {
    scopes: &'a [DeviceScope],
    restrictions: &'a DeviceGrantRestrictions,
    minimum_assurance: AssuranceLevel,
    lifetime_seconds: i64,
}

impl DeviceGrantPolicy {
    fn identity(&self, issued_at: DateTime<Utc>) -> Result<String> {
        let identity = DeviceGrantPolicyIdentity {
            scopes: &self.scopes,
            restrictions: &self.restrictions,
            minimum_assurance: self.minimum_assurance,
            lifetime_seconds: self
                .expires_at
                .signed_duration_since(issued_at)
                .num_seconds(),
        };
        let canonical =
            serde_jcs::to_vec(&identity).context("failed to canonicalize device grant policy")?;
        Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(canonical)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceRevocationReason {
    Ordinary,
    Lost,
    SuspectedCompromise,
    Retired,
    Reenrolled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceLifecycleStatus {
    Active,
    Revoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceGrantStatus {
    Active,
    NotYetValid,
    Expired,
    Revoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceAuthorizationKeyStatus {
    Absent,
    Active,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAuthorizationKeyView {
    pub status: DeviceAuthorizationKeyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assurance_class: Option<AssuranceClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_epoch: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub id: Uuid,
    pub display_name: String,
    pub paired_at: DateTime<Utc>,
    pub status: DeviceLifecycleStatus,
    pub grant_status: DeviceGrantStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<DateTime<Utc>>,
    pub grant_id: Uuid,
    pub scopes: Vec<DeviceScope>,
    pub transport: GrantTransportConstraint,
    pub require_fresh_user_verification_for: Vec<DeviceScope>,
    pub minimum_assurance: AssuranceLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub revocation_epoch: u64,
    pub authorization_key: DeviceAuthorizationKeyView,
}

pub struct DeviceAuthority {
    coven_home: PathBuf,
    registry: DeviceRegistry,
}

impl DeviceAuthority {
    pub fn load(coven_home: &Path) -> Result<Self> {
        Ok(Self {
            coven_home: coven_home.to_path_buf(),
            registry: DeviceRegistry::load(coven_home)?,
        })
    }

    pub fn list(&self, now: DateTime<Utc>) -> Result<Vec<DeviceView>> {
        self.registry.reload()?;
        self.registry
            .list_status()?
            .into_iter()
            .map(|status| {
                let record = self
                    .registry
                    .authorization_record(status.id)?
                    .context("mobile device disappeared during status projection")?;
                self.project(record, now)
            })
            .collect()
    }

    pub fn inspect(&self, selector: &str, now: DateTime<Utc>) -> Result<DeviceView> {
        let record = self.resolve(selector)?;
        self.project(record, now)
    }

    pub fn rename(
        &self,
        selector: &str,
        display_name: &str,
        now: DateTime<Utc>,
    ) -> Result<DeviceView> {
        let record = self.resolve(selector)?;
        self.registry
            .rename(record.device.id, display_name.to_owned())?;
        append_event(
            &self.coven_home,
            now,
            MobileAuditEvent::DeviceRenamed,
            Some(record.device.id),
        )?;
        self.inspect(&record.device.id.to_string(), now)
    }

    pub fn revoke(
        &self,
        selector: &str,
        reason: DeviceRevocationReason,
        now: DateTime<Utc>,
    ) -> Result<DeviceView> {
        let record = self.resolve(selector)?;
        self.flush_pending_grant_reissue_audit(record.device.id, now)?;
        self.registry.revoke(record.device.id, now)?;
        append_event(
            &self.coven_home,
            now,
            reason.audit_event(),
            Some(record.device.id),
        )?;
        self.inspect(&record.device.id.to_string(), now)
    }

    pub fn reissue_grant(
        &self,
        selector: &str,
        policy: DeviceGrantPolicy,
        now: DateTime<Utc>,
    ) -> Result<DeviceView> {
        let requested_policy_digest = policy.identity(now)?;
        let mut record = self.resolve(selector)?;
        if let Some(pending) = self
            .registry
            .pending_grant_reissue_audit(record.device.id)?
        {
            let identical = pending.requested_policy_digest == requested_policy_digest;
            self.audit_grant_reissue(&pending, now)?;
            if identical {
                return self.inspect(&record.device.id.to_string(), now);
            }
            record = self.resolve(&record.device.id.to_string())?;
        }
        if record.device.revoked_at.is_some() {
            bail!("mobile device is revoked");
        }
        let result = self
            .registry
            .reissue_grant_atomically(DeviceGrantReissueRequest {
                expected: DeviceGrantRevision::from(&record),
                scopes: policy.scopes,
                restrictions: policy.restrictions,
                minimum_assurance: policy.minimum_assurance,
                expires_at: policy.expires_at,
                issued_at: now,
                requested_policy_digest,
            })?;
        self.audit_grant_reissue(&result.transition, now)?;
        self.inspect(&result.device.device.id.to_string(), now)
    }

    fn audit_grant_reissue(
        &self,
        transition: &DeviceGrantReissueAuditTransition,
        audited_at: DateTime<Utc>,
    ) -> Result<()> {
        let receipt = append_grant_reissue_event(&self.coven_home, transition)?;
        self.registry
            .mark_grant_reissue_audited(&receipt, audited_at)
    }

    pub fn rotate_device(
        &self,
        source_selector: &str,
        replacement_selector: &str,
        now: DateTime<Utc>,
    ) -> Result<DeviceView> {
        self.flush_pending_rotation_audits(now)?;
        let source = self.resolve(source_selector)?;
        let replacement = self.resolve(replacement_selector)?;
        self.flush_pending_grant_reissue_audit(source.device.id, now)?;
        self.flush_pending_grant_reissue_audit(replacement.device.id, now)?;
        let source = self.resolve(&source.device.id.to_string())?;
        let replacement = self.resolve(&replacement.device.id.to_string())?;
        let result = self.registry.rotate_devices_atomically(
            DeviceGrantRevision::from(&source),
            DeviceGrantRevision::from(&replacement),
            now,
        )?;
        self.audit_rotation(&result.transition, now)?;
        self.inspect(&result.replacement.device.id.to_string(), now)
    }

    fn flush_pending_grant_reissue_audit(
        &self,
        device_id: Uuid,
        audited_at: DateTime<Utc>,
    ) -> Result<bool> {
        let Some(transition) = self.registry.pending_grant_reissue_audit(device_id)? else {
            return Ok(false);
        };
        self.audit_grant_reissue(&transition, audited_at)?;
        Ok(true)
    }

    fn flush_pending_rotation_audits(&self, audited_at: DateTime<Utc>) -> Result<()> {
        for transition in self.registry.pending_rotation_audits()? {
            self.audit_rotation(&transition, audited_at)?;
        }
        Ok(())
    }

    fn audit_rotation(
        &self,
        transition: &DeviceRotationAuditTransition,
        audited_at: DateTime<Utc>,
    ) -> Result<()> {
        let receipt = append_rotation_event(&self.coven_home, transition)?;
        self.registry.mark_rotation_audited(&receipt, audited_at)
    }

    fn resolve(&self, selector: &str) -> Result<DeviceAuthorizationRecord> {
        self.registry.reload()?;
        if let Ok(id) = Uuid::parse_str(selector) {
            return self
                .registry
                .authorization_record(id)?
                .context("mobile device is not registered");
        }
        let matches: Vec<_> = self
            .registry
            .list_status()?
            .into_iter()
            .filter(|device| device.display_name == selector)
            .collect();
        match matches.as_slice() {
            [] => bail!("mobile device is not registered"),
            [device] => self
                .registry
                .authorization_record(device.id)?
                .context("mobile device disappeared during lookup"),
            _ => bail!("mobile device name is ambiguous; use its id"),
        }
    }

    fn project(&self, record: DeviceAuthorizationRecord, now: DateTime<Utc>) -> Result<DeviceView> {
        let active_key = self.registry.authorization_key(record.device.id)?;
        let authorization_key = if let Some(key) = active_key {
            DeviceAuthorizationKeyView {
                status: DeviceAuthorizationKeyStatus::Active,
                assurance_class: Some(key.assurance_class),
                key_epoch: Some(key.key_epoch),
            }
        } else {
            let latest = self
                .registry
                .authorization_key_history(record.device.id)?
                .into_iter()
                .max_by_key(|key| key.key_epoch);
            DeviceAuthorizationKeyView {
                status: if latest.is_some() {
                    DeviceAuthorizationKeyStatus::Revoked
                } else {
                    DeviceAuthorizationKeyStatus::Absent
                },
                assurance_class: latest.as_ref().map(|key| key.assurance_class),
                key_epoch: latest.map(|key| key.key_epoch),
            }
        };
        let status = if record.device.revoked_at.is_some() {
            DeviceLifecycleStatus::Revoked
        } else {
            DeviceLifecycleStatus::Active
        };
        let grant_status = if status == DeviceLifecycleStatus::Revoked {
            DeviceGrantStatus::Revoked
        } else if now < record.grant.not_before {
            DeviceGrantStatus::NotYetValid
        } else if record
            .grant
            .expires_at
            .is_some_and(|expires_at| now >= expires_at)
        {
            DeviceGrantStatus::Expired
        } else {
            DeviceGrantStatus::Active
        };
        Ok(DeviceView {
            id: record.device.id,
            display_name: record.device.display_name,
            paired_at: record.device.paired_at,
            status,
            grant_status,
            revoked_at: record.device.revoked_at,
            grant_id: record.grant.id,
            scopes: record.grant.scopes,
            transport: record.grant.restrictions.transport,
            require_fresh_user_verification_for: record
                .grant
                .restrictions
                .require_fresh_user_verification_for,
            minimum_assurance: record.grant.minimum_assurance,
            expires_at: record.grant.expires_at,
            revocation_epoch: record.grant.revocation_epoch,
            authorization_key,
        })
    }
}

impl DeviceRevocationReason {
    fn audit_event(self) -> MobileAuditEvent {
        match self {
            Self::Lost => MobileAuditEvent::DeviceLostRevoked,
            Self::SuspectedCompromise => MobileAuditEvent::DeviceCompromiseRevoked,
            Self::Reenrolled => MobileAuditEvent::DeviceAuthorizationReenrolled,
            Self::Ordinary | Self::Retired => MobileAuditEvent::DeviceRevoked,
        }
    }
}

pub fn run_list(json: bool) -> Result<()> {
    let authority = DeviceAuthority::load(&crate::coven_home_dir()?)?;
    let devices = authority.list(Utc::now())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&devices)?);
    } else if devices.is_empty() {
        println!("No devices are enrolled.");
    } else {
        for device in devices {
            println!(
                "{}\t{:?}\t{:?}\t{}",
                device.id, device.status, device.grant_status, device.display_name
            );
        }
    }
    Ok(())
}

pub fn run_inspect(selector: &str, json: bool) -> Result<()> {
    let authority = DeviceAuthority::load(&crate::coven_home_dir()?)?;
    let device = authority.inspect(selector, Utc::now())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&device)?);
    } else {
        print_device(&device);
    }
    Ok(())
}

pub fn run_rename(selector: &str, display_name: &str) -> Result<()> {
    let authority = DeviceAuthority::load(&crate::coven_home_dir()?)?;
    let device = authority.rename(selector, display_name, Utc::now())?;
    println!("Renamed device {} to {}.", device.id, device.display_name);
    Ok(())
}

pub fn run_revoke(selector: &str, reason: DeviceRevocationReason) -> Result<()> {
    let authority = DeviceAuthority::load(&crate::coven_home_dir()?)?;
    let device = authority.revoke(selector, reason, Utc::now())?;
    println!("Revoked device {}.", device.id);
    Ok(())
}

pub fn run_reissue_grant(
    selector: &str,
    scopes: Vec<DeviceScope>,
    minimum_assurance: AssuranceLevel,
    require_fresh_user_verification_for: Vec<DeviceScope>,
    expires_in_days: u16,
    direct_only: bool,
) -> Result<()> {
    let authority = DeviceAuthority::load(&crate::coven_home_dir()?)?;
    let now = Utc::now();
    let device = authority.reissue_grant(
        selector,
        DeviceGrantPolicy {
            scopes,
            restrictions: DeviceGrantRestrictions {
                transport: if direct_only {
                    GrantTransportConstraint::DirectOnly
                } else {
                    GrantTransportConstraint::AnyAuthenticated
                },
                require_fresh_user_verification_for,
            },
            minimum_assurance,
            expires_at: now + chrono::Duration::days(i64::from(expires_in_days)),
        },
        now,
    )?;
    println!(
        "Reissued grant {} for device {} at revocation epoch {}.",
        device.grant_id, device.id, device.revocation_epoch
    );
    Ok(())
}

pub fn run_rotate(source_selector: &str, replacement_selector: &str) -> Result<()> {
    let authority = DeviceAuthority::load(&crate::coven_home_dir()?)?;
    let replacement = authority.rotate_device(source_selector, replacement_selector, Utc::now())?;
    println!(
        "Re-enrolled authority on replacement device {} with grant {}.",
        replacement.id, replacement.grant_id
    );
    Ok(())
}

fn print_device(device: &DeviceView) {
    println!("Device: {} ({})", device.display_name, device.id);
    println!("Status: {:?}", device.status);
    println!("Grant status: {:?}", device.grant_status);
    println!("Grant id: {}", device.grant_id);
    println!(
        "Scopes: {}",
        device
            .scopes
            .iter()
            .map(|scope| scope.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("Minimum assurance: {:?}", device.minimum_assurance);
    println!(
        "Fresh verification scopes: {}",
        device
            .require_fresh_user_verification_for
            .iter()
            .map(|scope| scope.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("Transport: {:?}", device.transport);
    println!(
        "Expires: {}",
        device
            .expires_at
            .map(|expires_at| expires_at.to_rfc3339())
            .unwrap_or_else(|| "unbounded legacy grant".to_owned())
    );
    println!("Revocation epoch: {}", device.revocation_epoch);
    println!(
        "Authorization key: {:?}{}",
        device.authorization_key.status,
        device
            .authorization_key
            .assurance_class
            .map(|class| format!(" ({})", class.as_str()))
            .unwrap_or_default()
    );
}

#[cfg(test)]
mod tests {
    use std::fs;

    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use chrono::{DateTime, Duration, Utc};
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    use uuid::Uuid;

    use super::*;
    use crate::mobile_memory::assurance::{AssuranceClass, NewAuthorizationKey};
    use crate::mobile_memory::config::atomic_replace_private;
    use crate::mobile_memory::grant::{
        AssuranceLevel, DeviceGrant, DeviceGrantRestrictions, DeviceScope, GrantTransportConstraint,
    };
    use crate::mobile_memory::registry::{DeviceRecord, DeviceRegistry};

    fn public_key(seed: u8) -> String {
        let signing_key = p256::SecretKey::from_slice(&[seed; 32]).unwrap();
        URL_SAFE_NO_PAD.encode(signing_key.public_key().to_encoded_point(false).as_bytes())
    }

    fn device(id: Uuid, name: &str, key_seed: u8, now: DateTime<Utc>) -> DeviceRecord {
        DeviceRecord {
            id,
            display_name: name.to_owned(),
            public_key_x963: public_key(key_seed),
            paired_at: now,
            revoked_at: None,
            scopes: vec![DeviceScope::MemoryRead],
        }
    }

    fn register_device(
        registry: &DeviceRegistry,
        record: DeviceRecord,
        authorization_seed: Option<u8>,
    ) {
        register_device_with_authorization(
            registry,
            record,
            authorization_seed.map(|seed| (seed, AssuranceClass::BiometricOnly)),
        );
    }

    fn register_device_with_authorization(
        registry: &DeviceRegistry,
        record: DeviceRecord,
        authorization: Option<(u8, AssuranceClass)>,
    ) {
        let grant = DeviceGrant::for_device(
            record.id,
            &record.public_key_x963,
            record.scopes.clone(),
            record.paired_at,
        )
        .unwrap();
        registry
            .register_with_grant_and_authorization(
                record.clone(),
                grant,
                authorization.map(|(seed, assurance_class)| NewAuthorizationKey {
                    public_key_x963: public_key(seed),
                    assurance_class,
                    enrolled_at: record.paired_at,
                }),
            )
            .unwrap();
    }

    fn policy(now: DateTime<Utc>) -> DeviceGrantPolicy {
        DeviceGrantPolicy {
            scopes: vec![
                DeviceScope::MemoryRead,
                DeviceScope::SessionMetadataRead,
                DeviceScope::ToolExecutionApprove,
            ],
            restrictions: DeviceGrantRestrictions {
                transport: GrantTransportConstraint::DirectOnly,
                require_fresh_user_verification_for: vec![DeviceScope::ToolExecutionApprove],
            },
            minimum_assurance: AssuranceLevel::RecentUserVerification,
            expires_at: now + Duration::days(30),
        }
    }

    #[test]
    fn grant_reissue_creates_fresh_identity_and_advances_revocation_state() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let id = Uuid::from_u128(1);
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(&registry, device(id, "Primary phone", 1, now), Some(2));
        let before = registry.authorization_record(id).unwrap().unwrap();
        let before_key = registry.authorization_key(id).unwrap().unwrap();

        let authority = DeviceAuthority::load(temp.path()).unwrap();
        let updated = authority
            .reissue_grant("Primary phone", policy(now), now)
            .unwrap();

        assert_ne!(updated.grant_id, before.grant.id);
        assert_eq!(updated.revocation_epoch, before.grant.revocation_epoch + 1);
        assert_eq!(updated.scopes, policy(now).scopes);
        assert_eq!(
            updated.require_fresh_user_verification_for,
            policy(now).restrictions.require_fresh_user_verification_for
        );
        assert_eq!(updated.minimum_assurance, policy(now).minimum_assurance);
        assert_eq!(updated.expires_at, Some(policy(now).expires_at));
        let after_key = authority.registry.authorization_key(id).unwrap().unwrap();
        assert_eq!(after_key.subject_key_id, before_key.subject_key_id);
        assert_eq!(after_key.key_epoch, before_key.key_epoch);
    }

    #[test]
    fn grant_reissue_rejects_noncanonical_restrictions_and_bad_expiry_without_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let id = Uuid::from_u128(1);
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(&registry, device(id, "Primary phone", 1, now), None);
        let authority = DeviceAuthority::load(temp.path()).unwrap();
        let original = authority
            .registry
            .authorization_record(id)
            .unwrap()
            .unwrap()
            .grant;

        let mut invalid = policy(now);
        invalid.scopes.swap(0, 1);
        assert!(authority
            .reissue_grant("Primary phone", invalid, now)
            .is_err());

        let mut invalid = policy(now);
        invalid.scopes.push(DeviceScope::ToolExecutionApprove);
        assert!(authority
            .reissue_grant("Primary phone", invalid, now)
            .is_err());

        let mut invalid = policy(now);
        invalid.restrictions.require_fresh_user_verification_for = vec![DeviceScope::SecretsRead];
        assert!(authority
            .reissue_grant("Primary phone", invalid, now)
            .is_err());

        let mut invalid = policy(now);
        invalid.expires_at = now;
        assert!(authority
            .reissue_grant("Primary phone", invalid, now)
            .is_err());

        let mut invalid = policy(now);
        invalid.expires_at = now + Duration::days(366);
        assert!(authority
            .reissue_grant("Primary phone", invalid, now)
            .is_err());

        let after = authority
            .registry
            .authorization_record(id)
            .unwrap()
            .unwrap()
            .grant;
        assert_eq!(after.id, original.id);
        assert_eq!(after.revocation_epoch, original.revocation_epoch);
    }

    #[test]
    fn broadened_grant_audit_failure_persists_outbox_and_retry_does_not_reissue() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let id = Uuid::from_u128(1);
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(&registry, device(id, "Primary phone", 1, now), None);
        let authority = DeviceAuthority::load(temp.path()).unwrap();
        let broadened = DeviceGrantPolicy {
            scopes: vec![DeviceScope::MemoryRead, DeviceScope::MessageSend],
            restrictions: DeviceGrantRestrictions::default(),
            minimum_assurance: AssuranceLevel::Possession,
            expires_at: now + Duration::days(30),
        };
        let audit_path = temp.path().join("mobile/audit.jsonl");
        fs::create_dir(&audit_path).unwrap();

        assert!(authority
            .reissue_grant("Primary phone", broadened.clone(), now)
            .is_err());

        let persisted = authority
            .registry
            .authorization_record(id)
            .unwrap()
            .unwrap()
            .grant;
        assert_eq!(
            persisted.scopes,
            [DeviceScope::MemoryRead, DeviceScope::MessageSend]
        );
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("mobile/devices.json")).unwrap())
                .unwrap();
        let transition = &stored["grantReissueTransitions"][0];
        assert_eq!(transition["deviceId"], id.to_string());
        assert_eq!(transition["replacementGrantId"], persisted.id.to_string());
        assert_eq!(
            transition["replacementRevocationEpoch"],
            persisted.revocation_epoch
        );
        assert!(transition["auditedAt"].is_null());

        fs::remove_dir(&audit_path).unwrap();
        let retry_at = now + Duration::seconds(1);
        let mut identical_retry = broadened;
        identical_retry.expires_at = retry_at + Duration::days(30);
        let retried = authority
            .reissue_grant("Primary phone", identical_retry, retry_at)
            .unwrap();
        assert_eq!(retried.grant_id, persisted.id);
        assert_eq!(retried.revocation_epoch, persisted.revocation_epoch);

        let audit = fs::read_to_string(&audit_path).unwrap();
        assert_eq!(
            audit.matches("\"event\":\"device_grant_reissued\"").count(),
            1
        );
        assert!(audit.contains(&format!("\"deviceId\":\"{id}\"")));
        assert!(!audit.contains("scope"));
        assert!(!audit.contains("publicKey"));
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("mobile/devices.json")).unwrap())
                .unwrap();
        assert!(stored["grantReissueTransitions"][0]["auditedAt"].is_string());
    }

    #[test]
    fn different_policy_after_pending_broadened_reissue_audits_then_reissues_restrictively() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let id = Uuid::from_u128(1);
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(&registry, device(id, "Primary phone", 1, now), None);
        let authority = DeviceAuthority::load(temp.path()).unwrap();
        let broadened = DeviceGrantPolicy {
            scopes: vec![DeviceScope::MemoryRead, DeviceScope::MessageSend],
            restrictions: DeviceGrantRestrictions::default(),
            minimum_assurance: AssuranceLevel::Possession,
            expires_at: now + Duration::days(30),
        };
        let restrictive = DeviceGrantPolicy {
            scopes: vec![DeviceScope::MemoryRead],
            restrictions: DeviceGrantRestrictions::default(),
            minimum_assurance: AssuranceLevel::Possession,
            expires_at: now + Duration::days(7),
        };
        let audit_path = temp.path().join("mobile/audit.jsonl");
        fs::create_dir(&audit_path).unwrap();

        assert!(authority
            .reissue_grant("Primary phone", broadened, now)
            .is_err());
        let broader = authority
            .registry
            .authorization_record(id)
            .unwrap()
            .unwrap()
            .grant;
        assert_eq!(
            broader.scopes,
            [DeviceScope::MemoryRead, DeviceScope::MessageSend]
        );

        fs::remove_dir(&audit_path).unwrap();
        let narrowed = authority
            .reissue_grant("Primary phone", restrictive, now + Duration::seconds(1))
            .unwrap();

        assert_eq!(narrowed.scopes, [DeviceScope::MemoryRead]);
        assert_ne!(narrowed.grant_id, broader.id);
        assert_eq!(narrowed.revocation_epoch, broader.revocation_epoch + 1);
        let audit = fs::read_to_string(&audit_path).unwrap();
        assert_eq!(
            audit.matches("\"event\":\"device_grant_reissued\"").count(),
            2
        );
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("mobile/devices.json")).unwrap())
                .unwrap();
        let transitions = stored["grantReissueTransitions"].as_array().unwrap();
        assert_eq!(transitions.len(), 2);
        assert!(transitions
            .iter()
            .all(|transition| transition["auditedAt"].is_string()));
        assert_ne!(
            transitions[0]["requestedPolicyDigest"],
            transitions[1]["requestedPolicyDigest"]
        );
    }

    #[test]
    fn rotation_transfers_policy_to_enrolled_replacement_and_revokes_source() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let old_id = Uuid::from_u128(1);
        let replacement_id = Uuid::from_u128(2);
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(&registry, device(old_id, "Old phone", 1, now), Some(3));
        register_device(
            &registry,
            device(replacement_id, "Replacement phone", 2, now),
            Some(4),
        );
        let authority = DeviceAuthority::load(temp.path()).unwrap();
        let old_policy = authority
            .reissue_grant("Old phone", policy(now), now)
            .unwrap();
        let replacement_before = authority
            .registry
            .authorization_record(replacement_id)
            .unwrap()
            .unwrap()
            .grant;

        let rotated = authority
            .rotate_device("Old phone", "Replacement phone", now + Duration::seconds(1))
            .unwrap();

        assert_eq!(rotated.scopes, old_policy.scopes);
        assert_eq!(
            rotated.require_fresh_user_verification_for,
            old_policy.require_fresh_user_verification_for
        );
        assert_eq!(rotated.minimum_assurance, old_policy.minimum_assurance);
        assert_eq!(rotated.expires_at, old_policy.expires_at);
        assert_ne!(rotated.grant_id, replacement_before.id);
        assert_eq!(
            rotated.revocation_epoch,
            replacement_before.revocation_epoch + 1
        );
        let source = authority.inspect("Old phone", now).unwrap();
        assert_eq!(source.status, DeviceLifecycleStatus::Revoked);
    }

    #[test]
    fn atomic_rotation_rejects_a_source_policy_changed_after_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let old_id = Uuid::from_u128(1);
        let replacement_id = Uuid::from_u128(2);
        let stale = DeviceRegistry::load(temp.path()).unwrap();
        register_device(&stale, device(old_id, "Old phone", 1, now), None);
        register_device(
            &stale,
            device(replacement_id, "Replacement phone", 2, now),
            None,
        );
        let source_snapshot = stale.authorization_record(old_id).unwrap().unwrap();
        let replacement_snapshot = stale.authorization_record(replacement_id).unwrap().unwrap();

        let concurrent = DeviceAuthority::load(temp.path()).unwrap();
        concurrent
            .reissue_grant(
                "Old phone",
                DeviceGrantPolicy {
                    scopes: vec![DeviceScope::MemoryRead],
                    restrictions: DeviceGrantRestrictions::default(),
                    minimum_assurance: AssuranceLevel::Possession,
                    expires_at: now + Duration::days(7),
                },
                now,
            )
            .unwrap();

        assert!(stale
            .rotate_devices_atomically(
                DeviceGrantRevision::from(&source_snapshot),
                DeviceGrantRevision::from(&replacement_snapshot),
                now + Duration::seconds(1),
            )
            .is_err());
        stale.reload().unwrap();
        let source = stale.authorization_record(old_id).unwrap().unwrap();
        assert_eq!(source.grant.scopes, [DeviceScope::MemoryRead]);
        assert!(source.device.revoked_at.is_none());
        let replacement = stale.authorization_record(replacement_id).unwrap().unwrap();
        assert_eq!(replacement.grant.id, replacement_snapshot.grant.id);
        assert_eq!(
            replacement.grant.revocation_epoch,
            replacement_snapshot.grant.revocation_epoch
        );
    }

    #[test]
    fn atomic_rotation_rejects_a_replacement_grant_changed_after_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let old_id = Uuid::from_u128(1);
        let replacement_id = Uuid::from_u128(2);
        let stale = DeviceRegistry::load(temp.path()).unwrap();
        register_device(&stale, device(old_id, "Old phone", 1, now), None);
        register_device(
            &stale,
            device(replacement_id, "Replacement phone", 2, now),
            None,
        );
        let source_snapshot = stale.authorization_record(old_id).unwrap().unwrap();
        let replacement_snapshot = stale.authorization_record(replacement_id).unwrap().unwrap();

        let concurrent = DeviceAuthority::load(temp.path()).unwrap();
        concurrent
            .reissue_grant(
                "Replacement phone",
                DeviceGrantPolicy {
                    scopes: vec![DeviceScope::MemoryRead],
                    restrictions: DeviceGrantRestrictions::default(),
                    minimum_assurance: AssuranceLevel::Possession,
                    expires_at: now + Duration::days(7),
                },
                now,
            )
            .unwrap();

        assert!(stale
            .rotate_devices_atomically(
                DeviceGrantRevision::from(&source_snapshot),
                DeviceGrantRevision::from(&replacement_snapshot),
                now + Duration::seconds(1),
            )
            .is_err());
        stale.reload().unwrap();
        assert!(stale
            .authorization_record(old_id)
            .unwrap()
            .unwrap()
            .device
            .revoked_at
            .is_none());
        let replacement = stale.authorization_record(replacement_id).unwrap().unwrap();
        assert_ne!(replacement.grant.id, replacement_snapshot.grant.id);
        assert_eq!(replacement.grant.scopes, [DeviceScope::MemoryRead]);
    }

    #[test]
    fn rotation_rejects_same_unknown_or_revoked_replacement_without_widening_authority() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let old_id = Uuid::from_u128(1);
        let replacement_id = Uuid::from_u128(2);
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(&registry, device(old_id, "Old phone", 1, now), None);
        register_device(
            &registry,
            device(replacement_id, "Replacement phone", 2, now),
            None,
        );
        let authority = DeviceAuthority::load(temp.path()).unwrap();
        let old_before = authority.inspect("Old phone", now).unwrap();
        let replacement_before = authority.inspect("Replacement phone", now).unwrap();

        assert!(authority
            .rotate_device("Old phone", "Old phone", now)
            .is_err());
        assert!(authority
            .rotate_device("Old phone", "Unknown phone", now)
            .is_err());
        authority
            .revoke("Replacement phone", DeviceRevocationReason::Retired, now)
            .unwrap();
        assert!(authority
            .rotate_device("Old phone", "Replacement phone", now)
            .is_err());

        let old_after = authority.inspect("Old phone", now).unwrap();
        assert_eq!(old_after.grant_id, old_before.grant_id);
        assert_eq!(old_after.status, DeviceLifecycleStatus::Active);
        let replacement_after = authority.inspect("Replacement phone", now).unwrap();
        assert_eq!(replacement_after.grant_id, replacement_before.grant_id);
        assert_eq!(replacement_after.status, DeviceLifecycleStatus::Revoked);
    }

    #[test]
    fn rotation_preflight_rejects_replacement_that_cannot_satisfy_transferred_assurance() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(
            &registry,
            device(Uuid::from_u128(1), "Biometric source", 1, now),
            Some(4),
        );
        register_device_with_authorization(
            &registry,
            device(Uuid::from_u128(2), "User verification replacement", 2, now),
            Some((5, AssuranceClass::UserVerification)),
        );
        register_device(
            &registry,
            device(Uuid::from_u128(3), "Fresh restriction source", 3, now),
            Some(6),
        );
        register_device(
            &registry,
            device(Uuid::from_u128(4), "Possession-only replacement", 7, now),
            None,
        );
        let authority = DeviceAuthority::load(temp.path()).unwrap();

        let mut biometric_policy = policy(now);
        biometric_policy.minimum_assurance = AssuranceLevel::FreshBiometric;
        authority
            .reissue_grant("Biometric source", biometric_policy, now)
            .unwrap();
        let biometric_source_before = authority.inspect("Biometric source", now).unwrap();
        let user_replacement_before = authority
            .inspect("User verification replacement", now)
            .unwrap();
        assert!(authority
            .rotate_device(
                "Biometric source",
                "User verification replacement",
                now + Duration::seconds(1),
            )
            .is_err());
        assert_eq!(
            authority.inspect("Biometric source", now).unwrap(),
            biometric_source_before
        );
        assert_eq!(
            authority
                .inspect("User verification replacement", now)
                .unwrap(),
            user_replacement_before
        );

        let mut fresh_policy = policy(now);
        fresh_policy.minimum_assurance = AssuranceLevel::Possession;
        authority
            .reissue_grant("Fresh restriction source", fresh_policy, now)
            .unwrap();
        let fresh_source_before = authority.inspect("Fresh restriction source", now).unwrap();
        let no_key_before = authority
            .inspect("Possession-only replacement", now)
            .unwrap();
        assert!(authority
            .rotate_device(
                "Fresh restriction source",
                "Possession-only replacement",
                now + Duration::seconds(1),
            )
            .is_err());
        assert_eq!(
            authority.inspect("Fresh restriction source", now).unwrap(),
            fresh_source_before
        );
        assert_eq!(
            authority
                .inspect("Possession-only replacement", now)
                .unwrap(),
            no_key_before
        );
    }

    #[test]
    fn possession_only_rotation_allows_replacement_without_step_up_key() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(
            &registry,
            device(Uuid::from_u128(1), "Possession source", 1, now),
            None,
        );
        register_device(
            &registry,
            device(Uuid::from_u128(2), "Possession replacement", 2, now),
            None,
        );
        let authority = DeviceAuthority::load(temp.path()).unwrap();

        authority
            .rotate_device("Possession source", "Possession replacement", now)
            .unwrap();

        assert_eq!(
            authority.inspect("Possession source", now).unwrap().status,
            DeviceLifecycleStatus::Revoked
        );
        assert_eq!(
            authority
                .inspect("Possession replacement", now)
                .unwrap()
                .minimum_assurance,
            AssuranceLevel::Possession
        );
    }

    #[test]
    fn rotation_authorization_store_failure_mutates_neither_device() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let old_id = Uuid::from_u128(1);
        let replacement_id = Uuid::from_u128(2);
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(&registry, device(old_id, "Old phone", 1, now), Some(3));
        register_device(
            &registry,
            device(replacement_id, "Replacement phone", 2, now),
            Some(4),
        );
        let authority = DeviceAuthority::load(temp.path()).unwrap();
        authority
            .reissue_grant("Old phone", policy(now), now)
            .unwrap();
        let source_before = authority
            .registry
            .authorization_record(old_id)
            .unwrap()
            .unwrap()
            .grant;
        let replacement_before = authority.inspect("Replacement phone", now).unwrap();
        let authorization_path = temp
            .path()
            .join("mobile")
            .join(crate::mobile_memory::assurance::AUTHORIZATION_KEYS_FILE);
        atomic_replace_private(&authorization_path, b"{not valid json}\n").unwrap();

        assert!(authority
            .rotate_device("Old phone", "Replacement phone", now)
            .is_err());

        let old = authority
            .registry
            .authorization_record(old_id)
            .unwrap()
            .unwrap();
        assert!(old.device.revoked_at.is_none());
        assert_eq!(old.grant.id, source_before.id);
        assert_eq!(old.grant.revocation_epoch, source_before.revocation_epoch);
        let replacement = authority
            .registry
            .authorization_record(replacement_id)
            .unwrap()
            .unwrap();
        assert_eq!(replacement.grant.id, replacement_before.grant_id);
        assert_eq!(
            replacement.grant.revocation_epoch,
            replacement_before.revocation_epoch
        );
    }

    #[test]
    fn pending_rotation_retry_completes_authorization_key_cleanup_after_secondary_store_failure() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let source_id = Uuid::from_u128(1);
        let replacement_id = Uuid::from_u128(2);
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(&registry, device(source_id, "Old phone", 1, now), Some(3));
        register_device(
            &registry,
            device(replacement_id, "Replacement phone", 2, now),
            None,
        );
        let authority = DeviceAuthority::load(temp.path()).unwrap();
        let authorization_path = temp
            .path()
            .join("mobile")
            .join(crate::mobile_memory::assurance::AUTHORIZATION_KEYS_FILE);
        let authorization_bytes = fs::read(&authorization_path).unwrap();
        fs::remove_file(&authorization_path).unwrap();
        fs::create_dir(&authorization_path).unwrap();

        assert!(authority
            .rotate_device("Old phone", "Replacement phone", now)
            .is_err());

        let source = authority
            .registry
            .authorization_record(source_id)
            .unwrap()
            .unwrap();
        assert!(source.device.revoked_at.is_some());
        let replacement_after_first = authority
            .registry
            .authorization_record(replacement_id)
            .unwrap()
            .unwrap()
            .grant;
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("mobile/devices.json")).unwrap())
                .unwrap();
        assert!(stored["rotationTransitions"][0]["auditedAt"].is_null());

        fs::remove_dir(&authorization_path).unwrap();
        atomic_replace_private(&authorization_path, &authorization_bytes).unwrap();
        let retried = authority
            .rotate_device("Old phone", "Replacement phone", now)
            .unwrap();

        assert_eq!(retried.grant_id, replacement_after_first.id);
        assert_eq!(
            retried.revocation_epoch,
            replacement_after_first.revocation_epoch
        );
        assert!(authority
            .registry
            .authorization_key(source_id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn pending_rotation_cleanup_survives_later_replacement_reissue_without_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let source_id = Uuid::from_u128(1);
        let replacement_id = Uuid::from_u128(2);
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(&registry, device(source_id, "Old phone", 1, now), Some(3));
        register_device(
            &registry,
            device(replacement_id, "Replacement phone", 2, now),
            None,
        );
        let authority = DeviceAuthority::load(temp.path()).unwrap();
        let authorization_path = temp
            .path()
            .join("mobile")
            .join(crate::mobile_memory::assurance::AUTHORIZATION_KEYS_FILE);
        let authorization_bytes = fs::read(&authorization_path).unwrap();
        fs::remove_file(&authorization_path).unwrap();
        fs::create_dir(&authorization_path).unwrap();
        assert!(authority
            .rotate_device("Old phone", "Replacement phone", now)
            .is_err());
        fs::remove_dir(&authorization_path).unwrap();
        atomic_replace_private(&authorization_path, &authorization_bytes).unwrap();

        let reissued = authority
            .reissue_grant(
                "Replacement phone",
                DeviceGrantPolicy {
                    scopes: vec![DeviceScope::MemoryRead],
                    restrictions: DeviceGrantRestrictions::default(),
                    minimum_assurance: AssuranceLevel::Possession,
                    expires_at: now + Duration::days(7),
                },
                now + Duration::seconds(1),
            )
            .unwrap();
        let retried = authority
            .rotate_device("Old phone", "Replacement phone", now + Duration::seconds(2))
            .unwrap();

        assert_eq!(retried.grant_id, reissued.grant_id);
        assert_eq!(retried.revocation_epoch, reissued.revocation_epoch);
        assert_eq!(retried.scopes, reissued.scopes);
        assert!(authority
            .registry
            .authorization_key(source_id)
            .unwrap()
            .is_none());
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("mobile/devices.json")).unwrap())
                .unwrap();
        assert_eq!(stored["rotationTransitions"].as_array().unwrap().len(), 1);
        assert!(stored["rotationTransitions"][0]["authorizationKeyCleanupCompletedAt"].is_string());
    }

    #[test]
    fn pending_rotation_cleanup_survives_later_replacement_revocation_without_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let source_id = Uuid::from_u128(1);
        let replacement_id = Uuid::from_u128(2);
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(&registry, device(source_id, "Old phone", 1, now), Some(3));
        register_device(
            &registry,
            device(replacement_id, "Replacement phone", 2, now),
            None,
        );
        let authority = DeviceAuthority::load(temp.path()).unwrap();
        let authorization_path = temp
            .path()
            .join("mobile")
            .join(crate::mobile_memory::assurance::AUTHORIZATION_KEYS_FILE);
        let authorization_bytes = fs::read(&authorization_path).unwrap();
        fs::remove_file(&authorization_path).unwrap();
        fs::create_dir(&authorization_path).unwrap();
        assert!(authority
            .rotate_device("Old phone", "Replacement phone", now)
            .is_err());
        fs::remove_dir(&authorization_path).unwrap();
        atomic_replace_private(&authorization_path, &authorization_bytes).unwrap();

        let revoked = authority
            .revoke(
                "Replacement phone",
                DeviceRevocationReason::Retired,
                now + Duration::seconds(1),
            )
            .unwrap();
        let retried = authority
            .rotate_device("Old phone", "Replacement phone", now + Duration::seconds(2))
            .unwrap();

        assert_eq!(retried.grant_id, revoked.grant_id);
        assert_eq!(retried.revocation_epoch, revoked.revocation_epoch);
        assert_eq!(retried.status, DeviceLifecycleStatus::Revoked);
        assert!(authority
            .registry
            .authorization_key(source_id)
            .unwrap()
            .is_none());
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("mobile/devices.json")).unwrap())
                .unwrap();
        assert_eq!(stored["rotationTransitions"].as_array().unwrap().len(), 1);
        assert!(stored["rotationTransitions"][0]["authorizationKeyCleanupCompletedAt"].is_string());
    }

    #[test]
    fn rotation_audit_failure_is_durable_and_retry_converges_without_rotating_twice() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let source_id = Uuid::from_u128(1);
        let replacement_id = Uuid::from_u128(2);
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(&registry, device(source_id, "Old phone", 1, now), None);
        register_device(
            &registry,
            device(replacement_id, "Replacement phone", 2, now),
            None,
        );
        let authority = DeviceAuthority::load(temp.path()).unwrap();
        let audit_path = temp.path().join("mobile/audit.jsonl");
        fs::create_dir(&audit_path).unwrap();

        assert!(authority
            .rotate_device("Old phone", "Replacement phone", now)
            .is_err());

        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("mobile/devices.json")).unwrap())
                .unwrap();
        let transition = &stored["rotationTransitions"][0];
        assert_eq!(transition["sourceDeviceId"], source_id.to_string());
        assert_eq!(
            transition["replacementDeviceId"],
            replacement_id.to_string()
        );
        assert!(transition["auditedAt"].is_null());
        let replacement_after_first = authority
            .registry
            .authorization_record(replacement_id)
            .unwrap()
            .unwrap()
            .grant;

        fs::remove_dir(&audit_path).unwrap();
        let retried = authority
            .rotate_device("Old phone", "Replacement phone", now)
            .unwrap();
        assert_eq!(retried.grant_id, replacement_after_first.id);
        assert_eq!(
            retried.revocation_epoch,
            replacement_after_first.revocation_epoch
        );

        let audit = fs::read_to_string(&audit_path).unwrap();
        assert_eq!(
            audit
                .matches("\"event\":\"device_authorization_reenrolled\"")
                .count(),
            1
        );
        assert!(audit.contains(&format!("\"sourceDeviceId\":\"{source_id}\"")));
        assert!(audit.contains(&format!("\"replacementDeviceId\":\"{replacement_id}\"")));
        assert!(!audit.contains("publicKey"));
        assert!(!audit.contains("subjectKey"));
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("mobile/devices.json")).unwrap())
                .unwrap();
        assert!(stored["rotationTransitions"][0]["auditedAt"].is_string());
    }

    #[test]
    fn privacy_safe_views_include_policy_and_authorization_key_class_without_identifiers() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let id = Uuid::from_u128(1);
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(&registry, device(id, "Primary phone", 1, now), Some(2));
        let authority = DeviceAuthority::load(temp.path()).unwrap();
        authority
            .reissue_grant("Primary phone", policy(now), now)
            .unwrap();

        let list_json = serde_json::to_string(&authority.list(now).unwrap()).unwrap();
        let inspect_json =
            serde_json::to_string(&authority.inspect("Primary phone", now).unwrap()).unwrap();
        for json in [&list_json, &inspect_json] {
            for forbidden in [
                "publicKey",
                "public_key",
                "subjectKey",
                "subject_key",
                "signature",
                "nonce",
                &public_key(1),
                &public_key(2),
            ] {
                assert!(!json.contains(forbidden), "privacy leak: {forbidden}");
            }
        }
        assert!(inspect_json.contains("\"authorizationKey\""));
        assert!(inspect_json.contains("\"status\":\"active\""));
        assert!(inspect_json.contains("\"assuranceClass\":\"biometric_only\""));
        assert!(inspect_json.contains("\"keyEpoch\":1"));
        assert!(inspect_json.contains("\"minimumAssurance\":\"recent_user_verification\""));
        assert!(inspect_json.contains("\"revocationEpoch\":1"));
    }

    #[test]
    fn management_actions_emit_only_coarse_audit_events() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_788_950_400, 0).unwrap();
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        register_device(
            &registry,
            device(Uuid::from_u128(1), "Lost phone", 1, now),
            None,
        );
        register_device(
            &registry,
            device(Uuid::from_u128(2), "Compromised phone", 2, now),
            None,
        );
        register_device(
            &registry,
            device(Uuid::from_u128(3), "Ordinary phone", 3, now),
            None,
        );
        let authority = DeviceAuthority::load(temp.path()).unwrap();
        authority
            .rename("Ordinary phone", "Renamed phone", now)
            .unwrap();
        authority
            .reissue_grant("Renamed phone", policy(now), now)
            .unwrap();
        authority
            .revoke("Lost phone", DeviceRevocationReason::Lost, now)
            .unwrap();
        authority
            .revoke(
                "Compromised phone",
                DeviceRevocationReason::SuspectedCompromise,
                now,
            )
            .unwrap();
        authority
            .revoke("Renamed phone", DeviceRevocationReason::Ordinary, now)
            .unwrap();

        let audit = fs::read_to_string(temp.path().join("mobile/audit.jsonl")).unwrap();
        for event in [
            "device_renamed",
            "device_grant_reissued",
            "device_lost_revoked",
            "device_compromise_revoked",
            "device_revoked",
        ] {
            assert!(audit.contains(&format!("\"event\":\"{event}\"")));
        }
        for forbidden in ["publicKey", "subjectKey", "signature", "nonce", "scope"] {
            assert!(!audit.contains(forbidden));
        }
    }
}
