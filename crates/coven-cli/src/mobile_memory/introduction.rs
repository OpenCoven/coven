//! Transport-independent local authorization checkpoint; no enrollment route.
//! Trusted inputs are the owner's explicit policy and instance fingerprint.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::assurance::{
    validate_public_key, verify_and_consume_assurance, verify_assurance_signature,
    AssuranceChallengeStore, AssuranceContext, ChallengeBinding, DeviceAuthorizationKeyRecord,
    IssuedAssuranceChallenge, PresentedAssuranceProof, RequestedAssurance,
};
use super::audit::{append_event, append_introduction_event, MobileAuditEvent};
use super::grant::{
    validate_scope_set, AssuranceLevel, DeviceActionIntent, DeviceGrant, DeviceScope,
    GrantTransportConstraint, DEVICE_ACTION_VERSION,
};
use super::registry::{DeviceAuthorizationRecord, DeviceRecord, DeviceRegistry};

pub(super) const MAX_INTRODUCTIONS: usize = 128;

#[derive(Debug)]
pub(super) struct IntroductionCommitUncertain;

impl std::fmt::Display for IntroductionCommitUncertain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("introduction commit outcome uncertain; reconcile persisted state")
    }
}

/// Owner-provided local configuration. There is deliberately no Default or
/// Deserialize implementation: this is never an enrollment request field.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntroductionPolicy {
    pub eligible_introducers: Vec<Uuid>,
    pub required_approvals: u16,
    pub allowed_scopes: Vec<DeviceScope>,
    pub maximum_grant_lifetime_seconds: u32,
    pub required_assurance: RequestedAssurance,
    pub maximum_step_up_age_seconds: u32,
}

impl IntroductionPolicy {
    fn validate(&self) -> Result<()> {
        if self.required_approvals != 1 {
            bail!("unsupported introduction approval threshold");
        }
        if self.eligible_introducers.is_empty()
            || self.eligible_introducers.len() > 128
            || self.eligible_introducers.iter().any(Uuid::is_nil)
            || self.eligible_introducers.windows(2).any(|w| w[0] >= w[1])
            || self.maximum_grant_lifetime_seconds == 0
            || self.maximum_grant_lifetime_seconds > 365 * 24 * 60 * 60
            || self.maximum_step_up_age_seconds == 0
            || self.maximum_step_up_age_seconds > 120
        {
            bail!("invalid introduction policy");
        }
        validate_scope_set(&self.allowed_scopes)?;
        Ok(())
    }

    fn digest(&self) -> Result<String> {
        self.validate()?;
        Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(serde_jcs::to_vec(self)?)))
    }

    fn minimum_assurance(&self) -> AssuranceLevel {
        match self.required_assurance {
            RequestedAssurance::FreshUserVerification => AssuranceLevel::FreshUserVerification,
            RequestedAssurance::FreshBiometric => AssuranceLevel::FreshBiometric,
        }
    }
}

/// All fields are material, including the complete destination grant and
/// presentation. Both possession signatures and the step-up action bind them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IntroductionRequest {
    pub version: u16,
    pub audience: [u8; 32],
    pub policy_digest: String,
    pub introducer_id: Uuid,
    pub introducer_grant_id: Uuid,
    pub introducer_revocation_epoch: u64,
    pub destination_id: Uuid,
    pub destination_public_key: String,
    pub device_name: String,
    pub device_context: String,
    pub grant: DeviceGrant,
    pub nonce: [u8; 32],
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl IntroductionRequest {
    pub fn action(&self) -> Result<DeviceActionIntent> {
        self.validate()?;
        let mut material = b"COVEN-INTRODUCTION/1\0".to_vec();
        material.extend(serde_jcs::to_vec(self)?);
        Ok(DeviceActionIntent {
            version: DEVICE_ACTION_VERSION,
            scope: DeviceScope::DeviceAdmin,
            operation: "devices.introduction.approve".into(),
            target: URL_SAFE_NO_PAD.encode(self.audience),
            effect_digest: URL_SAFE_NO_PAD.encode(Sha256::digest(material)),
            nonce: URL_SAFE_NO_PAD.encode(self.nonce),
            issued_at: self.issued_at,
            expires_at: self.expires_at,
        })
    }

    /// The destination signs a distinct exact action with its possession key.
    pub fn possession_bytes(&self) -> Result<Vec<u8>> {
        let mut action = self.action()?;
        action.operation = "devices.introduction.accept".into();
        Ok(action.canonical_bytes()?)
    }

    fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.audience == [0; 32]
            || self.nonce == [0; 32]
            || self.introducer_id.is_nil()
            || self.destination_id.is_nil()
            || self.destination_id == self.introducer_id
            || self.grant.id.is_nil()
            || self.grant.revocation_epoch != 0
            || self.grant.issued_at != self.issued_at
        {
            bail!("invalid introduction request");
        }
        validate_digest(&self.policy_digest)?;
        validate_public_key(&self.destination_public_key)?;
        self.grant.validate(&self.destination_public_key)?;
        for (text, limit) in [(&self.device_name, 80), (&self.device_context, 512)] {
            if text.is_empty()
                || text.trim() != text
                || text.chars().count() > limit
                || text.chars().any(char::is_control)
            {
                bail!("invalid introduction presentation");
            }
        }
        let lifetime = self.expires_at - self.issued_at;
        if lifetime <= chrono::Duration::zero() || lifetime > chrono::Duration::seconds(300) {
            bail!("invalid introduction lifetime");
        }
        Ok(())
    }
}

