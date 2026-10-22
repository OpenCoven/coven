//! Protected owner introduction policy: owner-root anchored install and
//! hash-chained replacement. No route or CLI command reaches this seam yet.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::assurance::{
    validate_public_key, verify_and_consume_assurance, verify_assurance_signature,
    AssuranceChallengeStore, AssuranceContext, ChallengeBinding, IssuedAssuranceChallenge,
    PresentedAssuranceProof, RequestedAssurance,
};
use super::audit::{append_event, append_policy_event, MobileAuditEvent};
use super::config::{
    atomic_create_private, atomic_replace_private, ensure_private_mobile_dir, validate_private_file,
};
use super::grant::{
    DeviceActionIntent, DeviceScope, GrantTransportConstraint, DEVICE_ACTION_VERSION,
};
use super::introduction::IntroductionPolicy;
use super::registry::DeviceRegistry;

const POLICY_FILE: &str = "introduction-policy.json";
const POLICY_LOCK_FILE: &str = ".introduction-policy.lock";
const POLICY_STORE_VERSION: u16 = 1;
const MAX_POLICY_ACTION_SECONDS: i64 = 300;
const INSTALL_OPERATION: &str = "owner.introduction_policy.install";
const REPLACE_OPERATION: &str = "owner.introduction_policy.replace";
static POLICY_WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug)]
pub(super) struct PolicyCommitUncertain;

impl std::fmt::Display for PolicyCommitUncertain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("introduction policy commit outcome uncertain; reload persisted state")
    }
}

/// Persisted mirror of `IntroductionPolicy`, which itself stays
/// non-deserializable so it can never arrive as an enrollment request field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PolicyDocument {
    eligible_introducers: Vec<Uuid>,
    required_approvals: u16,
    allowed_scopes: Vec<DeviceScope>,
    maximum_grant_lifetime_seconds: u32,
    required_assurance: RequestedAssurance,
    maximum_step_up_age_seconds: u32,
}

impl From<&IntroductionPolicy> for PolicyDocument {
    fn from(policy: &IntroductionPolicy) -> Self {
        Self {
            eligible_introducers: policy.eligible_introducers.clone(),
            required_approvals: policy.required_approvals,
            allowed_scopes: policy.allowed_scopes.clone(),
            maximum_grant_lifetime_seconds: policy.maximum_grant_lifetime_seconds,
            required_assurance: policy.required_assurance,
            maximum_step_up_age_seconds: policy.maximum_step_up_age_seconds,
        }
    }
}

impl From<PolicyDocument> for IntroductionPolicy {
    fn from(document: PolicyDocument) -> Self {
        Self {
            eligible_introducers: document.eligible_introducers,
            required_approvals: document.required_approvals,
            allowed_scopes: document.allowed_scopes,
            maximum_grant_lifetime_seconds: document.maximum_grant_lifetime_seconds,
            required_assurance: document.required_assurance,
            maximum_step_up_age_seconds: document.maximum_step_up_age_seconds,
        }
    }
}

