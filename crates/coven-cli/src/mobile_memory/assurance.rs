use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, RwLock};

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use fs2::FileExt;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::config::{atomic_replace_private, ensure_private_mobile_dir, validate_private_file};
use super::grant::{AssuranceLevel, DeviceActionIntent};

pub const AUTHORIZATION_KEYS_FILE: &str = "authorization-keys.json";
pub const ASSURANCE_CHALLENGES_FILE: &str = "assurance-challenges.json";
const AUTHORIZATION_KEY_REGISTRY_VERSION: u16 = 1;
const ASSURANCE_CHALLENGE_STORE_VERSION: u16 = 1;
const MAX_AUTHORIZATION_KEY_RECORDS: usize = 512;
const MAX_ASSURANCE_CHALLENGES: usize = 10_000;
const MAX_ASSURANCE_LIFETIME_SECONDS: i64 = 120;
const DEFAULT_CHALLENGE_LIFETIME_SECONDS: i64 = 60;
const SPENT_CHALLENGE_RETENTION_SECONDS: i64 = 300;
const ASSURANCE_DOMAIN: &[u8] = b"COVEN-ASSURANCE/1\0";
const AUTHORIZATION_KEY_LOCK_FILE: &str = ".authorization-keys.lock";
const ASSURANCE_CHALLENGE_LOCK_FILE: &str = ".assurance-challenges.lock";
static CHALLENGE_STORE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static AUTHORIZATION_KEY_STORE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct AssuranceStoreFileLock {
    file: fs::File,
}

