//! Transport-independent introduction authorization. Ingress is the local
//! enrollment route; trusted inputs are the owner's policy and instance key.

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
use super::owner_policy::OwnerPolicyStore;
use super::passkey::{action_challenge, PasskeyAssertion, PasskeyStore};
use super::registry::{DeviceAuthorizationRecord, DeviceRecord, DeviceRegistry, PendingEnrollment};

pub(super) const MAX_INTRODUCTIONS: usize = 128;
/// Upper bound on N for threshold approval; each approver is a separate lock
/// and step-up verification inside one commit.
pub(super) const MAX_INTRODUCTION_APPROVERS: usize = 16;

#[cfg(test)]
thread_local! {
    /// Runs inside the commit after every check passes, before the registry write.
    pub(super) static BEFORE_INTRODUCTION_WRITE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

#[derive(Debug)]
pub(super) struct IntroductionCommitUncertain;

impl std::fmt::Display for IntroductionCommitUncertain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("introduction commit outcome uncertain; reconcile persisted state")
    }
}

/// Owner-provided local configuration. There is deliberately no Default or
/// Deserialize implementation: this is never an enrollment request field.
/// `required_approvals` is always explicit; there is no default threshold.
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
    pub(super) fn validate(&self) -> Result<()> {
        if self.required_approvals == 0
            || usize::from(self.required_approvals) > self.eligible_introducers.len()
            || usize::from(self.required_approvals) > MAX_INTRODUCTION_APPROVERS
        {
            bail!("invalid introduction approval threshold");
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

    pub(super) fn digest(&self) -> Result<String> {
        self.validate()?;
        Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(serde_jcs::to_vec(self)?)))
    }

    pub(super) fn minimum_assurance(&self) -> AssuranceLevel {
        match self.required_assurance {
            RequestedAssurance::FreshUserVerification => AssuranceLevel::FreshUserVerification,
            RequestedAssurance::FreshBiometric => AssuranceLevel::FreshBiometric,
        }
    }
}

/// All fields are material, including the complete destination grant and
/// presentation. Both possession signatures and the step-up action bind them.
/// Version 1 names exactly one introducer and is valid only when N = 1.
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
        exact_action(b"COVEN-INTRODUCTION/1\0", self, &self.terms())
    }

    /// The destination signs a distinct exact action with its possession key.
    pub fn possession_bytes(&self) -> Result<Vec<u8>> {
        accept_bytes(self.action()?)
    }

    fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("invalid introduction request");
        }
        self.terms().validate()
    }

    fn terms(&self) -> IntroductionTerms<'_> {
        IntroductionTerms {
            audience: self.audience,
            policy_digest: &self.policy_digest,
            approvers: vec![IntroductionApprover {
                device_id: self.introducer_id,
                grant_id: self.introducer_grant_id,
                revocation_epoch: self.introducer_revocation_epoch,
            }],
            passkey_approver: None,
            destination_id: self.destination_id,
            destination_public_key: &self.destination_public_key,
            device_name: &self.device_name,
            device_context: &self.device_context,
            grant: &self.grant,
            nonce: self.nonce,
            issued_at: self.issued_at,
            expires_at: self.expires_at,
        }
    }
}

/// One approver named by a threshold request, bound to the exact grant and
/// revocation epoch it approves under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IntroductionApprover {
    pub device_id: Uuid,
    pub grant_id: Uuid,
    pub revocation_epoch: u64,
}