/// Every field is material: the approver signs the exact predecessor, so a
/// signed change applies to one chain head only and cannot be replayed.
#[derive(Debug, Clone)]
pub struct PolicyProposal {
    pub revision: u64,
    pub previous_digest: Option<String>,
    pub policy: IntroductionPolicy,
    pub nonce: [u8; 32],
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

pub enum PolicyApproval {
    /// Owner-root step-up: the only approval that may broaden policy.
    OwnerRoot { signature: String },
    /// A current eligible introducer may only keep or tighten policy.
    Introducer {
        device_id: Uuid,
        possession_signature: String,
        step_up: PresentedAssuranceProof,
    },
}

#[derive(Debug, Clone)]
pub struct InstalledPolicy {
    pub revision: u64,
    pub digest: String,
    pub owner_root_key: String,
    pub policy: IntroductionPolicy,
    pub committed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", tag = "kind")]
enum StoredApproval {
    OwnerRoot {
        action: DeviceActionIntent,
        signature: String,
    },
    Introducer {
        device_id: Uuid,
        grant_id: Uuid,
        revocation_epoch: u64,
        authorization_key_id: String,
        challenge: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredOwnerPolicy {
    version: u16,
    audience: String,
    owner_root_key: String,
    revision: u64,
    previous_digest: Option<String>,
    digest: String,
    policy: PolicyDocument,
    approval: StoredApproval,
    committed_at: DateTime<Utc>,
    transition_id: Uuid,
    audited_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PolicyTransitionMaterial<'a> {
    version: u16,
    audience: &'a str,
    owner_root_key: &'a str,
    revision: u64,
    previous_digest: Option<&'a str>,
    policy: &'a PolicyDocument,
}

fn transition_digest(
    audience: &str,
    owner_root_key: &str,
    revision: u64,
    previous_digest: Option<&str>,
    policy: &PolicyDocument,
) -> Result<String> {
    let mut material = b"COVEN-INTRODUCTION-POLICY/1\0".to_vec();
    material.extend(serde_jcs::to_vec(&PolicyTransitionMaterial {
        version: POLICY_STORE_VERSION,
        audience,
        owner_root_key,
        revision,
        previous_digest,
        policy,
    })?);
    Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(material)))
}

/// Names every dimension on which `new` grants introduction authority that
/// `old` refused. Empty means the change keeps or tightens policy.
pub(super) fn broadened_dimensions(
    new: &IntroductionPolicy,
    old: &IntroductionPolicy,
) -> Vec<&'static str> {
    let mut broadened = Vec::new();
    if new.required_approvals < old.required_approvals {
        broadened.push("required approvals");
    }
    if new
        .eligible_introducers
        .iter()
        .any(|id| old.eligible_introducers.binary_search(id).is_err())
    {
        broadened.push("eligible introducers");
    }
    if new
        .allowed_scopes
        .iter()
        .any(|scope| old.allowed_scopes.binary_search(scope).is_err())
    {
        broadened.push("allowed scopes");
    }
    if new.maximum_grant_lifetime_seconds > old.maximum_grant_lifetime_seconds {
        broadened.push("maximum grant lifetime");
    }
    if new.minimum_assurance() < old.minimum_assurance() {
        broadened.push("required assurance");
    }
    if new.maximum_step_up_age_seconds > old.maximum_step_up_age_seconds {
        broadened.push("maximum step-up age");
    }
    broadened
}

pub(super) fn is_at_least_as_strict(new: &IntroductionPolicy, old: &IntroductionPolicy) -> bool {
    broadened_dimensions(new, old).is_empty()
}

struct PolicyStoreLock {
    _process: MutexGuard<'static, ()>,
    file: fs::File,
}

impl PolicyStoreLock {
    fn acquire(directory: &Path) -> Result<Self> {
        let process = POLICY_WRITE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("introduction policy lock was poisoned"))?;
        let lock_path = directory.join(POLICY_LOCK_FILE);
        let file = crate::state_lock::open_lock_file(&lock_path)?;
        file.lock_exclusive()
            .with_context(|| format!("failed to lock {}", lock_path.display()))?;
        Ok(Self {
            _process: process,
            file,
        })
    }
}

impl Drop for PolicyStoreLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Audience is trusted local input (the installation fingerprint). A store
/// bound to another audience fails closed rather than being reinterpreted.
pub struct OwnerPolicyStore {
    home: PathBuf,
    path: PathBuf,
    audience: String,
}

impl OwnerPolicyStore {
    pub fn open(home: &Path, audience: [u8; 32]) -> Result<Self> {
        if audience == [0; 32] {
            bail!("introduction audience is not configured");
        }
        Ok(Self {
            home: home.to_owned(),
            path: ensure_private_mobile_dir(home)?.join(POLICY_FILE),
            audience: URL_SAFE_NO_PAD.encode(audience),
        })
    }

    pub fn current(&self) -> Result<Option<InstalledPolicy>> {
        Ok(self.read()?.map(installed))
    }

    pub fn install_action(
        &self,
        owner_root_key: &str,
        proposal: &PolicyProposal,
    ) -> Result<DeviceActionIntent> {
        self.action(INSTALL_OPERATION, owner_root_key, proposal)
    }

    pub fn replace_action(&self, proposal: &PolicyProposal) -> Result<DeviceActionIntent> {
        let current = self
            .read()?
            .context("introduction policy is not installed")?;
        self.action(REPLACE_OPERATION, &current.owner_root_key, proposal)
    }

