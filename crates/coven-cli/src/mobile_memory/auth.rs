use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::assurance::{
    verify_and_consume_assurance, AssuranceChallengeStore, AssuranceContext, AssuranceError,
    AssuranceProofInput, ChallengeBinding, IssuedAssuranceChallenge, PresentedAssuranceProof,
};
use super::grant::{AssuranceLevel, DeviceScope};
use super::registry::{DeviceAuthorizationRecord, DeviceRecord, DeviceRegistry};
use super::MOBILE_REQUEST_WINDOW_SECONDS;

const MAX_REPLAY_ENTRIES: usize = 10_000;
const RATE_LIMIT_WINDOW_SECONDS: i64 = 60;
const MAX_REQUESTS_PER_WINDOW: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobileAuthError {
    InvalidMethod,
    InvalidPath,
    InvalidEncoding,
    DeviceUnknown,
    DeviceRevoked,
    RequestExpired,
    RequestReplayed,
    SignatureInvalid,
    BodyDigestMismatch,
    RateLimited,
    AssuranceRequired,
}

pub fn canonical_request(
    method: &str,
    path_and_query: &str,
    timestamp: i64,
    nonce_b64url: &str,
    body_digest_b64url: &str,
) -> Result<Vec<u8>, MobileAuthError> {
    if !matches!(method, "GET" | "POST" | "DELETE") {
        return Err(MobileAuthError::InvalidMethod);
    }
    validate_exact_path_and_query(path_and_query)?;
    decode_32(nonce_b64url)?;
    decode_32(body_digest_b64url)?;
    Ok(format!(
        "COVEN-MEMORY/1\n{method}\n{path_and_query}\n{timestamp}\n{nonce_b64url}\n{body_digest_b64url}"
    )
    .into_bytes())
}

fn validate_exact_path_and_query(value: &str) -> Result<(), MobileAuthError> {
    if value.len() > 2_048 || !value.is_ascii() || value.contains('#') || value.contains('\\') {
        return Err(MobileAuthError::InvalidPath);
    }
    let path = value.split_once('?').map_or(value, |(path, _)| path);
    if !path.starts_with("/api/v1/mobile/")
        || path.contains('%')
        || path.contains("//")
        || path.split('/').any(|segment| matches!(segment, "." | ".."))
    {
        return Err(MobileAuthError::InvalidPath);
    }
    Ok(())
}

fn required_scope(path_and_query: &str) -> Option<DeviceScope> {
    let path = path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _)| path);
    (path == "/api/v1/mobile/memory" || path.starts_with("/api/v1/mobile/memory/"))
        .then_some(DeviceScope::MemoryRead)
}

fn decode_32(value: &str) -> Result<[u8; 32], MobileAuthError> {
    if value.len() != 43 {
        return Err(MobileAuthError::InvalidEncoding);
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| MobileAuthError::InvalidEncoding)?;
    decoded
        .try_into()
        .map_err(|_| MobileAuthError::InvalidEncoding)
}