/// Version 2: exactly N named approvers (N ≥ 2, sorted by device ID) under
/// its own domain separator. Every other field matches version 1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ThresholdIntroductionRequest {
    pub version: u16,
    pub audience: [u8; 32],
    pub policy_digest: String,
    pub approvers: Vec<IntroductionApprover>,
    /// A registered passkey filling one of the N approvals. Never sole: at
    /// least one device approver is always required alongside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passkey_approver: Option<String>,
    pub destination_id: Uuid,
    pub destination_public_key: String,
    pub device_name: String,
    pub device_context: String,
    pub grant: DeviceGrant,
    pub nonce: [u8; 32],
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl ThresholdIntroductionRequest {
    /// Every approver signs this same action with its own keys.
    pub fn action(&self) -> Result<DeviceActionIntent> {
        self.validate()?;
        exact_action(b"COVEN-INTRODUCTION/2\0", self, &self.terms())
    }

    pub fn possession_bytes(&self) -> Result<Vec<u8>> {
        accept_bytes(self.action()?)
    }

    fn validate(&self) -> Result<()> {
        if self.version != 2
            || self.approvers.len() + usize::from(self.passkey_approver.is_some()) < 2
        {
            bail!("invalid threshold introduction request");
        }
        self.terms().validate()
    }

    fn terms(&self) -> IntroductionTerms<'_> {
        IntroductionTerms {
            audience: self.audience,
            policy_digest: &self.policy_digest,
            approvers: self.approvers.clone(),
            passkey_approver: self.passkey_approver.as_deref(),
            destination_id: self.destination_id,
            destination_public_key: &self.destination_public_key,
            device_name: &self.device_name,
            device_context: &self.device_context,
            grant: &self.grant,
            nonce: self.nonce,
            issued_at: self.issued_at,
            expires_at: self.expires_at,
        }
    }
}

/// Either request version, as carried by an enrollment envelope. Untagged:
/// `deny_unknown_fields` on each version keeps the two shapes disjoint, so a
/// v1 envelope parses exactly as before.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IntroductionTranscript {
    Single(IntroductionRequest),
    Threshold(ThresholdIntroductionRequest),
}

impl IntroductionTranscript {
    pub fn action(&self) -> Result<DeviceActionIntent> {
        match self {
            Self::Single(request) => request.action(),
            Self::Threshold(request) => request.action(),
        }
    }

    pub fn possession_bytes(&self) -> Result<Vec<u8>> {
        match self {
            Self::Single(request) => request.possession_bytes(),
            Self::Threshold(request) => request.possession_bytes(),
        }
    }

    fn terms(&self) -> IntroductionTerms<'_> {
        match self {
            Self::Single(request) => request.terms(),
            Self::Threshold(request) => request.terms(),
        }
    }

    pub fn approvers(&self) -> Vec<IntroductionApprover> {
        self.terms().approvers
    }

    pub fn names_approver(&self, device_id: Uuid) -> bool {
        self.terms()
            .approvers
            .iter()
            .any(|a| a.device_id == device_id)
    }

    pub fn audience(&self) -> [u8; 32] {
        self.terms().audience
    }

    pub fn policy_digest(&self) -> &str {
        self.terms().policy_digest
    }

    pub fn destination_id(&self) -> Uuid {
        self.terms().destination_id
    }

    pub fn destination_public_key(&self) -> &str {
        self.terms().destination_public_key
    }

    pub fn device_name(&self) -> &str {
        self.terms().device_name
    }

    pub fn device_context(&self) -> &str {
        self.terms().device_context
    }

    pub fn grant(&self) -> &DeviceGrant {
        self.terms().grant
    }

    pub fn nonce(&self) -> [u8; 32] {
        self.terms().nonce
    }

    pub fn issued_at(&self) -> DateTime<Utc> {
        self.terms().issued_at
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        self.terms().expires_at
    }
}

/// Version-independent view used for every authority check, so v1 and v2
/// cannot drift apart. Signatures always cover the versioned original.
struct IntroductionTerms<'a> {
    audience: [u8; 32],
    policy_digest: &'a str,
    approvers: Vec<IntroductionApprover>,
    passkey_approver: Option<&'a str>,
    destination_id: Uuid,
    destination_public_key: &'a str,
    device_name: &'a str,
    device_context: &'a str,
    grant: &'a DeviceGrant,
    nonce: [u8; 32],
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

