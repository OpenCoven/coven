use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};
use uuid::Uuid;

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

pub struct VerifiedMobileDevice {
    pub device_id: Uuid,
    grant_id: Uuid,
    revocation_epoch: u64,
    required_scope: Option<DeviceScope>,
}

pub struct MobileAuthenticator {
    registry: Arc<DeviceRegistry>,
    replay: Mutex<HashMap<(Uuid, [u8; 32]), i64>>,
    rates: Mutex<HashMap<Uuid, VecDeque<i64>>>,
}

impl MobileAuthenticator {
    pub fn new(registry: Arc<DeviceRegistry>) -> Self {
        Self {
            registry,
            replay: Mutex::new(HashMap::new()),
            rates: Mutex::new(HashMap::new()),
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
        authorization
            .grant
            .authorize(required_scope, AssuranceLevel::Possession, now)
            .map_err(|_| MobileAuthError::DeviceRevoked)?;
        self.insert_nonce(auth.device_id, nonce, now.timestamp())?;
        self.record_rate(auth.device_id, now.timestamp())?;
        Ok(VerifiedMobileDevice {
            device_id: auth.device_id,
            grant_id: authorization.grant.id,
            revocation_epoch: authorization.grant.revocation_epoch,
            required_scope,
        })
    }

    pub fn ensure_still_active(
        &self,
        verified: &VerifiedMobileDevice,
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
        authorization
            .grant
            .authorize(
                verified.required_scope,
                AssuranceLevel::Possession,
                Utc::now(),
            )
            .map_err(|_| MobileAuthError::DeviceRevoked)
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
    use super::*;
    use chrono::Duration;
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
        };
        let mut changed = authorization.grant;
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
}