    /// First install only. The owner root self-signs, proving possession of the
    /// key it registers; its private half never enters `COVEN_HOME`.
    pub fn install(
        &self,
        owner_root_key: &str,
        proposal: &PolicyProposal,
        root_signature: &str,
        now: DateTime<Utc>,
    ) -> Result<InstalledPolicy> {
        self.audited(now, || {
            let _lock = PolicyStoreLock::acquire(self.lock_dir()?)?;
            if self.read()?.is_some() {
                bail!("introduction policy is already installed");
            }
            validate_public_key(owner_root_key)?;
            if proposal.revision != 1 || proposal.previous_digest.is_some() {
                bail!("introduction policy install must start the chain");
            }
            let action = self.action(INSTALL_OPERATION, owner_root_key, proposal)?;
            validate_window(&action, now)?;
            verify_assurance_signature(owner_root_key, &action.canonical_bytes()?, root_signature)?;
            let stored = self.record(
                owner_root_key,
                proposal,
                action.effect_digest.clone(),
                StoredApproval::OwnerRoot {
                    action,
                    signature: root_signature.to_owned(),
                },
                now,
            );
            let bytes = encode(&stored)?;
            if !atomic_create_private(&self.path, &bytes).context(PolicyCommitUncertain)? {
                bail!("introduction policy is already installed");
            }
            Ok(stored)
        })
    }