pub struct IntroductionApproval {
    pub possession_signature: String,
    pub step_up: PresentedAssuranceProof,
}

/// Local Rust seam only. Future ingress must obtain policy/audience from the
/// owner authority, never from a remote request or an account/relay service.
pub struct IntroductionAuthority {
    home: PathBuf,
    audience: [u8; 32],
    policy: IntroductionPolicy,
    registry: DeviceRegistry,
    challenges: AssuranceChallengeStore,
}

impl IntroductionAuthority {
    pub fn load(
        home: &Path,
        audience: [u8; 32],
        policy: Option<IntroductionPolicy>,
    ) -> Result<Self> {
        let policy = policy.context("introduction policy is not configured")?;
        policy.validate()?;
        if audience == [0; 32] {
            bail!("introduction audience is not configured");
        }
        Ok(Self {
            home: home.to_owned(),
            audience,
            policy,
            registry: DeviceRegistry::load(home)?,
            challenges: AssuranceChallengeStore::load(home)?,
        })
    }

    pub fn policy_digest(&self) -> Result<String> {
        self.policy.digest()
    }

    pub fn issue_challenge(
        &self,
        request: &IntroductionRequest,
        now: DateTime<Utc>,
    ) -> Result<IssuedAssuranceChallenge> {
        self.registry
            .with_introducer(request.introducer_id, |source, key| {
                self.validate_authority(request, source, key, now)?;
                Ok(self.challenges.issue(
                    ChallengeBinding {
                        device_id: source.device.id,
                        grant_id: source.grant.id,
                        revocation_epoch: source.grant.revocation_epoch,
                        authorization_key_id: key.subject_key_id.clone(),
                        authorization_key_epoch: key.key_epoch,
                    },
                    now,
                )?)
            })
    }

    pub fn introduce(
        &self,
        request: &IntroductionRequest,
        destination_signature: &str,
        approval: &IntroductionApproval,
        now: DateTime<Utc>,
    ) -> Result<DeviceAuthorizationRecord> {
        let (destination, transition) =
            match self.introduce_inner(request, destination_signature, approval, now) {
                Ok(committed) => committed,
                Err(error) => {
                    let event = if error.is::<IntroductionCommitUncertain>() {
                        MobileAuditEvent::DeviceIntroductionUncertain
                    } else {
                        MobileAuditEvent::DeviceIntroductionRejected
                    };
                    append_event(&self.home, now, event, None).context(format!(
                        "introduction failed ({error}); audit delivery failed"
                    ))?;
                    return Err(error);
                }
            };
        let receipt = append_introduction_event(&self.home, &transition)
            .context("introduction committed; audit delivery pending")?;
        self.registry
            .mark_introduction_audited(&receipt, now)
            .context("introduction committed; audit receipt persistence pending")?;
        Ok(destination)
    }

    fn introduce_inner(
        &self,
        request: &IntroductionRequest,
        destination_signature: &str,
        approval: &IntroductionApproval,
        now: DateTime<Utc>,
    ) -> Result<(DeviceAuthorizationRecord, IntroductionTransition)> {
        let record = DeviceRecord {
            id: request.destination_id,
            display_name: request.device_name.clone(),
            public_key_x963: request.destination_public_key.clone(),
            paired_at: request.issued_at,
            revoked_at: None,
            suspended_at: None,
            scopes: request.grant.scopes.clone(),
        };
        let nonce_digest = URL_SAFE_NO_PAD.encode(Sha256::digest(request.nonce));
        self.registry.commit_introduction(
            request.introducer_id,
            record,
            request.grant.clone(),
            nonce_digest,
            now,
            |source, key| {
                self.validate_authority(request, source, key, now)?;
                let action = request.action()?;
                verify_assurance_signature(
                    &request.destination_public_key,
                    &request.possession_bytes()?,
                    destination_signature,
                )?;
                verify_assurance_signature(
                    &source.device.public_key_x963,
                    &action.canonical_bytes()?,
                    &approval.possession_signature,
                )?;
                if approval.step_up.issued_at < request.issued_at
                    || now - approval.step_up.issued_at
                        > chrono::Duration::seconds(i64::from(
                            self.policy.maximum_step_up_age_seconds,
                        ))
                {
                    bail!("introduction step-up is not fresh");
                }
                let verified = verify_and_consume_assurance(
                    &self.challenges,
                    key,
                    source.grant.id,
                    source.grant.revocation_epoch,
                    &approval.step_up,
                    AssuranceContext::Action(&action),
                    now,
                )?;
                if verified.effective_assurance < self.policy.minimum_assurance() {
                    bail!("introduction requires stronger verified assurance");
                }
                source.grant.authorize(
                    Some(DeviceScope::DeviceAdmin),
                    verified.effective_assurance,
                    now,
                )?;
                Ok(())
            },
        )
    }