pub struct MobileRequestAuth {
    pub device_id: Uuid,
    pub timestamp: i64,
    pub nonce: String,
    pub body_digest: String,
    pub signature: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssuranceAttempt {
    Absent,
    Rejected,
    Verified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerifiedAssurance {
    binding: ChallengeBinding,
    challenge: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedMobileDevice {
    pub device_id: Uuid,
    grant_id: Uuid,
    revocation_epoch: u64,
    required_scope: Option<DeviceScope>,
    pub effective_assurance: AssuranceLevel,
    pub assurance_attempt: AssuranceAttempt,
    verified_assurance: Option<VerifiedAssurance>,
    assurance_challenge_request: bool,
}

pub struct MobileAuthenticator {
    registry: Arc<DeviceRegistry>,
    replay: Mutex<HashMap<(Uuid, [u8; 32]), i64>>,
    rates: Mutex<HashMap<Uuid, VecDeque<i64>>>,
    challenges: Option<AssuranceChallengeStore>,
}

impl MobileAuthenticator {
    pub fn new(registry: Arc<DeviceRegistry>) -> Self {
        Self {
            registry,
            replay: Mutex::new(HashMap::new()),
            rates: Mutex::new(HashMap::new()),
            challenges: None,
        }
    }

    pub fn with_assurance(
        registry: Arc<DeviceRegistry>,
        challenges: AssuranceChallengeStore,
    ) -> Self {
        Self {
            registry,
            replay: Mutex::new(HashMap::new()),
            rates: Mutex::new(HashMap::new()),
            challenges: Some(challenges),
        }
    }

    pub fn verify(
        &self,
        method: &str,
        path_and_query: &str,
        body: &[u8],
        auth: &MobileRequestAuth,
        now: DateTime<Utc>,
    ) -> Result<VerifiedMobileDevice, MobileAuthError> {
        self.verify_with_assurance(
            method,
            path_and_query,
            body,
            auth,
            AssuranceProofInput::Absent,
            now,
        )
    }

    pub fn verify_with_assurance(
        &self,
        method: &str,
        path_and_query: &str,
        body: &[u8],
        auth: &MobileRequestAuth,
        assurance: AssuranceProofInput,
        now: DateTime<Utc>,
    ) -> Result<VerifiedMobileDevice, MobileAuthError> {
        self.registry
            .reload()
            .map_err(|_| MobileAuthError::DeviceUnknown)?;
        let authorization = self.lookup_device(auth.device_id)?;
        if now.timestamp().abs_diff(auth.timestamp) > MOBILE_REQUEST_WINDOW_SECONDS as u64 {
            return Err(MobileAuthError::RequestExpired);
        }
        let nonce = decode_32(&auth.nonce)?;
        let body_digest = decode_32(&auth.body_digest)?;
        if Sha256::digest(body).as_slice() != body_digest {
            return Err(MobileAuthError::BodyDigestMismatch);
        }
        let canonical = canonical_request(
            method,
            path_and_query,
            auth.timestamp,
            &auth.nonce,
            &auth.body_digest,
        )?;
        verify_signature(&authorization.device, &canonical, &auth.signature)?;
        let required_scope = required_scope(path_and_query);
        let (effective_assurance, assurance_attempt, verified_assurance) = match assurance {
            AssuranceProofInput::Absent => {
                (AssuranceLevel::Possession, AssuranceAttempt::Absent, None)
            }
            AssuranceProofInput::Invalid => {
                (AssuranceLevel::Possession, AssuranceAttempt::Rejected, None)
            }
            AssuranceProofInput::Presented(proof) => {
                match self.verify_presented_assurance(&authorization, &canonical, &proof, now) {
                    Ok((effective, verified)) => {
                        (effective, AssuranceAttempt::Verified, Some(verified))
                    }
                    Err(_) => (AssuranceLevel::Possession, AssuranceAttempt::Rejected, None),
                }
            }
        };
        let grant_result = if path_and_query == "/api/v1/mobile/assurance/challenge" {
            authorization
                .grant
                .authorize(None, authorization.grant.minimum_assurance, now)
        } else {
            authorization
                .grant
                .authorize(required_scope, effective_assurance, now)
        };
        grant_result.map_err(|error| match error {
            super::grant::GrantError::AssuranceRequired => MobileAuthError::AssuranceRequired,
            _ => MobileAuthError::DeviceRevoked,
        })?;
        self.insert_nonce(auth.device_id, nonce, now.timestamp())?;
        self.record_rate(auth.device_id, now.timestamp())?;
        Ok(VerifiedMobileDevice {
            device_id: auth.device_id,
            grant_id: authorization.grant.id,
            revocation_epoch: authorization.grant.revocation_epoch,
            required_scope,
            effective_assurance,
            assurance_attempt,
            verified_assurance,
            assurance_challenge_request: path_and_query == "/api/v1/mobile/assurance/challenge",
        })
    }

    pub fn issue_assurance_challenge(
        &self,
        verified: &VerifiedMobileDevice,
        now: DateTime<Utc>,
    ) -> Result<IssuedAssuranceChallenge, MobileAuthError> {
        self.registry
            .reload()
            .map_err(|_| MobileAuthError::DeviceUnknown)?;
        let authorization = self.lookup_device(verified.device_id)?;
        if authorization.grant.id != verified.grant_id
            || authorization.grant.revocation_epoch != verified.revocation_epoch
        {
            return Err(MobileAuthError::DeviceRevoked);
        }
        let key = self
            .registry
            .authorization_key(verified.device_id)
            .map_err(|_| MobileAuthError::AssuranceRequired)?
            .ok_or(MobileAuthError::AssuranceRequired)?;
        self.challenges
            .as_ref()
            .ok_or(MobileAuthError::AssuranceRequired)?
            .issue(
                ChallengeBinding {
                    device_id: verified.device_id,
                    grant_id: verified.grant_id,
                    revocation_epoch: verified.revocation_epoch,
                    authorization_key_id: key.subject_key_id,
                    authorization_key_epoch: key.key_epoch,
                },
                now,
            )
            .map_err(|_| MobileAuthError::AssuranceRequired)
    }

    pub fn ensure_still_active(
        &self,
        verified: &VerifiedMobileDevice,
    ) -> Result<(), MobileAuthError> {
        self.ensure_still_active_at(verified, Utc::now())
    }

    pub fn ensure_still_active_at(
        &self,
        verified: &VerifiedMobileDevice,
        now: DateTime<Utc>,
    ) -> Result<(), MobileAuthError> {
        self.registry
            .reload()
            .map_err(|_| MobileAuthError::DeviceUnknown)?;
        let authorization = self.lookup_device(verified.device_id)?;
        if authorization.grant.id != verified.grant_id
            || authorization.grant.revocation_epoch != verified.revocation_epoch
        {
            return Err(MobileAuthError::DeviceRevoked);
        }
        if let Some(assurance) = &verified.verified_assurance {
            let current_key = self
                .registry
                .authorization_key(verified.device_id)
                .map_err(|_| MobileAuthError::AssuranceRequired)?
                .ok_or(MobileAuthError::AssuranceRequired)?;
            if current_key.subject_key_id != assurance.binding.authorization_key_id
                || current_key.key_epoch != assurance.binding.authorization_key_epoch
            {
                return Err(MobileAuthError::AssuranceRequired);
            }
            self.challenges
                .as_ref()
                .ok_or(MobileAuthError::AssuranceRequired)?
                .ensure_consumed(assurance.challenge, &assurance.binding)
                .map_err(|_| MobileAuthError::AssuranceRequired)?;
        }
        let presented_assurance = if verified.assurance_challenge_request {
            authorization.grant.minimum_assurance
        } else {
            verified.effective_assurance
        };
        authorization
            .grant
            .authorize(verified.required_scope, presented_assurance, now)
            .map_err(|error| match error {
                super::grant::GrantError::AssuranceRequired => MobileAuthError::AssuranceRequired,
                _ => MobileAuthError::DeviceRevoked,
            })
    }

    fn verify_presented_assurance(
        &self,
        authorization: &DeviceAuthorizationRecord,
        canonical_request: &[u8],
        proof: &PresentedAssuranceProof,
        now: DateTime<Utc>,
    ) -> Result<(AssuranceLevel, VerifiedAssurance), AssuranceError> {
        let key = self
            .registry
            .authorization_key(authorization.device.id)
            .map_err(|_| AssuranceError::StoreUnavailable)?
            .ok_or(AssuranceError::InvalidEncoding)?;
        let challenges = self
            .challenges
            .as_ref()
            .ok_or(AssuranceError::StoreUnavailable)?;
        let verified = verify_and_consume_assurance(
            challenges,
            &key,
            authorization.grant.id,
            authorization.grant.revocation_epoch,
            proof,
            AssuranceContext::Request(canonical_request),
            now,
        )?;
        Ok((
            verified.effective_assurance,
            VerifiedAssurance {
                binding: verified.binding,
                challenge: verified.challenge,
            },
        ))
    }

    fn lookup_device(&self, id: Uuid) -> Result<DeviceAuthorizationRecord, MobileAuthError> {
        match self
            .registry
            .authorization_record(id)
            .map_err(|_| MobileAuthError::DeviceUnknown)?
        {
            None => Err(MobileAuthError::DeviceUnknown),
            Some(record) if record.device.revoked_at.is_some() => {
                Err(MobileAuthError::DeviceRevoked)
            }
            Some(record) => Ok(record),
        }
    }

    fn insert_nonce(
        &self,
        device_id: Uuid,
        nonce: [u8; 32],
        now: i64,
    ) -> Result<(), MobileAuthError> {
        let mut replay = self
            .replay
            .lock()
            .map_err(|_| MobileAuthError::RateLimited)?;
        replay.retain(|_, expires| *expires > now);
        if replay.contains_key(&(device_id, nonce)) {
            return Err(MobileAuthError::RequestReplayed);
        }
        if replay.len() >= MAX_REPLAY_ENTRIES {
            return Err(MobileAuthError::RateLimited);
        }
        replay.insert((device_id, nonce), now + MOBILE_REQUEST_WINDOW_SECONDS);
        Ok(())
    }

    fn record_rate(&self, device_id: Uuid, now: i64) -> Result<(), MobileAuthError> {
        let mut rates = self
            .rates
            .lock()
            .map_err(|_| MobileAuthError::RateLimited)?;
        let requests = rates.entry(device_id).or_default();
        while requests
            .front()
            .is_some_and(|timestamp| *timestamp <= now - RATE_LIMIT_WINDOW_SECONDS)
        {
            requests.pop_front();
        }
        if requests.len() >= MAX_REQUESTS_PER_WINDOW {
            return Err(MobileAuthError::RateLimited);
        }
        requests.push_back(now);
        Ok(())
    }
}

fn verify_signature(
    device: &DeviceRecord,
    canonical: &[u8],
    signature_b64url: &str,
) -> Result<(), MobileAuthError> {
    if signature_b64url.len() > 128 {
        return Err(MobileAuthError::SignatureInvalid);
    }
    let public_key = URL_SAFE_NO_PAD
        .decode(&device.public_key_x963)
        .map_err(|_| MobileAuthError::SignatureInvalid)?;
    let verifying_key = VerifyingKey::from_sec1_bytes(&public_key)
        .map_err(|_| MobileAuthError::SignatureInvalid)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature_b64url)
        .map_err(|_| MobileAuthError::SignatureInvalid)?;
    let signature =
        Signature::from_der(&signature).map_err(|_| MobileAuthError::SignatureInvalid)?;
    verifying_key
        .verify(canonical, &signature)
        .map_err(|_| MobileAuthError::SignatureInvalid)
}

#[cfg(test)]
mod tests {
    use super::super::assurance::{
        AssuranceClass, AssuranceContextMode, AssuranceProofInput, ChallengeBinding,
        NewAuthorizationKey, PresentedAssuranceProof, RequestedAssurance,
    };
    use super::super::grant::DeviceGrant;
    use super::*;
    use chrono::Duration;
    use p256::ecdsa::signature::Signer;
    use p256::ecdsa::{Signature, SigningKey};
    use std::sync::Barrier;

    #[test]
    fn shared_signature_vector_verifies() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/mobile-memory-v1/signature-vector.json"
        ))
        .unwrap();
        let device = DeviceRecord {
            id: Uuid::from_u128(1),
            display_name: "Synthetic phone".to_owned(),
            public_key_x963: vector["publicKeyX963"].as_str().unwrap().to_owned(),
            paired_at: Utc::now(),
            revoked_at: None,
            scopes: vec![DeviceScope::MemoryRead],
        };
        let canonical = canonical_request(
            vector["method"].as_str().unwrap(),
            vector["pathAndQuery"].as_str().unwrap(),
            vector["timestamp"].as_i64().unwrap(),
            vector["nonce"].as_str().unwrap(),
            vector["bodyDigest"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(canonical, vector["canonical"].as_str().unwrap().as_bytes());
        verify_signature(
            &device,
            &canonical,
            vector["signatureDER"].as_str().unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn canonical_request_rejects_path_normalization_ambiguity() {
        let empty_body_digest = URL_SAFE_NO_PAD.encode(Sha256::digest([]));
        for path in [
            "/api/v1/mobile/memory/%2fprivate",
            "/api/v1/mobile/memory/../device",
            "/api/v1/mobile//memory",
        ] {
            assert_eq!(
                canonical_request(
                    "GET",
                    path,
                    1_785_326_400,
                    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                    &empty_body_digest,
                )
                .unwrap_err(),
                MobileAuthError::InvalidPath
            );
        }
    }

    #[test]
    fn protected_memory_routes_require_memory_read_scope() {
        assert_eq!(
            required_scope("/api/v1/mobile/memory"),
            Some(DeviceScope::MemoryRead)
        );
        assert_eq!(
            required_scope("/api/v1/mobile/memory/overview"),
            Some(DeviceScope::MemoryRead)
        );
        assert_eq!(required_scope("/api/v1/mobile/device"), None);
    }

    #[test]
    fn accepted_nonce_cannot_be_replayed_even_concurrently() {
        let (_temp, authenticator) = authenticator();
        let authenticator = Arc::new(authenticator);
        let barrier = Arc::new(Barrier::new(8));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let authenticator = authenticator.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                authenticator.insert_nonce(Uuid::from_u128(1), [7; 32], 100)
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
    fn revoked_device_loses_a_race_before_response() {
        let (_temp, authenticator) = authenticator();
        let device_id = Uuid::from_u128(1);
        authenticator
            .registry
            .register(test_device(device_id))
            .unwrap();
        let authorization = authenticator
            .registry
            .authorization_record(device_id)
            .unwrap()
            .unwrap();
        let verified = VerifiedMobileDevice {
            device_id,
            grant_id: authorization.grant.id,
            revocation_epoch: authorization.grant.revocation_epoch,
            required_scope: Some(DeviceScope::MemoryRead),
            effective_assurance: AssuranceLevel::Possession,
            assurance_attempt: AssuranceAttempt::Absent,
            verified_assurance: None,
            assurance_challenge_request: false,
        };
        authenticator
            .registry
            .revoke(device_id, Utc::now() + Duration::seconds(1))
            .unwrap();
        assert_eq!(
            authenticator.ensure_still_active(&verified),
            Err(MobileAuthError::DeviceRevoked)
        );
    }

    #[test]
    fn changed_grant_loses_a_race_before_response() {
        let (_temp, authenticator) = authenticator();
        let device_id = Uuid::from_u128(2);
        authenticator
            .registry
            .register(test_device(device_id))
            .unwrap();
        let authorization = authenticator
            .registry
            .authorization_record(device_id)
            .unwrap()
            .unwrap();
        let verified = VerifiedMobileDevice {
            device_id,
            grant_id: authorization.grant.id,
            revocation_epoch: authorization.grant.revocation_epoch,
            required_scope: Some(DeviceScope::MemoryRead),
            effective_assurance: AssuranceLevel::Possession,
            assurance_attempt: AssuranceAttempt::Absent,
            verified_assurance: None,
            assurance_challenge_request: false,
        };
        let mut changed = authorization.grant;
        changed.id = Uuid::new_v4();
        changed.revocation_epoch += 1;
        authenticator
            .registry
            .replace_grant(device_id, changed)
            .unwrap();
        assert_eq!(
            authenticator.ensure_still_active(&verified),
            Err(MobileAuthError::DeviceRevoked)
        );
    }

    #[test]
    fn rate_limits_are_per_device_and_bounded() {
        let (_temp, authenticator) = authenticator();
        let first = Uuid::from_u128(1);
        for _ in 0..MAX_REQUESTS_PER_WINDOW {
            authenticator.record_rate(first, 100).unwrap();
        }
        assert_eq!(
            authenticator.record_rate(first, 100),
            Err(MobileAuthError::RateLimited)
        );
        authenticator.record_rate(Uuid::from_u128(2), 100).unwrap();
        assert_eq!(
            authenticator.rates.lock().unwrap()[&first].len(),
            MAX_REQUESTS_PER_WINDOW
        );
    }

    #[test]
    fn step_up_authentication_fails_closed_and_spends_only_valid_exact_proof() {
        let mut harness = StepUpHarness::new(AssuranceClass::BiometricOnly);
        harness.grant.minimum_assurance = AssuranceLevel::FreshBiometric;
        harness.grant.id = Uuid::new_v4();
        harness.grant.revocation_epoch += 1;
        harness
            .authenticator
            .registry
            .replace_grant(harness.device_id, harness.grant.clone())
            .unwrap();

        let (auth, _) = harness.request_auth(7, "/api/v1/mobile/memory/overview");
        assert_eq!(
            harness.authenticator.verify_with_assurance(
                "GET",
                "/api/v1/mobile/memory/overview",
                &[],
                &auth,
                AssuranceProofInput::Absent,
                harness.now,
            ),
            Err(MobileAuthError::AssuranceRequired)
        );

        let issued = harness.issue_challenge();
        let (auth, canonical) = harness.request_auth(8, "/api/v1/mobile/memory/overview");
        let proof = harness.proof(issued.challenge, issued.expires_at, &canonical);
        let mut invalid = proof.clone();
        invalid.signature = URL_SAFE_NO_PAD.encode([9; 64]);
        assert_eq!(
            harness.authenticator.verify_with_assurance(
                "GET",
                "/api/v1/mobile/memory/overview",
                &[],
                &auth,
                AssuranceProofInput::Presented(invalid),
                harness.now,
            ),
            Err(MobileAuthError::AssuranceRequired)
        );

        let (auth, _) = harness.request_auth(8, "/api/v1/mobile/memory/overview");
        let verified = harness
            .authenticator
            .verify_with_assurance(
                "GET",
                "/api/v1/mobile/memory/overview",
                &[],
                &auth,
                AssuranceProofInput::Presented(proof.clone()),
                harness.now,
            )
            .unwrap();
        assert_eq!(verified.effective_assurance, AssuranceLevel::FreshBiometric);
        assert_eq!(verified.assurance_attempt, AssuranceAttempt::Verified);

        let (auth, _) = harness.request_auth(9, "/api/v1/mobile/memory/overview");
        assert_eq!(
            harness.authenticator.verify_with_assurance(
                "GET",
                "/api/v1/mobile/memory/overview",
                &[],
                &auth,
                AssuranceProofInput::Presented(proof),
                harness.now,
            ),
            Err(MobileAuthError::AssuranceRequired)
        );

        let issued = harness.issue_challenge();
        let wrong_context = b"COVEN-MEMORY/1\nGET\n/api/v1/mobile/memory/other";
        let wrong_proof = harness.proof(issued.challenge, issued.expires_at, wrong_context);
        let (auth, canonical) = harness.request_auth(11, "/api/v1/mobile/memory/overview");
        assert_eq!(
            harness.authenticator.verify_with_assurance(
                "GET",
                "/api/v1/mobile/memory/overview",
                &[],
                &auth,
                AssuranceProofInput::Presented(wrong_proof),
                harness.now,
            ),
            Err(MobileAuthError::AssuranceRequired)
        );
        let correct = harness.proof(issued.challenge, issued.expires_at, &canonical);
        let (auth, _) = harness.request_auth(11, "/api/v1/mobile/memory/overview");
        harness
            .authenticator
            .verify_with_assurance(
                "GET",
                "/api/v1/mobile/memory/overview",
                &[],
                &auth,
                AssuranceProofInput::Presented(correct),
                harness.now,
            )
            .unwrap();
    }

    #[test]
    fn device_credential_proof_never_satisfies_fresh_biometric() {
        let mut harness = StepUpHarness::new(AssuranceClass::DeviceCredential);
        harness.grant.minimum_assurance = AssuranceLevel::FreshBiometric;
        harness.grant.id = Uuid::new_v4();
        harness.grant.revocation_epoch += 1;
        harness
            .authenticator
            .registry
            .replace_grant(harness.device_id, harness.grant.clone())
            .unwrap();
        let issued = harness.issue_challenge();
        let (auth, canonical) = harness.request_auth(20, "/api/v1/mobile/memory/overview");
        let proof = harness.proof(issued.challenge, issued.expires_at, &canonical);
        assert_eq!(
            harness.authenticator.verify_with_assurance(
                "GET",
                "/api/v1/mobile/memory/overview",
                &[],
                &auth,
                AssuranceProofInput::Presented(proof),
                harness.now,
            ),
            Err(MobileAuthError::AssuranceRequired)
        );
    }

    #[test]
    fn post_response_recheck_reuses_assurance_and_detects_key_revocation() {
        let mut harness = StepUpHarness::new(AssuranceClass::BiometricOnly);
        harness.grant.minimum_assurance = AssuranceLevel::FreshBiometric;
        harness.grant.id = Uuid::new_v4();
        harness.grant.revocation_epoch += 1;
        harness
            .authenticator
            .registry
            .replace_grant(harness.device_id, harness.grant.clone())
            .unwrap();
        let issued = harness.issue_challenge();
        let (auth, canonical) = harness.request_auth(30, "/api/v1/mobile/memory/overview");
        let proof = harness.proof(issued.challenge, issued.expires_at, &canonical);
        let verified = harness
            .authenticator
            .verify_with_assurance(
                "GET",
                "/api/v1/mobile/memory/overview",
                &[],
                &auth,
                AssuranceProofInput::Presented(proof),
                harness.now,
            )
            .unwrap();
        harness
            .authenticator
            .ensure_still_active_at(&verified, harness.now)
            .unwrap();

        harness
            .authenticator
            .registry
            .revoke_authorization_key(harness.device_id, harness.now)
            .unwrap();
        assert_eq!(
            harness
                .authenticator
                .ensure_still_active_at(&verified, harness.now),
            Err(MobileAuthError::AssuranceRequired)
        );
    }

    #[test]
    fn grant_reissue_invalidates_outstanding_assurance_challenges() {
        let mut harness = StepUpHarness::new(AssuranceClass::BiometricOnly);
        harness.grant.minimum_assurance = AssuranceLevel::FreshBiometric;
        harness.grant.id = Uuid::new_v4();
        harness.grant.revocation_epoch += 1;
        harness
            .authenticator
            .registry
            .replace_grant(harness.device_id, harness.grant.clone())
            .unwrap();
        let issued = harness.issue_challenge();
        let (auth, canonical) = harness.request_auth(40, "/api/v1/mobile/memory/overview");
        let proof = harness.proof(issued.challenge, issued.expires_at, &canonical);
        let possession_public_key = harness
            .authenticator
            .registry
            .authorization_record(harness.device_id)
            .unwrap()
            .unwrap()
            .device
            .public_key_x963;

        let reissued = harness
            .grant
            .reissue(
                &possession_public_key,
                harness.grant.scopes.clone(),
                harness.grant.restrictions.clone(),
                harness.grant.minimum_assurance,
                harness.now,
                harness.now + Duration::days(30),
            )
            .unwrap();
        harness
            .authenticator
            .registry
            .replace_grant(harness.device_id, reissued.clone())
            .unwrap();
        harness.grant = reissued;

        assert_eq!(
            harness.authenticator.verify_with_assurance(
                "GET",
                "/api/v1/mobile/memory/overview",
                &[],
                &auth,
                AssuranceProofInput::Presented(proof),
                harness.now,
            ),
            Err(MobileAuthError::AssuranceRequired)
        );
    }

    fn authenticator() -> (tempfile::TempDir, MobileAuthenticator) {
        let temp = tempfile::tempdir().unwrap();
        let registry = Arc::new(DeviceRegistry::load(temp.path()).unwrap());
        (temp, MobileAuthenticator::new(registry))
    }

    fn test_device(id: Uuid) -> DeviceRecord {
        let signing_key = p256::SecretKey::from_slice(&[1; 32]).unwrap();
        use p256::elliptic_curve::sec1::ToEncodedPoint;
        DeviceRecord {
            id,
            display_name: "Synthetic phone".to_owned(),
            public_key_x963: URL_SAFE_NO_PAD
                .encode(signing_key.public_key().to_encoded_point(false).as_bytes()),
            paired_at: Utc::now(),
            revoked_at: None,
            scopes: vec![DeviceScope::MemoryRead],
        }
    }

    struct StepUpHarness {
        _temp: tempfile::TempDir,
        authenticator: MobileAuthenticator,
        device_id: Uuid,
        possession_key: SigningKey,
        authorization_key: SigningKey,
        authorization_key_id: String,
        authorization_key_epoch: u64,
        grant: DeviceGrant,
        now: DateTime<Utc>,
    }

    impl StepUpHarness {
        fn new(assurance_class: AssuranceClass) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let registry = Arc::new(DeviceRegistry::load(temp.path()).unwrap());
            let challenges =
                super::super::assurance::AssuranceChallengeStore::load(temp.path()).unwrap();
            let authenticator = MobileAuthenticator::with_assurance(registry, challenges);
            let device_id = Uuid::from_u128(1);
            let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
            let possession_key = SigningKey::from_slice(&[1; 32]).unwrap();
            let authorization_key = SigningKey::from_slice(&[2; 32]).unwrap();
            let public_key_x963 = URL_SAFE_NO_PAD.encode(
                possession_key
                    .verifying_key()
                    .to_encoded_point(false)
                    .as_bytes(),
            );
            let authorization_public_key = URL_SAFE_NO_PAD.encode(
                authorization_key
                    .verifying_key()
                    .to_encoded_point(false)
                    .as_bytes(),
            );
            let record = DeviceRecord {
                id: device_id,
                display_name: "Synthetic phone".to_owned(),
                public_key_x963: public_key_x963.clone(),
                paired_at: now,
                revoked_at: None,
                scopes: vec![DeviceScope::MemoryRead],
            };
            let grant =
                DeviceGrant::for_device(device_id, &public_key_x963, record.scopes.clone(), now)
                    .unwrap();
            authenticator
                .registry
                .register_with_grant_and_authorization(
                    record,
                    grant.clone(),
                    Some(NewAuthorizationKey {
                        public_key_x963: authorization_public_key,
                        assurance_class,
                        enrolled_at: now,
                    }),
                )
                .unwrap();
            let authorization = authenticator
                .registry
                .authorization_key(device_id)
                .unwrap()
                .unwrap();
            Self {
                _temp: temp,
                authenticator,
                device_id,
                possession_key,
                authorization_key,
                authorization_key_id: authorization.subject_key_id,
                authorization_key_epoch: authorization.key_epoch,
                grant,
                now,
            }
        }

        fn request_auth(&self, nonce_seed: u8, path: &str) -> (MobileRequestAuth, Vec<u8>) {
            let nonce = URL_SAFE_NO_PAD.encode([nonce_seed; 32]);
            let body_digest = URL_SAFE_NO_PAD.encode(Sha256::digest([]));
            let canonical =
                canonical_request("GET", path, self.now.timestamp(), &nonce, &body_digest).unwrap();
            let signature: Signature = self.possession_key.sign(&canonical);
            (
                MobileRequestAuth {
                    device_id: self.device_id,
                    timestamp: self.now.timestamp(),
                    nonce,
                    body_digest,
                    signature: URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes()),
                },
                canonical,
            )
        }

        fn issue_challenge(&self) -> super::super::assurance::IssuedAssuranceChallenge {
            self.authenticator
                .challenges
                .as_ref()
                .unwrap()
                .issue(
                    ChallengeBinding {
                        device_id: self.device_id,
                        grant_id: self.grant.id,
                        revocation_epoch: self.grant.revocation_epoch,
                        authorization_key_id: self.authorization_key_id.clone(),
                        authorization_key_epoch: self.authorization_key_epoch,
                    },
                    self.now,
                )
                .unwrap()
        }

        fn proof(
            &self,
            challenge: [u8; 32],
            challenge_expires_at: DateTime<Utc>,
            canonical_request: &[u8],
        ) -> PresentedAssuranceProof {
            let issued_at = self.now;
            let expires_at = challenge_expires_at;
            let canonical = super::super::assurance::CanonicalAssuranceProof::for_request(
                super::super::assurance::AssuranceProofBinding {
                    device_id: self.device_id,
                    grant_id: self.grant.id,
                    revocation_epoch: self.grant.revocation_epoch,
                    authorization_key_id: self.authorization_key_id.clone(),
                },
                &PresentedAssuranceProof {
                    context_mode: AssuranceContextMode::Request,
                    challenge,
                    issued_at,
                    expires_at,
                    requested_assurance: RequestedAssurance::FreshBiometric,
                    signature: String::new(),
                },
                canonical_request,
            )
            .unwrap();
            let signature: Signature = self.authorization_key.sign(&canonical.canonical_bytes());
            PresentedAssuranceProof {
                context_mode: AssuranceContextMode::Request,
                challenge,
                issued_at,
                expires_at,
                requested_assurance: RequestedAssurance::FreshBiometric,
                signature: URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes()),
            }
        }
    }
}