impl AssuranceStoreFileLock {
    fn acquire(path: &Path, lock_name: &str) -> Result<Self> {
        let parent = path
            .parent()
            .context("mobile assurance store has no parent")?;
        let lock_path = parent.join(lock_name);
        let file = crate::state_lock::open_lock_file(&lock_path)?;
        file.lock_exclusive()
            .with_context(|| format!("failed to lock {}", lock_path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for AssuranceStoreFileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssuranceClass {
    BiometricOnly,
    UserVerification,
    DeviceCredential,
}

impl AssuranceClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BiometricOnly => "biometric_only",
            Self::UserVerification => "user_verification",
            Self::DeviceCredential => "device_credential",
        }
    }

    pub(crate) const fn ceiling(self) -> AssuranceLevel {
        match self {
            Self::BiometricOnly => AssuranceLevel::FreshBiometric,
            Self::UserVerification | Self::DeviceCredential => {
                AssuranceLevel::FreshUserVerification
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestedAssurance {
    FreshUserVerification,
    FreshBiometric,
}

impl RequestedAssurance {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FreshUserVerification => "fresh_user_verification",
            Self::FreshBiometric => "fresh_biometric",
        }
    }

    const fn level(self) -> AssuranceLevel {
        match self {
            Self::FreshUserVerification => AssuranceLevel::FreshUserVerification,
            Self::FreshBiometric => AssuranceLevel::FreshBiometric,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssuranceContextMode {
    Request,
    Action,
}

impl AssuranceContextMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Action => "action",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedAssuranceProof {
    pub context_mode: AssuranceContextMode,
    pub challenge: [u8; 32],
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub requested_assurance: RequestedAssurance,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssuranceProofInput {
    Absent,
    Invalid,
    Presented(PresentedAssuranceProof),
}

pub enum AssuranceContext<'a> {
    Request(&'a [u8]),
    Action(&'a DeviceActionIntent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssuranceProofBinding {
    pub device_id: Uuid,
    pub grant_id: Uuid,
    pub revocation_epoch: u64,
    pub authorization_key_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalAssuranceProof {
    pub device_id: Uuid,
    pub grant_id: Uuid,
    pub revocation_epoch: u64,
    pub authorization_key_id: String,
    pub context_mode: AssuranceContextMode,
    pub context_digest: [u8; 32],
    pub challenge: [u8; 32],
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub requested_assurance: RequestedAssurance,
}

impl CanonicalAssuranceProof {
    pub fn for_request(
        binding: AssuranceProofBinding,
        proof: &PresentedAssuranceProof,
        canonical_request: &[u8],
    ) -> Result<Self, AssuranceError> {
        if proof.context_mode != AssuranceContextMode::Request {
            return Err(AssuranceError::InvalidContext);
        }
        Self::new(
            binding,
            proof,
            AssuranceContextMode::Request,
            Sha256::digest(canonical_request).into(),
        )
    }

    pub fn for_action(
        binding: AssuranceProofBinding,
        proof: &PresentedAssuranceProof,
        intent: &DeviceActionIntent,
    ) -> Result<Self, AssuranceError> {
        if proof.context_mode != AssuranceContextMode::Action
            || proof.expires_at > intent.expires_at
        {
            return Err(AssuranceError::InvalidTimeWindow);
        }
        let canonical = intent
            .canonical_bytes()
            .map_err(|_| AssuranceError::InvalidContext)?;
        Self::new(
            binding,
            proof,
            AssuranceContextMode::Action,
            Sha256::digest(canonical).into(),
        )
    }

    fn new(
        binding: AssuranceProofBinding,
        proof: &PresentedAssuranceProof,
        context_mode: AssuranceContextMode,
        context_digest: [u8; 32],
    ) -> Result<Self, AssuranceError> {
        validate_key_id(&binding.authorization_key_id)?;
        validate_proof_lifetime(proof.issued_at, proof.expires_at)?;
        Ok(Self {
            device_id: binding.device_id,
            grant_id: binding.grant_id,
            revocation_epoch: binding.revocation_epoch,
            authorization_key_id: binding.authorization_key_id,
            context_mode,
            context_digest,
            challenge: proof.challenge,
            issued_at: proof.issued_at,
            expires_at: proof.expires_at,
            requested_assurance: proof.requested_assurance,
        })
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let issued_at = self
            .issued_at
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let expires_at = self
            .expires_at
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let revocation_epoch = self.revocation_epoch.to_be_bytes();
        let mut encoded = ASSURANCE_DOMAIN.to_vec();
        for field in [
            self.device_id.as_bytes().as_slice(),
            self.grant_id.as_bytes().as_slice(),
            revocation_epoch.as_slice(),
            self.authorization_key_id.as_bytes(),
            self.context_mode.as_str().as_bytes(),
            self.context_digest.as_slice(),
            self.challenge.as_slice(),
            issued_at.as_bytes(),
            expires_at.as_bytes(),
            self.requested_assurance.as_str().as_bytes(),
        ] {
            encoded.extend_from_slice(&(field.len() as u32).to_be_bytes());
            encoded.extend_from_slice(field);
        }
        encoded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssuranceError {
    InvalidEncoding,
    InvalidContext,
    InvalidTimeWindow,
    NotYetValid,
    Expired,
    SignatureInvalid,
    ChallengeUnknown,
    ChallengeBindingMismatch,
    ChallengeSpent,
    ChallengeExpired,
    ChallengeUnspent,
    StoreUnavailable,
}

impl fmt::Display for AssuranceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidEncoding => "assurance proof encoding is invalid",
            Self::InvalidContext => "assurance proof context is invalid",
            Self::InvalidTimeWindow => "assurance proof time window is invalid",
            Self::NotYetValid => "assurance proof is not yet valid",
            Self::Expired => "assurance proof expired",
            Self::SignatureInvalid => "assurance proof signature is invalid",
            Self::ChallengeUnknown => "assurance challenge is unknown",
            Self::ChallengeBindingMismatch => "assurance challenge binding did not match",
            Self::ChallengeSpent => "assurance challenge was already spent",
            Self::ChallengeExpired => "assurance challenge expired",
            Self::ChallengeUnspent => "assurance challenge was not spent",
            Self::StoreUnavailable => "assurance challenge store is unavailable",
        })
    }
}

impl std::error::Error for AssuranceError {}

pub fn effective_assurance(
    assurance_class: AssuranceClass,
    requested_assurance: RequestedAssurance,
) -> AssuranceLevel {
    requested_assurance.level().min(assurance_class.ceiling())
}

pub fn validate_proof_window(
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
    challenge_expires_at: DateTime<Utc>,
    action_expires_at: Option<DateTime<Utc>>,
) -> Result<(), AssuranceError> {
    validate_proof_lifetime(issued_at, expires_at)?;
    if issued_at > now {
        return Err(AssuranceError::NotYetValid);
    }
    if now > expires_at {
        return Err(AssuranceError::Expired);
    }
    if expires_at > challenge_expires_at
        || action_expires_at.is_some_and(|action_expires_at| expires_at > action_expires_at)
    {
        return Err(AssuranceError::InvalidTimeWindow);
    }
    Ok(())
}

fn validate_proof_lifetime(
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) -> Result<(), AssuranceError> {
    let lifetime = expires_at.signed_duration_since(issued_at);
    if lifetime.num_seconds() <= 0 || lifetime.num_seconds() > MAX_ASSURANCE_LIFETIME_SECONDS {
        return Err(AssuranceError::InvalidTimeWindow);
    }
    Ok(())
}

fn validate_key_id(value: &str) -> Result<(), AssuranceError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AssuranceError::InvalidEncoding)?;
    if decoded.len() != 32 || URL_SAFE_NO_PAD.encode(decoded) != value {
        return Err(AssuranceError::InvalidEncoding);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChallengeBinding {
    pub device_id: Uuid,
    pub grant_id: Uuid,
    pub revocation_epoch: u64,
    pub authorization_key_id: String,
    pub authorization_key_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedAssuranceProof {
    pub effective_assurance: AssuranceLevel,
    pub binding: ChallengeBinding,
    pub challenge: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssuedAssuranceChallenge {
    pub challenge: [u8; 32],
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssuranceChallengeState {
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredAssuranceChallenge {
    challenge: String,
    device_id: Uuid,
    grant_id: Uuid,
    revocation_epoch: u64,
    authorization_key_id: String,
    authorization_key_epoch: u64,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    spent_at: Option<DateTime<Utc>>,
}

impl StoredAssuranceChallenge {
    fn binding_matches(&self, binding: &ChallengeBinding) -> bool {
        self.device_id == binding.device_id
            && self.grant_id == binding.grant_id
            && self.revocation_epoch == binding.revocation_epoch
            && self.authorization_key_id == binding.authorization_key_id
            && self.authorization_key_epoch == binding.authorization_key_epoch
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredAssuranceChallenges {
    version: u16,
    challenges: Vec<StoredAssuranceChallenge>,
}

pub struct AssuranceChallengeStore {
    path: PathBuf,
}

impl AssuranceChallengeStore {
    pub fn load(coven_home: &Path) -> Result<Self> {
        let path = ensure_private_mobile_dir(coven_home)?.join(ASSURANCE_CHALLENGES_FILE);
        let _guard = CHALLENGE_STORE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile assurance challenge lock poisoned"))?;
        let _file_lock = AssuranceStoreFileLock::acquire(&path, ASSURANCE_CHALLENGE_LOCK_FILE)?;
        read_challenges(&path)?;
        Ok(Self { path })
    }

    pub fn issue(
        &self,
        binding: ChallengeBinding,
        now: DateTime<Utc>,
    ) -> Result<IssuedAssuranceChallenge, AssuranceError> {
        validate_challenge_binding(&binding)?;
        let _guard = CHALLENGE_STORE_LOCK
            .lock()
            .map_err(|_| AssuranceError::StoreUnavailable)?;
        let _file_lock = AssuranceStoreFileLock::acquire(&self.path, ASSURANCE_CHALLENGE_LOCK_FILE)
            .map_err(|_| AssuranceError::StoreUnavailable)?;
        let mut challenges =
            read_challenges(&self.path).map_err(|_| AssuranceError::StoreUnavailable)?;
        challenges.retain(|challenge| {
            challenge.expires_at + chrono::Duration::seconds(SPENT_CHALLENGE_RETENTION_SECONDS)
                > now
        });
        if challenges.len() >= MAX_ASSURANCE_CHALLENGES {
            return Err(AssuranceError::StoreUnavailable);
        }
        let challenge = loop {
            let candidate = rand::random::<[u8; 32]>();
            let encoded = URL_SAFE_NO_PAD.encode(candidate);
            if !challenges
                .iter()
                .any(|existing| existing.challenge == encoded)
            {
                break candidate;
            }
        };
        let expires_at = now + chrono::Duration::seconds(DEFAULT_CHALLENGE_LIFETIME_SECONDS);
        challenges.push(StoredAssuranceChallenge {
            challenge: URL_SAFE_NO_PAD.encode(challenge),
            device_id: binding.device_id,
            grant_id: binding.grant_id,
            revocation_epoch: binding.revocation_epoch,
            authorization_key_id: binding.authorization_key_id,
            authorization_key_epoch: binding.authorization_key_epoch,
            issued_at: now,
            expires_at,
            spent_at: None,
        });
        write_challenges(&self.path, &challenges).map_err(|_| AssuranceError::StoreUnavailable)?;
        Ok(IssuedAssuranceChallenge {
            challenge,
            expires_at,
        })
    }

    pub fn consume(
        &self,
        challenge: [u8; 32],
        binding: &ChallengeBinding,
        now: DateTime<Utc>,
    ) -> Result<(), AssuranceError> {
        validate_challenge_binding(binding)?;
        let _guard = CHALLENGE_STORE_LOCK
            .lock()
            .map_err(|_| AssuranceError::StoreUnavailable)?;
        let _file_lock = AssuranceStoreFileLock::acquire(&self.path, ASSURANCE_CHALLENGE_LOCK_FILE)
            .map_err(|_| AssuranceError::StoreUnavailable)?;
        let mut challenges =
            read_challenges(&self.path).map_err(|_| AssuranceError::StoreUnavailable)?;
        let encoded = URL_SAFE_NO_PAD.encode(challenge);
        let record = challenges
            .iter_mut()
            .find(|record| record.challenge == encoded)
            .ok_or(AssuranceError::ChallengeUnknown)?;
        if !record.binding_matches(binding) {
            return Err(AssuranceError::ChallengeBindingMismatch);
        }
        if record.spent_at.is_some() {
            return Err(AssuranceError::ChallengeSpent);
        }
        if now > record.expires_at {
            return Err(AssuranceError::ChallengeExpired);
        }
        record.spent_at = Some(now);
        write_challenges(&self.path, &challenges).map_err(|_| AssuranceError::StoreUnavailable)
    }

    pub fn inspect(
        &self,
        challenge: [u8; 32],
        binding: &ChallengeBinding,
        now: DateTime<Utc>,
    ) -> Result<AssuranceChallengeState, AssuranceError> {
        validate_challenge_binding(binding)?;
        let _guard = CHALLENGE_STORE_LOCK
            .lock()
            .map_err(|_| AssuranceError::StoreUnavailable)?;
        let _file_lock = AssuranceStoreFileLock::acquire(&self.path, ASSURANCE_CHALLENGE_LOCK_FILE)
            .map_err(|_| AssuranceError::StoreUnavailable)?;
        let challenges =
            read_challenges(&self.path).map_err(|_| AssuranceError::StoreUnavailable)?;
        let encoded = URL_SAFE_NO_PAD.encode(challenge);
        let record = challenges
            .iter()
            .find(|record| record.challenge == encoded)
            .ok_or(AssuranceError::ChallengeUnknown)?;
        if !record.binding_matches(binding) {
            return Err(AssuranceError::ChallengeBindingMismatch);
        }
        if record.spent_at.is_some() {
            return Err(AssuranceError::ChallengeSpent);
        }
        if now > record.expires_at {
            return Err(AssuranceError::ChallengeExpired);
        }
        Ok(AssuranceChallengeState {
            issued_at: record.issued_at,
            expires_at: record.expires_at,
        })
    }

    pub fn ensure_consumed(
        &self,
        challenge: [u8; 32],
        binding: &ChallengeBinding,
    ) -> Result<(), AssuranceError> {
        validate_challenge_binding(binding)?;
        let _guard = CHALLENGE_STORE_LOCK
            .lock()
            .map_err(|_| AssuranceError::StoreUnavailable)?;
        let _file_lock = AssuranceStoreFileLock::acquire(&self.path, ASSURANCE_CHALLENGE_LOCK_FILE)
            .map_err(|_| AssuranceError::StoreUnavailable)?;
        let challenges =
            read_challenges(&self.path).map_err(|_| AssuranceError::StoreUnavailable)?;
        let encoded = URL_SAFE_NO_PAD.encode(challenge);
        let record = challenges
            .iter()
            .find(|record| record.challenge == encoded)
            .ok_or(AssuranceError::ChallengeUnknown)?;
        if !record.binding_matches(binding) {
            return Err(AssuranceError::ChallengeBindingMismatch);
        }
        if record.spent_at.is_none() {
            return Err(AssuranceError::ChallengeUnspent);
        }
        Ok(())
    }
}

pub fn verify_and_consume_assurance(
    store: &AssuranceChallengeStore,
    key: &DeviceAuthorizationKeyRecord,
    grant_id: Uuid,
    revocation_epoch: u64,
    proof: &PresentedAssuranceProof,
    context: AssuranceContext<'_>,
    now: DateTime<Utc>,
) -> Result<VerifiedAssuranceProof, AssuranceError> {
    if key.revoked_at.is_some() {
        return Err(AssuranceError::InvalidEncoding);
    }
    let binding = ChallengeBinding {
        device_id: key.device_id,
        grant_id,
        revocation_epoch,
        authorization_key_id: key.subject_key_id.clone(),
        authorization_key_epoch: key.key_epoch,
    };
    let challenge = store.inspect(proof.challenge, &binding, now)?;
    if proof.issued_at < challenge.issued_at {
        return Err(AssuranceError::InvalidTimeWindow);
    }
    let (canonical, action_expires_at) = match context {
        AssuranceContext::Request(request) => (
            CanonicalAssuranceProof::for_request(
                AssuranceProofBinding {
                    device_id: key.device_id,
                    grant_id,
                    revocation_epoch,
                    authorization_key_id: key.subject_key_id.clone(),
                },
                proof,
                request,
            )?,
            None,
        ),
        AssuranceContext::Action(intent) => (
            CanonicalAssuranceProof::for_action(
                AssuranceProofBinding {
                    device_id: key.device_id,
                    grant_id,
                    revocation_epoch,
                    authorization_key_id: key.subject_key_id.clone(),
                },
                proof,
                intent,
            )?,
            Some(intent.expires_at),
        ),
    };
    validate_proof_window(
        proof.issued_at,
        proof.expires_at,
        now,
        challenge.expires_at,
        action_expires_at,
    )?;
    verify_assurance_signature(
        &key.public_key_x963,
        &canonical.canonical_bytes(),
        &proof.signature,
    )?;
    store.consume(proof.challenge, &binding, now)?;
    Ok(VerifiedAssuranceProof {
        effective_assurance: effective_assurance(key.assurance_class, proof.requested_assurance),
        binding,
        challenge: proof.challenge,
    })
}

fn verify_assurance_signature(
    public_key_x963: &str,
    canonical: &[u8],
    signature_b64url: &str,
) -> Result<(), AssuranceError> {
    if signature_b64url.len() > 128 {
        return Err(AssuranceError::SignatureInvalid);
    }
    let public_key = URL_SAFE_NO_PAD
        .decode(public_key_x963)
        .map_err(|_| AssuranceError::SignatureInvalid)?;
    let verifying_key =
        VerifyingKey::from_sec1_bytes(&public_key).map_err(|_| AssuranceError::SignatureInvalid)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature_b64url)
        .map_err(|_| AssuranceError::SignatureInvalid)?;
    if URL_SAFE_NO_PAD.encode(&signature) != signature_b64url {
        return Err(AssuranceError::SignatureInvalid);
    }
    let signature =
        Signature::from_der(&signature).map_err(|_| AssuranceError::SignatureInvalid)?;
    verifying_key
        .verify(canonical, &signature)
        .map_err(|_| AssuranceError::SignatureInvalid)
}

fn validate_challenge_binding(binding: &ChallengeBinding) -> Result<(), AssuranceError> {
    validate_key_id(&binding.authorization_key_id)?;
    if binding.authorization_key_epoch == 0 {
        return Err(AssuranceError::InvalidEncoding);
    }
    Ok(())
}

fn read_challenges(path: &Path) -> Result<Vec<StoredAssuranceChallenge>> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_private_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }
    let stored: StoredAssuranceChallenges = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("failed to read {}", path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", path.display()))?;
    if stored.version != ASSURANCE_CHALLENGE_STORE_VERSION {
        bail!("unsupported mobile assurance challenge store version");
    }
    validate_challenges(&stored.challenges)?;
    Ok(stored.challenges)
}

fn write_challenges(path: &Path, challenges: &[StoredAssuranceChallenge]) -> Result<()> {
    validate_challenges(challenges)?;
    let stored = StoredAssuranceChallenges {
        version: ASSURANCE_CHALLENGE_STORE_VERSION,
        challenges: challenges.to_vec(),
    };
    let mut encoded =
        serde_json::to_vec_pretty(&stored).context("failed to encode assurance challenges")?;
    encoded.push(b'\n');
    atomic_replace_private(path, &encoded)
}

fn validate_challenges(challenges: &[StoredAssuranceChallenge]) -> Result<()> {
    if challenges.len() > MAX_ASSURANCE_CHALLENGES {
        bail!("mobile assurance challenge store exceeds the record limit");
    }
    for (index, challenge) in challenges.iter().enumerate() {
        let decoded = URL_SAFE_NO_PAD
            .decode(&challenge.challenge)
            .context("mobile assurance challenge is not valid base64url")?;
        if decoded.len() != 32 || URL_SAFE_NO_PAD.encode(decoded) != challenge.challenge {
            bail!("mobile assurance challenge is not canonical");
        }
        validate_key_id(&challenge.authorization_key_id)
            .map_err(|_| anyhow::anyhow!("mobile assurance key id is invalid"))?;
        if challenge.authorization_key_epoch == 0 {
            bail!("mobile assurance key epoch must be positive");
        }
        validate_proof_lifetime(challenge.issued_at, challenge.expires_at)
            .map_err(|_| anyhow::anyhow!("mobile assurance challenge window is invalid"))?;
        if challenge
            .spent_at
            .is_some_and(|spent_at| spent_at < challenge.issued_at)
        {
            bail!("mobile assurance challenge spend predates issuance");
        }
        if challenges[..index]
            .iter()
            .any(|existing| existing.challenge == challenge.challenge)
        {
            bail!("mobile assurance challenge store contains duplicates");
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepUpAuthorizationEnrollment {
    pub public_key: String,
    pub assurance_class: AssuranceClass,
    pub enrollment_signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceAuthorizationKeyRecord {
    pub device_id: Uuid,
    pub public_key_x963: String,
    pub subject_key_id: String,
    pub assurance_class: AssuranceClass,
    pub enrolled_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub key_epoch: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct NewAuthorizationKey {
    pub public_key_x963: String,
    pub assurance_class: AssuranceClass,
    pub enrolled_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredAuthorizationKeys {
    version: u16,
    keys: Vec<DeviceAuthorizationKeyRecord>,
}

pub(crate) struct AuthorizationKeyRegistry {
    path: PathBuf,
    keys: RwLock<Vec<DeviceAuthorizationKeyRecord>>,
}

impl AuthorizationKeyRegistry {
    pub fn load(coven_home: &Path) -> Result<Self> {
        let path = ensure_private_mobile_dir(coven_home)?.join(AUTHORIZATION_KEYS_FILE);
        let _guard = AUTHORIZATION_KEY_STORE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key store lock poisoned"))?;
        let _file_lock = AssuranceStoreFileLock::acquire(&path, AUTHORIZATION_KEY_LOCK_FILE)?;
        let keys = read_authorization_keys(&path)?;
        Ok(Self {
            path,
            keys: RwLock::new(keys),
        })
    }

    pub fn active(&self, device_id: Uuid) -> Result<Option<DeviceAuthorizationKeyRecord>> {
        let _guard = AUTHORIZATION_KEY_STORE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key store lock poisoned"))?;
        let _file_lock = AssuranceStoreFileLock::acquire(&self.path, AUTHORIZATION_KEY_LOCK_FILE)?;
        let loaded = read_authorization_keys(&self.path)?;
        let active = loaded
            .iter()
            .find(|key| key.device_id == device_id && key.revoked_at.is_none())
            .cloned();
        *self
            .keys
            .write()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key registry lock poisoned"))? =
            loaded;
        Ok(active)
    }

    pub fn enroll_initial(
        &self,
        device_id: Uuid,
        possession_public_key_x963: &str,
        key: NewAuthorizationKey,
    ) -> Result<DeviceAuthorizationKeyRecord> {
        validate_public_key(&key.public_key_x963)?;
        if key.public_key_x963 == possession_public_key_x963 {
            bail!("mobile authorization key must differ from the possession key");
        }
        let _guard = AUTHORIZATION_KEY_STORE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key store lock poisoned"))?;
        let _file_lock = AssuranceStoreFileLock::acquire(&self.path, AUTHORIZATION_KEY_LOCK_FILE)?;
        let keys = read_authorization_keys(&self.path)?;
        if keys.len() >= MAX_AUTHORIZATION_KEY_RECORDS {
            bail!("mobile authorization-key registry is full");
        }
        if keys.iter().any(|existing| existing.device_id == device_id) {
            bail!("mobile device already has authorization-key history");
        }
        let record = DeviceAuthorizationKeyRecord {
            device_id,
            subject_key_id: authorization_key_id(&key.public_key_x963)?,
            public_key_x963: key.public_key_x963,
            assurance_class: key.assurance_class,
            enrolled_at: key.enrolled_at,
            revoked_at: None,
            key_epoch: 1,
        };
        let mut updated = keys.clone();
        updated.push(record.clone());
        validate_authorization_keys(&updated)?;
        write_authorization_keys(&self.path, &updated)?;
        *self
            .keys
            .write()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key registry lock poisoned"))? =
            updated;
        Ok(record)
    }

    #[cfg(test)]
    fn rotate(
        &self,
        device_id: Uuid,
        possession_public_key_x963: &str,
        key: NewAuthorizationKey,
    ) -> Result<DeviceAuthorizationKeyRecord> {
        validate_public_key(&key.public_key_x963)?;
        if key.public_key_x963 == possession_public_key_x963 {
            bail!("mobile authorization key must differ from the possession key");
        }
        let _guard = AUTHORIZATION_KEY_STORE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key store lock poisoned"))?;
        let _file_lock = AssuranceStoreFileLock::acquire(&self.path, AUTHORIZATION_KEY_LOCK_FILE)?;
        let keys = read_authorization_keys(&self.path)?;
        if keys.len() >= MAX_AUTHORIZATION_KEY_RECORDS {
            bail!("mobile authorization-key registry is full");
        }
        let active_index = keys
            .iter()
            .position(|existing| existing.device_id == device_id && existing.revoked_at.is_none());
        let latest = keys
            .iter()
            .filter(|existing| existing.device_id == device_id)
            .max_by_key(|existing| existing.key_epoch)
            .context("mobile device has no authorization-key history")?;
        let earliest_rotation = latest.revoked_at.unwrap_or(latest.enrolled_at);
        if key.enrolled_at < earliest_rotation {
            bail!("mobile authorization-key rotation predates the active key");
        }
        let key_epoch = latest
            .key_epoch
            .checked_add(1)
            .context("mobile authorization-key epoch overflow")?;
        let record = DeviceAuthorizationKeyRecord {
            device_id,
            subject_key_id: authorization_key_id(&key.public_key_x963)?,
            public_key_x963: key.public_key_x963,
            assurance_class: key.assurance_class,
            enrolled_at: key.enrolled_at,
            revoked_at: None,
            key_epoch,
        };
        let mut updated = keys.clone();
        if let Some(active_index) = active_index {
            updated[active_index].revoked_at = Some(record.enrolled_at);
        }
        updated.push(record.clone());
        validate_authorization_keys(&updated)?;
        write_authorization_keys(&self.path, &updated)?;
        *self
            .keys
            .write()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key registry lock poisoned"))? =
            updated;
        Ok(record)
    }

    pub fn revoke(&self, device_id: Uuid, revoked_at: DateTime<Utc>) -> Result<()> {
        let _guard = AUTHORIZATION_KEY_STORE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key store lock poisoned"))?;
        let _file_lock = AssuranceStoreFileLock::acquire(&self.path, AUTHORIZATION_KEY_LOCK_FILE)?;
        let keys = read_authorization_keys(&self.path)?;
        let Some(active_index) = keys
            .iter()
            .position(|existing| existing.device_id == device_id && existing.revoked_at.is_none())
        else {
            return Ok(());
        };
        if revoked_at < keys[active_index].enrolled_at {
            bail!("mobile authorization-key revocation predates enrollment");
        }
        let mut updated = keys.clone();
        updated[active_index].revoked_at = Some(revoked_at);
        validate_authorization_keys(&updated)?;
        write_authorization_keys(&self.path, &updated)?;
        *self
            .keys
            .write()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key registry lock poisoned"))? =
            updated;
        Ok(())
    }

    pub fn history(&self, device_id: Uuid) -> Result<Vec<DeviceAuthorizationKeyRecord>> {
        let _guard = AUTHORIZATION_KEY_STORE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key store lock poisoned"))?;
        let _file_lock = AssuranceStoreFileLock::acquire(&self.path, AUTHORIZATION_KEY_LOCK_FILE)?;
        let loaded = read_authorization_keys(&self.path)?;
        let history = loaded
            .iter()
            .filter(|key| key.device_id == device_id)
            .cloned()
            .collect();
        *self
            .keys
            .write()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key registry lock poisoned"))? =
            loaded;
        Ok(history)
    }

    pub fn forget_all(&self) -> Result<()> {
        let _guard = AUTHORIZATION_KEY_STORE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key store lock poisoned"))?;
        let _file_lock = AssuranceStoreFileLock::acquire(&self.path, AUTHORIZATION_KEY_LOCK_FILE)?;
        write_authorization_keys(&self.path, &[])?;
        self.keys
            .write()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key registry lock poisoned"))?
            .clear();
        Ok(())
    }

    pub fn remove_device(&self, device_id: Uuid) -> Result<()> {
        let _guard = AUTHORIZATION_KEY_STORE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key store lock poisoned"))?;
        let _file_lock = AssuranceStoreFileLock::acquire(&self.path, AUTHORIZATION_KEY_LOCK_FILE)?;
        let keys = read_authorization_keys(&self.path)?;
        let updated: Vec<_> = keys
            .into_iter()
            .filter(|key| key.device_id != device_id)
            .collect();
        write_authorization_keys(&self.path, &updated)?;
        *self
            .keys
            .write()
            .map_err(|_| anyhow::anyhow!("mobile authorization-key registry lock poisoned"))? =
            updated;
        Ok(())
    }
}

pub(crate) fn authorization_key_id(public_key_x963: &str) -> Result<String> {
    let key = validate_public_key(public_key_x963)?;
    Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(key)))
}

pub(crate) fn validate_public_key(public_key_x963: &str) -> Result<Vec<u8>> {
    let key = URL_SAFE_NO_PAD
        .decode(public_key_x963)
        .context("mobile authorization key is not valid base64url")?;
    if key.len() != 65
        || key.first() != Some(&4)
        || URL_SAFE_NO_PAD.encode(&key) != public_key_x963
        || p256::PublicKey::from_sec1_bytes(&key).is_err()
    {
        bail!("mobile authorization key is not a canonical P-256 X9.63 key");
    }
    Ok(key)
}

fn read_authorization_keys(path: &Path) -> Result<Vec<DeviceAuthorizationKeyRecord>> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_private_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }
    let stored: StoredAuthorizationKeys = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("failed to read {}", path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", path.display()))?;
    if stored.version != AUTHORIZATION_KEY_REGISTRY_VERSION {
        bail!("unsupported mobile authorization-key registry version");
    }
    validate_authorization_keys(&stored.keys)?;
    Ok(stored.keys)
}

fn write_authorization_keys(path: &Path, keys: &[DeviceAuthorizationKeyRecord]) -> Result<()> {
    validate_authorization_keys(keys)?;
    let stored = StoredAuthorizationKeys {
        version: AUTHORIZATION_KEY_REGISTRY_VERSION,
        keys: keys.to_vec(),
    };
    let mut encoded = serde_json::to_vec_pretty(&stored)
        .context("failed to encode mobile authorization-key registry")?;
    encoded.push(b'\n');
    atomic_replace_private(path, &encoded)
}

fn validate_authorization_keys(keys: &[DeviceAuthorizationKeyRecord]) -> Result<()> {
    if keys.len() > MAX_AUTHORIZATION_KEY_RECORDS {
        bail!("mobile authorization-key registry exceeds the record limit");
    }
    for (index, key) in keys.iter().enumerate() {
        validate_public_key(&key.public_key_x963)?;
        if key.subject_key_id != authorization_key_id(&key.public_key_x963)? {
            bail!("mobile authorization-key subject id is invalid");
        }
        if key.key_epoch == 0 {
            bail!("mobile authorization-key epoch must be positive");
        }
        if key
            .revoked_at
            .is_some_and(|revoked_at| revoked_at < key.enrolled_at)
        {
            bail!("mobile authorization-key revocation cannot predate enrollment");
        }
        if keys[..index].iter().any(|existing| {
            existing.public_key_x963 == key.public_key_x963
                || existing.subject_key_id == key.subject_key_id
        }) {
            bail!("mobile authorization-key registry contains duplicate keys");
        }
        if keys[..index].iter().any(|existing| {
            existing.device_id == key.device_id && existing.key_epoch == key.key_epoch
        }) {
            bail!("mobile authorization-key registry contains duplicate epochs");
        }
        if key.revoked_at.is_none()
            && keys[..index].iter().any(|existing| {
                existing.device_id == key.device_id && existing.revoked_at.is_none()
            })
        {
            bail!("mobile device has multiple active authorization keys");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::grant::{DeviceActionIntent, DeviceScope, DEVICE_ACTION_VERSION};
    use super::*;
    use chrono::Duration;
    use p256::ecdsa::signature::{Signer, Verifier};
    use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
    use p256::elliptic_curve::sec1::ToEncodedPoint;

    fn public_key(seed: u8) -> String {
        let signing_key = p256::SecretKey::from_slice(&[seed; 32]).unwrap();
        URL_SAFE_NO_PAD.encode(signing_key.public_key().to_encoded_point(false).as_bytes())
    }

    fn new_key(
        seed: u8,
        assurance_class: AssuranceClass,
        enrolled_at: DateTime<Utc>,
    ) -> NewAuthorizationKey {
        NewAuthorizationKey {
            public_key_x963: public_key(seed),
            assurance_class,
            enrolled_at,
        }
    }

    fn decode_hex(value: &str) -> Vec<u8> {
        assert_eq!(value.len() % 2, 0);
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let high = char::from(pair[0]).to_digit(16).unwrap();
                let low = char::from(pair[1]).to_digit(16).unwrap();
                ((high << 4) | low) as u8
            })
            .collect()
    }

    fn encode_hex(value: &[u8]) -> String {
        value.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn action_intent() -> DeviceActionIntent {
        let issued_at = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        DeviceActionIntent {
            version: DEVICE_ACTION_VERSION,
            scope: DeviceScope::ToolExecutionApprove,
            operation: "deploy.production".to_owned(),
            target: "OpenCoven/psyche@synthetic".to_owned(),
            effect_digest: URL_SAFE_NO_PAD.encode([7; 32]),
            nonce: URL_SAFE_NO_PAD.encode([8; 32]),
            issued_at,
            expires_at: issued_at + Duration::seconds(90),
        }
    }

    fn proof_binding(
        device_id: Uuid,
        grant_id: Uuid,
        revocation_epoch: u64,
    ) -> AssuranceProofBinding {
        AssuranceProofBinding {
            device_id,
            grant_id,
            revocation_epoch,
            authorization_key_id: URL_SAFE_NO_PAD.encode([4; 32]),
        }
    }

    fn presented_proof(
        context_mode: AssuranceContextMode,
        challenge: [u8; 32],
        issued_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
        requested_assurance: RequestedAssurance,
    ) -> PresentedAssuranceProof {
        PresentedAssuranceProof {
            context_mode,
            challenge,
            issued_at,
            expires_at,
            requested_assurance,
            signature: String::new(),
        }
    }

    #[test]
    fn authorization_key_rotation_and_revocation_survive_reload() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        let device_id = Uuid::from_u128(1);
        let possession_key = public_key(1);
        let registry = AuthorizationKeyRegistry::load(temp.path()).unwrap();
        let first = registry
            .enroll_initial(
                device_id,
                &possession_key,
                new_key(2, AssuranceClass::BiometricOnly, now),
            )
            .unwrap();
        let second = registry
            .rotate(
                device_id,
                &possession_key,
                new_key(
                    3,
                    AssuranceClass::DeviceCredential,
                    now + Duration::minutes(1),
                ),
            )
            .unwrap();
        assert_eq!(first.key_epoch, 1);
        assert_eq!(second.key_epoch, 2);
        assert_eq!(
            registry.active(device_id).unwrap().unwrap().subject_key_id,
            second.subject_key_id
        );

        registry
            .revoke(device_id, now + Duration::minutes(2))
            .unwrap();
        assert!(registry.active(device_id).unwrap().is_none());
        let third = registry
            .rotate(
                device_id,
                &possession_key,
                new_key(
                    4,
                    AssuranceClass::UserVerification,
                    now + Duration::minutes(3),
                ),
            )
            .unwrap();
        assert_eq!(third.key_epoch, 3);
        assert_eq!(
            registry.active(device_id).unwrap().unwrap().subject_key_id,
            third.subject_key_id
        );
        registry
            .revoke(device_id, now + Duration::minutes(4))
            .unwrap();
        let reloaded = AuthorizationKeyRegistry::load(temp.path()).unwrap();
        let history = reloaded.history(device_id).unwrap();
        assert_eq!(history.len(), 3);
        assert!(history.iter().all(|record| record.revoked_at.is_some()));
        assert_eq!(history[0].key_epoch, 1);
        assert_eq!(history[1].key_epoch, 2);
        assert_eq!(history[2].key_epoch, 3);
    }

    #[test]
    fn authorization_key_revocation_is_visible_to_existing_handles() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        let device_id = Uuid::from_u128(1);
        let possession_key = public_key(1);
        let first = AuthorizationKeyRegistry::load(temp.path()).unwrap();
        first
            .enroll_initial(
                device_id,
                &possession_key,
                new_key(2, AssuranceClass::BiometricOnly, now),
            )
            .unwrap();
        assert!(first.active(device_id).unwrap().is_some());

        let second = AuthorizationKeyRegistry::load(temp.path()).unwrap();
        second
            .revoke(device_id, now + Duration::seconds(1))
            .unwrap();

        assert!(first.active(device_id).unwrap().is_none());
    }

    #[test]
    fn authorization_registry_rejects_same_key_and_corruption_without_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        let possession_key = public_key(1);
        let registry = AuthorizationKeyRegistry::load(temp.path()).unwrap();
        assert!(registry
            .enroll_initial(
                Uuid::from_u128(1),
                &possession_key,
                NewAuthorizationKey {
                    public_key_x963: possession_key.clone(),
                    assurance_class: AssuranceClass::UserVerification,
                    enrolled_at: now,
                },
            )
            .is_err());

        let path = temp.path().join("mobile").join(AUTHORIZATION_KEYS_FILE);
        atomic_replace_private(&path, b"{not valid json}\n").unwrap();
        let before = fs::read(&path).unwrap();
        assert!(AuthorizationKeyRegistry::load(temp.path()).is_err());
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn authorization_registry_rejects_oversized_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = ensure_private_mobile_dir(temp.path())
            .unwrap()
            .join(AUTHORIZATION_KEYS_FILE);
        let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        let template = DeviceAuthorizationKeyRecord {
            device_id: Uuid::from_u128(1),
            public_key_x963: public_key(2),
            subject_key_id: authorization_key_id(&public_key(2)).unwrap(),
            assurance_class: AssuranceClass::BiometricOnly,
            enrolled_at: now,
            revoked_at: Some(now),
            key_epoch: 1,
        };
        let stored = StoredAuthorizationKeys {
            version: AUTHORIZATION_KEY_REGISTRY_VERSION,
            keys: vec![template; MAX_AUTHORIZATION_KEY_RECORDS + 1],
        };
        atomic_replace_private(&path, &serde_json::to_vec(&stored).unwrap()).unwrap();
        assert!(AuthorizationKeyRegistry::load(temp.path()).is_err());
    }

    #[test]
    fn portable_assurance_vector_matches_canonical_bytes_and_signature() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/mobile-assurance-v1/assurance-vector.json"
        ))
        .unwrap();
        let protected_request =
            decode_hex(vector["protectedCanonicalRequestHex"].as_str().unwrap());
        let authorization_key_id = URL_SAFE_NO_PAD.encode(decode_hex(
            vector["authorizationKeyIdHex"].as_str().unwrap(),
        ));
        let proof = CanonicalAssuranceProof::for_request(
            AssuranceProofBinding {
                device_id: Uuid::parse_str(vector["deviceId"].as_str().unwrap()).unwrap(),
                grant_id: Uuid::parse_str(vector["grantId"].as_str().unwrap()).unwrap(),
                revocation_epoch: vector["revocationEpoch"].as_u64().unwrap(),
                authorization_key_id,
            },
            &presented_proof(
                AssuranceContextMode::Request,
                decode_hex(vector["challengeHex"].as_str().unwrap())
                    .try_into()
                    .unwrap(),
                vector["issuedAt"].as_str().unwrap().parse().unwrap(),
                vector["expiresAt"].as_str().unwrap().parse().unwrap(),
                RequestedAssurance::FreshBiometric,
            ),
            &protected_request,
        )
        .unwrap();
        let canonical = proof.canonical_bytes();
        assert_eq!(
            encode_hex(&proof.context_digest),
            vector["contextDigestHex"].as_str().unwrap()
        );
        assert_eq!(
            encode_hex(&canonical),
            vector["canonicalProofBytesHex"].as_str().unwrap()
        );
        let verifying_key = VerifyingKey::from_sec1_bytes(&decode_hex(
            vector["stepUpPublicKeyX963Hex"].as_str().unwrap(),
        ))
        .unwrap();
        let signature =
            Signature::from_der(&decode_hex(vector["signatureDERHex"].as_str().unwrap())).unwrap();
        verifying_key.verify(&canonical, &signature).unwrap();
    }

    #[test]
    fn assurance_proof_binds_request_action_device_and_grant() {
        let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        let request = b"COVEN-MEMORY/1\nGET\n/api/v1/mobile/memory\n1785326400\nnonce\ndigest";
        let request_claims = presented_proof(
            AssuranceContextMode::Request,
            [5; 32],
            now,
            now + Duration::seconds(60),
            RequestedAssurance::FreshUserVerification,
        );
        let request_proof = CanonicalAssuranceProof::for_request(
            proof_binding(Uuid::from_u128(1), Uuid::from_u128(2), 3),
            &request_claims,
            request,
        )
        .unwrap();
        let mut other_request = request.to_vec();
        other_request.push(b'!');
        assert_ne!(
            request_proof.canonical_bytes(),
            CanonicalAssuranceProof::for_request(
                proof_binding(Uuid::from_u128(1), Uuid::from_u128(2), 3),
                &request_claims,
                &other_request,
            )
            .unwrap()
            .canonical_bytes()
        );
        assert_ne!(
            request_proof.canonical_bytes(),
            CanonicalAssuranceProof::for_request(
                proof_binding(Uuid::from_u128(9), Uuid::from_u128(2), 3),
                &request_claims,
                request,
            )
            .unwrap()
            .canonical_bytes()
        );
        assert_ne!(
            request_proof.canonical_bytes(),
            CanonicalAssuranceProof::for_request(
                proof_binding(Uuid::from_u128(1), Uuid::from_u128(9), 3),
                &request_claims,
                request,
            )
            .unwrap()
            .canonical_bytes()
        );
        let mut changed_action = action_intent();
        let action_claims = presented_proof(
            AssuranceContextMode::Action,
            [5; 32],
            now,
            now + Duration::seconds(60),
            RequestedAssurance::FreshUserVerification,
        );
        let action_proof = CanonicalAssuranceProof::for_action(
            proof_binding(Uuid::from_u128(1), Uuid::from_u128(2), 3),
            &action_claims,
            &changed_action,
        )
        .unwrap();
        assert_ne!(
            request_proof.canonical_bytes(),
            action_proof.canonical_bytes()
        );
        changed_action.target.push_str("-substituted");
        assert_ne!(
            action_proof.canonical_bytes(),
            CanonicalAssuranceProof::for_action(
                proof_binding(Uuid::from_u128(1), Uuid::from_u128(2), 3),
                &action_claims,
                &changed_action,
            )
            .unwrap()
            .canonical_bytes()
        );
    }

    #[test]
    fn assurance_windows_and_class_ceilings_fail_closed() {
        let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        let intent = action_intent();
        assert_eq!(
            validate_proof_window(
                now,
                now + Duration::seconds(121),
                now,
                now + Duration::seconds(121),
                None,
            ),
            Err(AssuranceError::InvalidTimeWindow)
        );
        assert_eq!(
            validate_proof_window(
                now + Duration::seconds(1),
                now + Duration::seconds(60),
                now,
                now + Duration::seconds(60),
                None,
            ),
            Err(AssuranceError::NotYetValid)
        );
        assert_eq!(
            CanonicalAssuranceProof::for_action(
                AssuranceProofBinding {
                    device_id: Uuid::from_u128(1),
                    grant_id: Uuid::from_u128(2),
                    revocation_epoch: 0,
                    authorization_key_id: URL_SAFE_NO_PAD.encode([3; 32]),
                },
                &presented_proof(
                    AssuranceContextMode::Action,
                    [4; 32],
                    now,
                    intent.expires_at + Duration::seconds(1),
                    RequestedAssurance::FreshBiometric,
                ),
                &intent,
            )
            .unwrap_err(),
            AssuranceError::InvalidTimeWindow
        );
        assert_eq!(
            effective_assurance(
                AssuranceClass::BiometricOnly,
                RequestedAssurance::FreshBiometric,
            ),
            super::super::grant::AssuranceLevel::FreshBiometric
        );
        assert_eq!(
            effective_assurance(
                AssuranceClass::UserVerification,
                RequestedAssurance::FreshBiometric,
            ),
            super::super::grant::AssuranceLevel::FreshUserVerification
        );
        assert_eq!(
            effective_assurance(
                AssuranceClass::DeviceCredential,
                RequestedAssurance::FreshBiometric,
            ),
            super::super::grant::AssuranceLevel::FreshUserVerification
        );
    }

    fn challenge_binding(key_epoch: u64) -> ChallengeBinding {
        ChallengeBinding {
            device_id: Uuid::from_u128(1),
            grant_id: Uuid::from_u128(2),
            revocation_epoch: 3,
            authorization_key_id: URL_SAFE_NO_PAD.encode([4; 32]),
            authorization_key_epoch: key_epoch,
        }
    }

    #[test]
    fn challenge_spend_is_persistent_and_single_use_across_restart() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        let store = AssuranceChallengeStore::load(temp.path()).unwrap();
        let issued = store.issue(challenge_binding(1), now).unwrap();
        store
            .consume(issued.challenge, &challenge_binding(1), now)
            .unwrap();

        let reloaded = AssuranceChallengeStore::load(temp.path()).unwrap();
        assert_eq!(
            reloaded.consume(issued.challenge, &challenge_binding(1), now),
            Err(AssuranceError::ChallengeSpent)
        );
    }

    #[test]
    fn challenge_binding_substitution_and_expiry_do_not_spend_valid_challenge() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        let store = AssuranceChallengeStore::load(temp.path()).unwrap();
        let issued = store.issue(challenge_binding(1), now).unwrap();
        let mut wrong_device = challenge_binding(1);
        wrong_device.device_id = Uuid::from_u128(9);
        assert_eq!(
            store.consume(issued.challenge, &wrong_device, now),
            Err(AssuranceError::ChallengeBindingMismatch)
        );
        let mut wrong_grant = challenge_binding(1);
        wrong_grant.grant_id = Uuid::from_u128(9);
        assert_eq!(
            store.consume(issued.challenge, &wrong_grant, now),
            Err(AssuranceError::ChallengeBindingMismatch)
        );
        assert_eq!(
            store.consume(issued.challenge, &challenge_binding(2), now),
            Err(AssuranceError::ChallengeBindingMismatch)
        );
        store
            .consume(issued.challenge, &challenge_binding(1), now)
            .unwrap();

        let expired = store.issue(challenge_binding(1), now).unwrap();
        assert_eq!(
            store.consume(
                expired.challenge,
                &challenge_binding(1),
                expired.expires_at + Duration::milliseconds(1),
            ),
            Err(AssuranceError::ChallengeExpired)
        );
    }

    #[test]
    fn concurrent_challenge_spend_has_exactly_one_winner() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        let issued = AssuranceChallengeStore::load(temp.path())
            .unwrap()
            .issue(challenge_binding(1), now)
            .unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let path = temp.path().to_path_buf();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                let store = AssuranceChallengeStore::load(&path).unwrap();
                barrier.wait();
                store.consume(issued.challenge, &challenge_binding(1), now)
            }));
        }
        let accepted = workers
            .into_iter()
            .map(|worker| worker.join().unwrap().is_ok())
            .filter(|accepted| *accepted)
            .count();
        assert_eq!(accepted, 1);
    }

    #[test]
    fn action_verifier_recomputes_intent_and_consumes_only_exact_valid_proof() {
        let temp = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
        let store = AssuranceChallengeStore::load(temp.path()).unwrap();
        let signing_key = SigningKey::from_slice(&[2; 32]).unwrap();
        let public_key_x963 = URL_SAFE_NO_PAD.encode(
            signing_key
                .verifying_key()
                .to_encoded_point(false)
                .as_bytes(),
        );
        let key = DeviceAuthorizationKeyRecord {
            device_id: Uuid::from_u128(1),
            subject_key_id: authorization_key_id(&public_key_x963).unwrap(),
            public_key_x963,
            assurance_class: AssuranceClass::BiometricOnly,
            enrolled_at: now,
            revoked_at: None,
            key_epoch: 1,
        };
        let binding = ChallengeBinding {
            device_id: key.device_id,
            grant_id: Uuid::from_u128(2),
            revocation_epoch: 3,
            authorization_key_id: key.subject_key_id.clone(),
            authorization_key_epoch: key.key_epoch,
        };
        let issued = store.issue(binding, now).unwrap();
        let intent = action_intent();
        let mut proof = presented_proof(
            AssuranceContextMode::Action,
            issued.challenge,
            now,
            now + Duration::seconds(60),
            RequestedAssurance::FreshBiometric,
        );
        let canonical = CanonicalAssuranceProof::for_action(
            AssuranceProofBinding {
                device_id: key.device_id,
                grant_id: Uuid::from_u128(2),
                revocation_epoch: 3,
                authorization_key_id: key.subject_key_id.clone(),
            },
            &proof,
            &intent,
        )
        .unwrap();
        let signature: Signature = signing_key.sign(&canonical.canonical_bytes());
        proof.signature = URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes());

        let mut substituted = intent.clone();
        substituted.operation = "deploy.staging".to_owned();
        assert_eq!(
            verify_and_consume_assurance(
                &store,
                &key,
                Uuid::from_u128(2),
                3,
                &proof,
                AssuranceContext::Action(&substituted),
                now,
            ),
            Err(AssuranceError::SignatureInvalid)
        );
        let verified = verify_and_consume_assurance(
            &store,
            &key,
            Uuid::from_u128(2),
            3,
            &proof,
            AssuranceContext::Action(&intent),
            now,
        )
        .unwrap();
        assert_eq!(verified.effective_assurance, AssuranceLevel::FreshBiometric);
        assert_eq!(
            verify_and_consume_assurance(
                &store,
                &key,
                Uuid::from_u128(2),
                3,
                &proof,
                AssuranceContext::Action(&intent),
                now,
            ),
            Err(AssuranceError::ChallengeSpent)
        );
    }
}