    /// Reconcile only committed outbox records. Never retry enrollment or infer
    /// a grant from a consumed challenge, matching name, or cached device.
    pub fn reconcile_audits(&self, now: DateTime<Utc>) -> Result<usize> {
        let pending = self.registry.pending_introduction_audits()?;
        for transition in &pending {
            let receipt = append_introduction_event(&self.home, transition)?;
            self.registry.mark_introduction_audited(&receipt, now)?;
        }
        Ok(pending.len())
    }

    fn validate_authority(
        &self,
        request: &IntroductionRequest,
        source: &DeviceAuthorizationRecord,
        key: &DeviceAuthorizationKeyRecord,
        now: DateTime<Utc>,
    ) -> Result<()> {
        request.validate()?;
        if request.audience != self.audience || request.policy_digest != self.policy.digest()? {
            bail!("introduction audience or owner policy mismatch");
        }
        if self
            .policy
            .eligible_introducers
            .binary_search(&source.device.id)
            .is_err()
            || source.grant.id != request.introducer_grant_id
            || source.grant.revocation_epoch != request.introducer_revocation_epoch
            || source.device.revoked_at.is_some()
            || source.device.suspended_at.is_some()
            || request.issued_at > now
            || now >= request.expires_at
            || key.enrolled_at > now
            || key.public_key_x963 == request.destination_public_key
            || source.device.public_key_x963 == request.destination_public_key
        {
            bail!("introduction authority is not current");
        }
        // This seam has no authenticated transport evidence. Never infer that
        // a caller using the local API satisfies a DirectOnly grant remotely.
        if source.grant.restrictions.transport != GrantTransportConstraint::AnyAuthenticated {
            bail!("introduction cannot verify the source transport restriction");
        }
        source.grant.validate(&source.device.public_key_x963)?;
        source.grant.authorize(
            Some(DeviceScope::DeviceAdmin),
            key.assurance_class.ceiling(),
            now,
        )?;
        if key.assurance_class.ceiling() < self.policy.minimum_assurance() {
            bail!("introduction authorization key cannot satisfy policy");
        }
        let grant = &request.grant;
        let expiry = grant.expires_at.context("introduced grant must expire")?;
        if grant.not_before > now
            || now >= expiry
            || expiry - grant.issued_at
                > chrono::Duration::seconds(i64::from(self.policy.maximum_grant_lifetime_seconds))
            || source.grant.expires_at.is_some_and(|limit| expiry > limit)
            || grant.minimum_assurance < source.grant.minimum_assurance
            || (source.grant.restrictions.transport == GrantTransportConstraint::DirectOnly
                && grant.restrictions.transport != GrantTransportConstraint::DirectOnly)
            || grant.scopes.iter().any(|scope| {
                self.policy.allowed_scopes.binary_search(scope).is_err()
                    || source.grant.scopes.binary_search(scope).is_err()
                    || (source
                        .grant
                        .restrictions
                        .require_fresh_user_verification_for
                        .binary_search(scope)
                        .is_ok()
                        && grant
                            .restrictions
                            .require_fresh_user_verification_for
                            .binary_search(scope)
                            .is_err())
            })
        {
            bail!("introduction would broaden delegated authority");
        }
        Ok(())
    }
}

/// Consumption and audit outbox are part of the same registry replacement as
/// the grant. Only opaque random transition IDs enter the audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct IntroductionTransition {
    pub transition_id: Uuid,
    pub nonce_digest: String,
    pub occurred_at: DateTime<Utc>,
    pub audited_at: Option<DateTime<Utc>>,
}

pub(super) fn validate_transitions(transitions: &[IntroductionTransition]) -> Result<()> {
    if transitions.len() > MAX_INTRODUCTIONS {
        bail!("introduction consumption ledger is full");
    }
    for (index, transition) in transitions.iter().enumerate() {
        validate_digest(&transition.nonce_digest)?;
        if transition.transition_id.is_nil()
            || transition
                .audited_at
                .is_some_and(|at| at < transition.occurred_at)
            || transitions[..index].iter().any(|prior| {
                prior.transition_id == transition.transition_id
                    || prior.nonce_digest == transition.nonce_digest
            })
        {
            bail!("invalid introduction consumption ledger");
        }
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<()> {
    let bytes = URL_SAFE_NO_PAD.decode(value)?;
    if bytes.len() != 32 || URL_SAFE_NO_PAD.encode(bytes) != value {
        bail!("noncanonical introduction digest");
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/fixtures/trusted-device-introduction/mod.rs"]
mod tests;