    /// Issues an action step-up challenge for a current eligible introducer.
    pub fn issue_replacement_challenge(
        &self,
        device_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<IssuedAssuranceChallenge> {
        let current = self
            .read()?
            .context("introduction policy is not installed")?;
        let policy = IntroductionPolicy::from(current.policy);
        if policy
            .eligible_introducers
            .binary_search(&device_id)
            .is_err()
        {
            bail!("device is not an eligible introducer under the current policy");
        }
        DeviceRegistry::load(&self.home)?.with_introducer(device_id, |source, key| {
            Ok(AssuranceChallengeStore::load(&self.home)?.issue(
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

    pub fn replace(
        &self,
        proposal: &PolicyProposal,
        approval: &PolicyApproval,
        now: DateTime<Utc>,
    ) -> Result<InstalledPolicy> {
        self.audited(now, || {
            let _lock = PolicyStoreLock::acquire(self.lock_dir()?)?;
            let current = self
                .read()?
                .context("introduction policy is not installed")?;
            if proposal.revision != current.revision.saturating_add(1)
                || proposal.previous_digest.as_deref() != Some(current.digest.as_str())
            {
                bail!("introduction policy proposal does not extend the current chain");
            }
            let action = self.action(REPLACE_OPERATION, &current.owner_root_key, proposal)?;
            validate_window(&action, now)?;
            let previous = IntroductionPolicy::from(current.policy.clone());
            if previous.digest()? == proposal.policy.digest()? {
                bail!("introduction policy proposal is unchanged");
            }
            let stored_approval = match approval {
                PolicyApproval::OwnerRoot { signature } => {
                    verify_assurance_signature(
                        &current.owner_root_key,
                        &action.canonical_bytes()?,
                        signature,
                    )?;
                    StoredApproval::OwnerRoot {
                        action: action.clone(),
                        signature: signature.clone(),
                    }
                }
                PolicyApproval::Introducer {
                    device_id,
                    possession_signature,
                    step_up,
                } => {
                    if !is_at_least_as_strict(&proposal.policy, &previous) {
                        bail!("introduction policy downgrade requires owner-root step-up");
                    }
                    self.verify_introducer(
                        &previous,
                        *device_id,
                        &action,
                        possession_signature,
                        step_up,
                        now,
                    )?
                }
            };
            let stored = self.record(
                &current.owner_root_key,
                proposal,
                action.effect_digest,
                stored_approval,
                now,
            );
            atomic_replace_private(&self.path, &encode(&stored)?).context(PolicyCommitUncertain)?;
            Ok(stored)
        })
    }

    /// Re-delivers an audit record for a committed transition. It never
    /// re-applies or re-authorizes a policy change.
    pub fn reconcile_audit(&self, now: DateTime<Utc>) -> Result<bool> {
        let _lock = PolicyStoreLock::acquire(self.lock_dir()?)?;
        let Some(stored) = self.read()? else {
            return Ok(false);
        };
        if stored.audited_at.is_some() {
            return Ok(false);
        }
        self.deliver_audit(stored, now)?;
        Ok(true)
    }

    fn verify_introducer(
        &self,
        previous: &IntroductionPolicy,
        device_id: Uuid,
        action: &DeviceActionIntent,
        possession_signature: &str,
        step_up: &PresentedAssuranceProof,
        now: DateTime<Utc>,
    ) -> Result<StoredApproval> {
        if previous
            .eligible_introducers
            .binary_search(&device_id)
            .is_err()
        {
            bail!("device is not an eligible introducer under the current policy");
        }
        let mut device_action = action.clone();
        device_action.operation = "devices.introduction_policy.replace".into();
        let challenges = AssuranceChallengeStore::load(&self.home)?;
        DeviceRegistry::load(&self.home)?.with_introducer(device_id, |source, key| {
            if source.grant.restrictions.transport != GrantTransportConstraint::AnyAuthenticated {
                bail!("policy approval cannot verify the source transport restriction");
            }
            source.grant.validate(&source.device.public_key_x963)?;
            if key.enrolled_at > now || key.assurance_class.ceiling() < previous.minimum_assurance()
            {
                bail!("policy approver authorization key cannot satisfy policy");
            }
            verify_assurance_signature(
                &source.device.public_key_x963,
                &device_action.canonical_bytes()?,
                possession_signature,
            )?;
            if step_up.issued_at < action.issued_at
                || now - step_up.issued_at
                    > chrono::Duration::seconds(i64::from(previous.maximum_step_up_age_seconds))
            {
                bail!("policy approval step-up is not fresh");
            }
            let verified = verify_and_consume_assurance(
                &challenges,
                key,
                source.grant.id,
                source.grant.revocation_epoch,
                step_up,
                AssuranceContext::Action(&device_action),
                now,
            )?;
            if verified.effective_assurance < previous.minimum_assurance() {
                bail!("policy approval requires stronger verified assurance");
            }
            source.grant.authorize(
                Some(DeviceScope::DeviceAdmin),
                verified.effective_assurance,
                now,
            )?;
            Ok(StoredApproval::Introducer {
                device_id,
                grant_id: source.grant.id,
                revocation_epoch: source.grant.revocation_epoch,
                authorization_key_id: key.subject_key_id.clone(),
                challenge: URL_SAFE_NO_PAD.encode(step_up.challenge),
            })
        })
    }

    fn action(
        &self,
        operation: &str,
        owner_root_key: &str,
        proposal: &PolicyProposal,
    ) -> Result<DeviceActionIntent> {
        proposal.policy.validate()?;
        if proposal.revision == 0 || proposal.nonce == [0; 32] {
            bail!("invalid introduction policy proposal");
        }
        if let Some(previous) = &proposal.previous_digest {
            validate_digest(previous)?;
        }
        Ok(DeviceActionIntent {
            version: DEVICE_ACTION_VERSION,
            scope: DeviceScope::DeviceAdmin,
            operation: operation.into(),
            target: self.audience.clone(),
            effect_digest: transition_digest(
                &self.audience,
                owner_root_key,
                proposal.revision,
                proposal.previous_digest.as_deref(),
                &PolicyDocument::from(&proposal.policy),
            )?,
            nonce: URL_SAFE_NO_PAD.encode(proposal.nonce),
            issued_at: proposal.issued_at,
            expires_at: proposal.expires_at,
        })
    }

    fn record(
        &self,
        owner_root_key: &str,
        proposal: &PolicyProposal,
        digest: String,
        approval: StoredApproval,
        now: DateTime<Utc>,
    ) -> StoredOwnerPolicy {
        StoredOwnerPolicy {
            version: POLICY_STORE_VERSION,
            audience: self.audience.clone(),
            owner_root_key: owner_root_key.to_owned(),
            revision: proposal.revision,
            previous_digest: proposal.previous_digest.clone(),
            digest,
            policy: PolicyDocument::from(&proposal.policy),
            approval,
            committed_at: now,
            transition_id: Uuid::new_v4(),
            audited_at: None,
        }
    }

    /// Mirrors the introduction contract: rejected/uncertain outcomes are
    /// audited, and success is returned only after audit delivery.
    fn audited(
        &self,
        now: DateTime<Utc>,
        commit: impl FnOnce() -> Result<StoredOwnerPolicy>,
    ) -> Result<InstalledPolicy> {
        let stored = match commit() {
            Ok(stored) => stored,
            Err(error) => {
                let event = if error.is::<PolicyCommitUncertain>() {
                    MobileAuditEvent::IntroductionPolicyUncertain
                } else {
                    MobileAuditEvent::IntroductionPolicyRejected
                };
                append_event(&self.home, now, event, None).context(format!(
                    "introduction policy change failed ({error}); audit delivery failed"
                ))?;
                return Err(error);
            }
        };
        let _lock = PolicyStoreLock::acquire(self.lock_dir()?)?;
        self.deliver_audit(stored, now)
    }

    fn deliver_audit(
        &self,
        mut stored: StoredOwnerPolicy,
        now: DateTime<Utc>,
    ) -> Result<InstalledPolicy> {
        let event = if stored.revision == 1 {
            MobileAuditEvent::IntroductionPolicyInstalled
        } else {
            MobileAuditEvent::IntroductionPolicyReplaced
        };
        append_policy_event(&self.home, stored.transition_id, stored.committed_at, event)
            .context("introduction policy committed; audit delivery pending")?;
        // Only mark the head we delivered; a newer head keeps its own outbox.
        if let Some(head) = self.read()? {
            if head.transition_id == stored.transition_id && head.audited_at.is_none() {
                stored.audited_at = Some(now.max(stored.committed_at));
                atomic_replace_private(&self.path, &encode(&stored)?)
                    .context("introduction policy committed; audit receipt persistence pending")?;
            }
        }
        Ok(installed(stored))
    }

    fn lock_dir(&self) -> Result<&Path> {
        self.path
            .parent()
            .context("introduction policy store must have a parent directory")
    }

    fn read(&self) -> Result<Option<StoredOwnerPolicy>> {
        match fs::symlink_metadata(&self.path) {
            Ok(_) => validate_private_file(&self.path)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect {}", self.path.display()));
            }
        }
        let stored: StoredOwnerPolicy = serde_json::from_slice(
            &fs::read(&self.path)
                .with_context(|| format!("failed to read {}", self.path.display()))?,
        )
        .with_context(|| format!("failed to parse {}", self.path.display()))?;
        self.validate_stored(&stored)?;
        Ok(Some(stored))
    }

    fn validate_stored(&self, stored: &StoredOwnerPolicy) -> Result<()> {
        if stored.version != POLICY_STORE_VERSION {
            bail!("unsupported introduction policy store version");
        }
        if stored.audience != self.audience {
            bail!("introduction policy is bound to a different installation");
        }
        validate_public_key(&stored.owner_root_key)?;
        IntroductionPolicy::from(stored.policy.clone()).validate()?;
        let chain_valid = match (&stored.previous_digest, stored.revision) {
            (None, 1) => true,
            (Some(previous), revision) if revision > 1 => validate_digest(previous).is_ok(),
            _ => false,
        };
        let digest = transition_digest(
            &stored.audience,
            &stored.owner_root_key,
            stored.revision,
            stored.previous_digest.as_deref(),
            &stored.policy,
        )?;
        if !chain_valid
            || digest != stored.digest
            || stored.transition_id.is_nil()
            || stored.audited_at.is_some_and(|at| at < stored.committed_at)
        {
            bail!("introduction policy store is inconsistent");
        }
        match &stored.approval {
            // Root-approved heads stay self-verifying against the root key.
            StoredApproval::OwnerRoot { action, signature } => {
                let operation = if stored.revision == 1 {
                    INSTALL_OPERATION
                } else {
                    REPLACE_OPERATION
                };
                if action.operation != operation
                    || action.target != stored.audience
                    || action.effect_digest != stored.digest
                    || action.scope != DeviceScope::DeviceAdmin
                {
                    bail!("introduction policy approval does not match its record");
                }
                verify_assurance_signature(
                    &stored.owner_root_key,
                    &action.canonical_bytes()?,
                    signature,
                )
                .context("introduction policy root signature is invalid")?;
            }
            StoredApproval::Introducer {
                device_id,
                authorization_key_id,
                ..
            } => {
                if stored.revision == 1 || device_id.is_nil() || authorization_key_id.is_empty() {
                    bail!("introduction policy approval does not match its record");
                }
            }
        }
        Ok(())
    }
}

fn installed(stored: StoredOwnerPolicy) -> InstalledPolicy {
    InstalledPolicy {
        revision: stored.revision,
        digest: stored.digest,
        owner_root_key: stored.owner_root_key,
        policy: stored.policy.into(),
        committed_at: stored.committed_at,
    }
}

fn validate_window(action: &DeviceActionIntent, now: DateTime<Utc>) -> Result<()> {
    let lifetime = action.expires_at - action.issued_at;
    if lifetime <= chrono::Duration::zero()
        || lifetime > chrono::Duration::seconds(MAX_POLICY_ACTION_SECONDS)
        || action.issued_at > now
        || now >= action.expires_at
    {
        bail!("introduction policy approval is outside its validity window");
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<()> {
    let bytes = URL_SAFE_NO_PAD.decode(value)?;
    if bytes.len() != 32 || URL_SAFE_NO_PAD.encode(bytes) != value {
        bail!("noncanonical introduction policy digest");
    }
    Ok(())
}

fn encode(stored: &StoredOwnerPolicy) -> Result<Vec<u8>> {
    let mut encoded =
        serde_json::to_vec_pretty(stored).context("failed to encode introduction policy")?;
    encoded.push(b'\n');
    Ok(encoded)
}

#[cfg(test)]
#[path = "../../tests/fixtures/owner-introduction-policy/mod.rs"]
mod tests;