impl IntroductionTerms<'_> {
    fn validate(&self) -> Result<()> {
        if self.audience == [0; 32]
            || self.nonce == [0; 32]
            || self.destination_id.is_nil()
            || self.approvers.is_empty()
            || self.approvers.len() > MAX_INTRODUCTION_APPROVERS
            || self
                .approvers
                .windows(2)
                .any(|w| w[0].device_id >= w[1].device_id)
            || self.approvers.iter().any(|approver| {
                approver.device_id.is_nil()
                    || approver.grant_id.is_nil()
                    || approver.device_id == self.destination_id
            })
        {
            bail!("invalid introduction request");
        }
        validate_digest(self.policy_digest)?;
        validate_public_key(self.destination_public_key)?;
        validate_destination(
            self.grant,
            self.destination_public_key,
            self.device_name,
            self.device_context,
            self.issued_at,
            self.expires_at,
        )
    }
}

/// Destination checks shared by introduction and access recovery: a fresh,
/// unrevoked grant for this key, bounded presentation, and a short lifetime.
pub(super) fn validate_destination(
    grant: &DeviceGrant,
    destination_public_key: &str,
    device_name: &str,
    device_context: &str,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) -> Result<()> {
    if grant.id.is_nil() || grant.revocation_epoch != 0 || grant.issued_at != issued_at {
        bail!("invalid introduction request");
    }
    grant.validate(destination_public_key)?;
    for (text, limit) in [(device_name, 80), (device_context, 512)] {
        if text.is_empty()
            || text.trim() != text
            || text.chars().count() > limit
            || text.chars().any(char::is_control)
        {
            bail!("invalid introduction presentation");
        }
    }
    let lifetime = expires_at - issued_at;
    if lifetime <= chrono::Duration::zero() || lifetime > chrono::Duration::seconds(300) {
        bail!("invalid introduction lifetime");
    }
    Ok(())
}

fn exact_action(
    domain: &[u8],
    request: &impl Serialize,
    terms: &IntroductionTerms<'_>,
) -> Result<DeviceActionIntent> {
    let mut material = domain.to_vec();
    material.extend(serde_jcs::to_vec(request)?);
    Ok(DeviceActionIntent {
        version: DEVICE_ACTION_VERSION,
        scope: DeviceScope::DeviceAdmin,
        operation: "devices.introduction.approve".into(),
        target: URL_SAFE_NO_PAD.encode(terms.audience),
        effect_digest: URL_SAFE_NO_PAD.encode(Sha256::digest(material)),
        nonce: URL_SAFE_NO_PAD.encode(terms.nonce),
        issued_at: terms.issued_at,
        expires_at: terms.expires_at,
    })
}

fn accept_bytes(mut action: DeviceActionIntent) -> Result<Vec<u8>> {
    action.operation = "devices.introduction.accept".into();
    Ok(action.canonical_bytes()?)
}

pub struct IntroductionApproval {
    pub possession_signature: String,
    pub step_up: PresentedAssuranceProof,
}

/// The only ingress is local enrollment, which obtains policy and audience
/// from the owner authority, never from a remote request or relay service.
pub struct IntroductionAuthority {
    home: PathBuf,
    audience: [u8; 32],
    policy: IntroductionPolicy,
    /// Set when policy came from the protected store; each check re-reads the
    /// head so a replacement retires requests bound to the previous policy.
    installed: Option<(OwnerPolicyStore, String)>,
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
            installed: None,
            registry: DeviceRegistry::load(home)?,
            challenges: AssuranceChallengeStore::load(home)?,
        })
    }

    /// Loads policy only from the owner-root anchored store.
    pub fn load_installed(home: &Path, audience: [u8; 32]) -> Result<Self> {
        let store = OwnerPolicyStore::open(home, audience)?;
        let current = store
            .current()?
            .context("introduction policy is not installed")?;
        let mut authority = Self::load(home, audience, Some(current.policy))?;
        authority.installed = Some((store, current.digest));
        Ok(authority)
    }

    pub fn policy_digest(&self) -> Result<String> {
        self.policy.digest()
    }

    pub(super) fn maximum_step_up_age(&self) -> chrono::Duration {
        chrono::Duration::seconds(i64::from(self.policy.maximum_step_up_age_seconds))
    }

    /// Admission check before approvers are asked: the request is current under
    /// trusted policy for every named approver and the destination proved
    /// possession. Consumes nothing; commit re-runs every check.
    pub fn admit(
        &self,
        request: &IntroductionRequest,
        destination_signature: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.admit_terms(
            &request.terms(),
            &request.possession_bytes()?,
            destination_signature,
            now,
        )
    }

    pub fn admit_threshold(
        &self,
        request: &ThresholdIntroductionRequest,
        destination_signature: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.admit_terms(
            &request.terms(),
            &request.possession_bytes()?,
            destination_signature,
            now,
        )
    }

    pub fn issue_challenge(
        &self,
        request: &IntroductionRequest,
        now: DateTime<Utc>,
    ) -> Result<IssuedAssuranceChallenge> {
        request.validate()?;
        self.issue_challenge_for(&request.terms(), request.introducer_id, now)
    }

    pub fn issue_threshold_challenge(
        &self,
        request: &ThresholdIntroductionRequest,
        approver: Uuid,
        now: DateTime<Utc>,
    ) -> Result<IssuedAssuranceChallenge> {
        request.validate()?;
        self.issue_challenge_for(&request.terms(), approver, now)
    }

    pub fn introduce(
        &self,
        request: &IntroductionRequest,
        destination_signature: &str,
        approval: &IntroductionApproval,
        now: DateTime<Utc>,
    ) -> Result<DeviceAuthorizationRecord> {
        let action = request.action()?;
        self.audited_introduce(now, || {
            self.introduce_terms(
                &request.terms(),
                &action,
                destination_signature,
                &[(request.introducer_id, approval)],
                None,
                now,
            )
        })
    }

    /// Commits only with an approval from every named approver, all verified
    /// and consumed inside one registry commit.
    pub fn introduce_threshold(
        &self,
        request: &ThresholdIntroductionRequest,
        destination_signature: &str,
        approvals: &[(Uuid, &IntroductionApproval)],
        now: DateTime<Utc>,
    ) -> Result<DeviceAuthorizationRecord> {
        self.introduce_threshold_with_passkey(request, destination_signature, approvals, None, now)
    }

    /// As `introduce_threshold`, with the named passkey's WebAuthn assertion
    /// over the same exact action filling its one approval.
    pub fn introduce_threshold_with_passkey(
        &self,
        request: &ThresholdIntroductionRequest,
        destination_signature: &str,
        approvals: &[(Uuid, &IntroductionApproval)],
        passkey: Option<&PasskeyAssertion>,
        now: DateTime<Utc>,
    ) -> Result<DeviceAuthorizationRecord> {
        let action = request.action()?;
        self.audited_introduce(now, || {
            self.introduce_terms(
                &request.terms(),
                &action,
                destination_signature,
                approvals,
                passkey,
                now,
            )
        })
    }

    pub fn admit_transcript(
        &self,
        transcript: &IntroductionTranscript,
        destination_signature: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        match transcript {
            IntroductionTranscript::Single(request) => {
                self.admit(request, destination_signature, now)
            }
            IntroductionTranscript::Threshold(request) => {
                self.admit_threshold(request, destination_signature, now)
            }
        }
    }

    pub fn introduce_transcript(
        &self,
        transcript: &IntroductionTranscript,
        destination_signature: &str,
        approvals: &[(Uuid, &IntroductionApproval)],
        passkey: Option<&PasskeyAssertion>,
        now: DateTime<Utc>,
    ) -> Result<DeviceAuthorizationRecord> {
        match transcript {
            IntroductionTranscript::Single(request) => match approvals {
                [(id, approval)] if *id == request.introducer_id && passkey.is_none() => {
                    self.introduce(request, destination_signature, approval, now)
                }
                _ => bail!("introduction approvals do not match the named approvers"),
            },
            IntroductionTranscript::Threshold(request) => self.introduce_threshold_with_passkey(
                request,
                destination_signature,
                approvals,
                passkey,
                now,
            ),
        }
    }

    pub fn passkey_approver(transcript: &IntroductionTranscript) -> Option<&str> {
        transcript.terms().passkey_approver
    }

    fn admit_terms(
        &self,
        terms: &IntroductionTerms<'_>,
        possession_bytes: &[u8],
        destination_signature: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.validate_request(terms, now)?;
        for approver in &terms.approvers {
            self.registry
                .with_introducer(approver.device_id, |source, key| {
                    self.validate_source(terms, approver, source, key, now)
                })?;
        }
        verify_assurance_signature(
            terms.destination_public_key,
            possession_bytes,
            destination_signature,
        )?;
        Ok(())
    }

    fn issue_challenge_for(
        &self,
        terms: &IntroductionTerms<'_>,
        approver_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<IssuedAssuranceChallenge> {
        self.validate_request(terms, now)?;
        let approver = terms
            .approvers
            .iter()
            .find(|approver| approver.device_id == approver_id)
            .context("device is not an approver of this introduction")?;
        self.registry
            .with_introducer(approver.device_id, |source, key| {
                self.validate_source(terms, approver, source, key, now)?;
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

    fn audited_introduce(
        &self,
        now: DateTime<Utc>,
        commit: impl FnOnce() -> Result<(DeviceAuthorizationRecord, IntroductionTransition)>,
    ) -> Result<DeviceAuthorizationRecord> {
        let (destination, transition) = match commit() {
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

    fn introduce_terms(
        &self,
        terms: &IntroductionTerms<'_>,
        action: &DeviceActionIntent,
        destination_signature: &str,
        approvals: &[(Uuid, &IntroductionApproval)],
        passkey: Option<&PasskeyAssertion>,
        now: DateTime<Utc>,
    ) -> Result<(DeviceAuthorizationRecord, IntroductionTransition)> {
        if passkey.map(|assertion| assertion.credential_id.as_str()) != terms.passkey_approver {
            bail!("introduction passkey approval does not match the named passkey");
        }
        // Exactly one approval per named approver; nothing more, nothing less.
        let mut approval_ids: Vec<_> = approvals.iter().map(|(id, _)| *id).collect();
        approval_ids.sort_unstable();
        let approver_ids: Vec<_> = terms.approvers.iter().map(|a| a.device_id).collect();
        if approval_ids != approver_ids {
            bail!("introduction approvals do not match the named approvers");
        }
        let record = DeviceRecord {
            id: terms.destination_id,
            display_name: terms.device_name.to_owned(),
            public_key_x963: terms.destination_public_key.to_owned(),
            paired_at: terms.issued_at,
            revoked_at: None,
            suspended_at: None,
            scopes: terms.grant.scopes.clone(),
        };
        let nonce_digest = URL_SAFE_NO_PAD.encode(Sha256::digest(terms.nonce));
        let mut possession = action.clone();
        possession.operation = "devices.introduction.accept".into();
        let commit = || {
            self.registry.commit_introduction(
                &approver_ids,
                PendingEnrollment {
                    device: record,
                    grant: terms.grant.clone(),
                    nonce_digest,
                    kind: TransitionKind::Introduction,
                },
                now,
                |sources| {
                    self.validate_request(terms, now)?;
                    verify_assurance_signature(
                        terms.destination_public_key,
                        &possession.canonical_bytes()?,
                        destination_signature,
                    )?;
                    for ((source, key), approver) in sources.iter().zip(&terms.approvers) {
                        let (_, approval) = approvals
                            .iter()
                            .find(|(id, _)| *id == approver.device_id)
                            .context("introduction approval is missing")?;
                        self.verify_approval(terms, action, approver, source, key, approval, now)?;
                    }
                    // Passkey store lock is last in the commit's lock order.
                    if let Some(assertion) = passkey {
                        PasskeyStore::open(&self.home, self.audience)?
                            .verify_and_advance(assertion, &action_challenge(action)?)?;
                    }
                    #[cfg(test)]
                    BEFORE_INTRODUCTION_WRITE
                        .with(|hook| hook.borrow_mut().take().map(|hook| hook()));
                    Ok(())
                },
            )
        };
        match &self.installed {
            // Policy → registry: the head is checked and the enrollment written
            // under one policy lock, so a replacement retires this request
            // atomically rather than racing its commit.
            Some((store, digest)) => store.with_current_locked(|head| {
                if head.map(|head| &head.digest) != Some(digest) {
                    bail!("introduction owner policy was replaced");
                }
                commit()
            }),
            None => commit(),
        }
    }

    #[allow(clippy::too_many_arguments)] // each input is a distinct bound value
    fn verify_approval(
        &self,
        terms: &IntroductionTerms<'_>,
        action: &DeviceActionIntent,
        approver: &IntroductionApprover,
        source: &DeviceAuthorizationRecord,
        key: &DeviceAuthorizationKeyRecord,
        approval: &IntroductionApproval,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.validate_source(terms, approver, source, key, now)?;
        verify_assurance_signature(
            &source.device.public_key_x963,
            &action.canonical_bytes()?,
            &approval.possession_signature,
        )?;
        if approval.step_up.issued_at < terms.issued_at
            || now - approval.step_up.issued_at > self.maximum_step_up_age()
        {
            bail!("introduction step-up is not fresh");
        }
        let verified = verify_and_consume_assurance(
            &self.challenges,
            key,
            source.grant.id,
            source.grant.revocation_epoch,
            &approval.step_up,
            AssuranceContext::Action(action),
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

    /// Request-level checks shared by every approver.
    fn validate_request(&self, terms: &IntroductionTerms<'_>, now: DateTime<Utc>) -> Result<()> {
        terms.validate()?;
        if terms.audience != self.audience || terms.policy_digest != self.policy.digest()? {
            bail!("introduction audience or owner policy mismatch");
        }
        if let Some((store, digest)) = &self.installed {
            if store.current()?.map(|head| head.digest).as_ref() != Some(digest) {
                bail!("introduction owner policy was replaced");
            }
        }
        if terms.approvers.len() + usize::from(terms.passkey_approver.is_some())
            != usize::from(self.policy.required_approvals)
            || terms.approvers.iter().any(|approver| {
                self.policy
                    .eligible_introducers
                    .binary_search(&approver.device_id)
                    .is_err()
            })
            || terms.issued_at > now
            || now >= terms.expires_at
        {
            bail!("introduction authority is not current");
        }
        Ok(())
    }

    /// Checks one approver's current state; the destination grant must be no
    /// broader than every approver's grant.
    fn validate_source(
        &self,
        terms: &IntroductionTerms<'_>,
        approver: &IntroductionApprover,
        source: &DeviceAuthorizationRecord,
        key: &DeviceAuthorizationKeyRecord,
        now: DateTime<Utc>,
    ) -> Result<()> {
        if source.device.id != approver.device_id
            || source.grant.id != approver.grant_id
            || source.grant.revocation_epoch != approver.revocation_epoch
            || source.device.revoked_at.is_some()
            || source.device.suspended_at.is_some()
            || key.enrolled_at > now
            || key.public_key_x963 == terms.destination_public_key
            || source.device.public_key_x963 == terms.destination_public_key
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
        let grant = terms.grant;
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
    /// Omitted for introductions so existing records serialize unchanged.
    #[serde(default, skip_serializing_if = "TransitionKind::is_introduction")]
    pub kind: TransitionKind,
    pub nonce_digest: String,
    pub occurred_at: DateTime<Utc>,
    pub audited_at: Option<DateTime<Utc>>,
}

/// Which operation consumed a nonce. Both share one ledger, so a recovery
/// and an introduction can never reuse each other's nonce.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TransitionKind {
    #[default]
    Introduction,
    AccessRecovery,
}

impl TransitionKind {
    fn is_introduction(&self) -> bool {
        *self == Self::Introduction
    }
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

#[cfg(test)]
#[path = "../../tests/fixtures/trusted-device-introduction/threshold.rs"]
pub(super) mod threshold_tests;
