use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use qrcode::render::unicode::Dense1x2;
use qrcode::QrCode;
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

use super::contract::{MobileDeviceScope, MobilePairedDevice, MobilePairingRequest};
use super::registry::{DeviceRecord, DeviceRegistry, DeviceScope};
use super::MOBILE_PROTOCOL_VERSION;

const PAIRING_WORDS: &str = include_str!("pairing_words.txt");

#[derive(Debug, Clone)]
pub struct PendingPairing {
    pub id: Uuid,
    pub nonce_hash: [u8; 32],
    pub expires_at: DateTime<Utc>,
    pub transcript_hash: Option<[u8; 32]>,
    pub device: Option<PendingDevice>,
    pub host_confirmed: bool,
    pub device_confirmed: bool,
    pub consumed: bool,
    pub completed: Option<MobilePairedDevice>,
}

#[derive(Debug, Clone)]
pub struct PendingDevice {
    pub display_name: String,
    pub public_key_x963: String,
    pub app_version: String,
}

#[derive(Debug, Clone)]
pub struct PairingInvitation {
    pub id: Uuid,
    pub nonce: [u8; 32],
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrolledPairing {
    pub id: Uuid,
    pub phrase: [String; 6],
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingProgress {
    Pending,
    Complete {
        device: MobilePairedDevice,
        replayed: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingError {
    PairingExpired,
    PairingConsumed,
    PairingConfirmationRequired,
    PairingPhraseMismatch,
    InvalidRequest,
}

impl fmt::Display for PairingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::PairingExpired => "pairing expired",
            Self::PairingConsumed => "pairing nonce was already consumed",
            Self::PairingConfirmationRequired => "pairing confirmation is required",
            Self::PairingPhraseMismatch => "pairing phrase did not match",
            Self::InvalidRequest => "pairing request is invalid",
        })
    }
}

impl std::error::Error for PairingError {}

pub struct PairingManager {
    registry: Arc<DeviceRegistry>,
    pending: Mutex<HashMap<Uuid, PendingPairing>>,
}

impl PairingManager {
    pub fn new(registry: Arc<DeviceRegistry>) -> Self {
        Self {
            registry,
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn begin_pairing(
        &self,
        nonce: [u8; 32],
        expires_at: DateTime<Utc>,
    ) -> Result<PairingInvitation, PairingError> {
        self.insert_pairing(Uuid::new_v4(), nonce, expires_at, Some(Utc::now()))
    }

    #[cfg(test)]
    fn begin_pairing_with_id(
        &self,
        id: Uuid,
        nonce: [u8; 32],
        expires_at: DateTime<Utc>,
    ) -> Result<PairingInvitation, PairingError> {
        self.insert_pairing(id, nonce, expires_at, None)
    }

    fn insert_pairing(
        &self,
        id: Uuid,
        nonce: [u8; 32],
        expires_at: DateTime<Utc>,
        prune_before_insert: Option<DateTime<Utc>>,
    ) -> Result<PairingInvitation, PairingError> {
        let pairing = PendingPairing {
            id,
            nonce_hash: Sha256::digest(nonce).into(),
            expires_at,
            transcript_hash: None,
            device: None,
            host_confirmed: false,
            device_confirmed: false,
            consumed: false,
            completed: None,
        };
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| PairingError::InvalidRequest)?;
        if let Some(now) = prune_before_insert {
            Self::prune_expired(&mut pending, now, |_| false);
        }
        pending.insert(id, pairing);
        Ok(PairingInvitation {
            id,
            nonce,
            expires_at,
        })
    }

    fn prune_expired<F>(
        pending: &mut HashMap<Uuid, PendingPairing>,
        now: DateTime<Utc>,
        mut retain_expired: F,
    ) where
        F: FnMut(&PendingPairing) -> bool,
    {
        pending.retain(|_, pairing| pairing.expires_at > now || retain_expired(pairing));
    }

    pub fn enroll(
        &self,
        pairing_id: Uuid,
        nonce: [u8; 32],
        request: MobilePairingRequest,
        host_fingerprint: [u8; 32],
        now: DateTime<Utc>,
    ) -> Result<EnrolledPairing, PairingError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| PairingError::InvalidRequest)?;
        let pairing = pending
            .get_mut(&pairing_id)
            .ok_or(PairingError::PairingConsumed)?;
        if now >= pairing.expires_at {
            pending.remove(&pairing_id);
            return Err(PairingError::PairingExpired);
        }
        if pairing.consumed {
            return Err(PairingError::PairingConsumed);
        }
        pairing.consumed = true;
        if Sha256::digest(nonce).as_slice() != pairing.nonce_hash {
            return Err(PairingError::PairingPhraseMismatch);
        }
        validate_pairing_request(&request)?;
        let public_key = URL_SAFE_NO_PAD
            .decode(&request.device_public_key)
            .map_err(|_| PairingError::InvalidRequest)?;
        let transcript = PairingTranscript {
            protocol_version: request.protocol_version,
            host_fingerprint,
            pairing_id,
            device_public_key: public_key,
            nonce,
        };
        let transcript_hash = transcript.hash();
        let phrase = derive_pairing_phrase(&transcript);
        pairing.transcript_hash = Some(transcript_hash);
        pairing.device = Some(PendingDevice {
            display_name: request.device_name,
            public_key_x963: request.device_public_key,
            app_version: request.app_version,
        });
        Ok(EnrolledPairing {
            id: pairing_id,
            phrase,
            expires_at: pairing.expires_at,
        })
    }

    pub fn enroll_by_nonce(
        &self,
        nonce: [u8; 32],
        request: MobilePairingRequest,
        host_fingerprint: [u8; 32],
        now: DateTime<Utc>,
    ) -> Result<EnrolledPairing, PairingError> {
        let nonce_hash: [u8; 32] = Sha256::digest(nonce).into();
        let pairing_id = {
            let mut pending = self
                .pending
                .lock()
                .map_err(|_| PairingError::InvalidRequest)?;
            Self::prune_expired(&mut pending, now, |pairing| {
                pairing.nonce_hash == nonce_hash
            });
            pending
                .values()
                .find(|pairing| pairing.nonce_hash == nonce_hash)
                .map(|pairing| pairing.id)
                .ok_or(PairingError::PairingConsumed)?
        };
        self.enroll(pairing_id, nonce, request, host_fingerprint, now)
    }

    pub fn confirm_host(
        &self,
        pairing_id: Uuid,
        phrase: &[String],
        now: DateTime<Utc>,
    ) -> Result<PairingProgress, PairingError> {
        self.confirm(pairing_id, phrase, now, true)
    }

    pub fn confirm_device(
        &self,
        pairing_id: Uuid,
        phrase: &[String],
        now: DateTime<Utc>,
    ) -> Result<PairingProgress, PairingError> {
        self.confirm(pairing_id, phrase, now, false)
    }

    pub fn phrase(
        &self,
        pairing_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<[String; 6]>, PairingError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| PairingError::InvalidRequest)?;
        let pairing = pending
            .get(&pairing_id)
            .ok_or(PairingError::PairingConsumed)?;
        if now >= pairing.expires_at {
            pending.remove(&pairing_id);
            return Err(PairingError::PairingExpired);
        }
        Ok(pairing.transcript_hash.map(phrase_for_hash))
    }

    fn confirm(
        &self,
        pairing_id: Uuid,
        phrase: &[String],
        now: DateTime<Utc>,
        host: bool,
    ) -> Result<PairingProgress, PairingError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| PairingError::InvalidRequest)?;
        let pairing = pending
            .get_mut(&pairing_id)
            .ok_or(PairingError::PairingConsumed)?;
        if now >= pairing.expires_at {
            pending.remove(&pairing_id);
            return Err(PairingError::PairingExpired);
        }
        let transcript_hash = pairing
            .transcript_hash
            .ok_or(PairingError::PairingConfirmationRequired)?;
        let expected = phrase_for_hash(transcript_hash);
        if phrase != expected {
            if pairing.completed.is_none() {
                pending.remove(&pairing_id);
            }
            return Err(PairingError::PairingPhraseMismatch);
        }
        if let Some(device) = &pairing.completed {
            return Ok(PairingProgress::Complete {
                device: device.clone(),
                replayed: true,
            });
        }
        if host {
            pairing.host_confirmed = true;
        } else {
            pairing.device_confirmed = true;
        }
        if !pairing.host_confirmed || !pairing.device_confirmed {
            return Ok(PairingProgress::Pending);
        }
        let device = pairing
            .device
            .clone()
            .ok_or(PairingError::PairingConfirmationRequired)?;
        let record = DeviceRecord {
            id: Uuid::new_v4(),
            display_name: device.display_name,
            public_key_x963: device.public_key_x963,
            paired_at: now,
            revoked_at: None,
            scopes: vec![DeviceScope::MemoryRead],
        };
        self.registry
            .register(record.clone())
            .map_err(|_| PairingError::InvalidRequest)?;
        let completed = MobilePairedDevice {
            id: record.id,
            display_name: record.display_name,
            paired_at: record.paired_at,
            scopes: vec![MobileDeviceScope::MemoryRead],
        };
        pairing.device = None;
        pairing.completed = Some(completed.clone());
        Ok(PairingProgress::Complete {
            device: completed,
            replayed: false,
        })
    }
}

struct PairingTranscript {
    protocol_version: u16,
    host_fingerprint: [u8; 32],
    pairing_id: Uuid,
    device_public_key: Vec<u8>,
    nonce: [u8; 32],
}

impl PairingTranscript {
    fn hash(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        for field in [
            self.protocol_version.to_be_bytes().as_slice(),
            self.host_fingerprint.as_slice(),
            self.pairing_id.as_bytes(),
            self.device_public_key.as_slice(),
            self.nonce.as_slice(),
        ] {
            digest.update((field.len() as u32).to_be_bytes());
            digest.update(field);
        }
        digest.finalize().into()
    }
}

fn validate_pairing_request(request: &MobilePairingRequest) -> Result<(), PairingError> {
    if request.protocol_version != MOBILE_PROTOCOL_VERSION
        || request.supported_protocol.minimum > MOBILE_PROTOCOL_VERSION
        || request.supported_protocol.maximum < MOBILE_PROTOCOL_VERSION
        || request.device_name.is_empty()
        || request.device_name.trim() != request.device_name
        || request.device_name.chars().count() > 80
        || request.device_name.chars().any(char::is_control)
        || request.app_version.is_empty()
        || request.app_version.len() > 64
    {
        return Err(PairingError::InvalidRequest);
    }
    let key = URL_SAFE_NO_PAD
        .decode(&request.device_public_key)
        .map_err(|_| PairingError::InvalidRequest)?;
    if key.len() != 65
        || key.first() != Some(&4)
        || URL_SAFE_NO_PAD.encode(&key) != request.device_public_key
        || p256::PublicKey::from_sec1_bytes(&key).is_err()
    {
        return Err(PairingError::InvalidRequest);
    }
    Ok(())
}

fn derive_pairing_phrase(transcript: &PairingTranscript) -> [String; 6] {
    phrase_for_hash(transcript.hash())
}

fn phrase_for_hash(hash: [u8; 32]) -> [String; 6] {
    let words = pairing_words();
    std::array::from_fn(|word_index| {
        let mut index = 0_usize;
        for bit_offset in 0..11 {
            let bit = word_index * 11 + bit_offset;
            index = (index << 1) | usize::from((hash[bit / 8] >> (7 - bit % 8)) & 1);
        }
        words[index].to_owned()
    })
}

fn pairing_words() -> Vec<&'static str> {
    PAIRING_WORDS.lines().collect()
}

pub fn build_pairing_url(
    invitation: &PairingInvitation,
    endpoint: &str,
    host_fingerprint: [u8; 32],
) -> Result<String, PairingError> {
    let mut url = Url::parse("coven-memory://pair").map_err(|_| PairingError::InvalidRequest)?;
    url.query_pairs_mut()
        .append_pair("version", &MOBILE_PROTOCOL_VERSION.to_string())
        .append_pair("endpoint", endpoint)
        .append_pair("fingerprint", &URL_SAFE_NO_PAD.encode(host_fingerprint))
        .append_pair("nonce", &URL_SAFE_NO_PAD.encode(invitation.nonce))
        .append_pair("expires", &invitation.expires_at.timestamp().to_string());
    Ok(url.into())
}

pub fn render_pairing_invitation(
    pairing_url: &str,
    expires_at: DateTime<Utc>,
) -> Result<String, PairingError> {
    let qr = QrCode::new(pairing_url.as_bytes())
        .map_err(|_| PairingError::InvalidRequest)?
        .render::<Dense1x2>()
        .build();
    Ok(format!(
        "{qr}\n{pairing_url}\nExpires: {}\nWaiting for device…\nPress Ctrl-C to cancel pairing.",
        expires_at.to_rfc3339()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    use rand::random;
    use std::collections::HashSet;

    fn assert_complete(progress: PairingProgress, replayed: bool) -> MobilePairedDevice {
        match progress {
            PairingProgress::Complete {
                device,
                replayed: actual,
            } => {
                assert_eq!(actual, replayed);
                device
            }
            PairingProgress::Pending => panic!("expected pairing completion"),
        }
    }

    struct PairingHarness {
        _temp: tempfile::TempDir,
        manager: PairingManager,
        pairing_id: Uuid,
        now: DateTime<Utc>,
        pairing_nonce: [u8; 32],
        request: MobilePairingRequest,
    }

    impl PairingHarness {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let registry = Arc::new(DeviceRegistry::load(temp.path()).unwrap());
            let manager = PairingManager::new(registry);
            let pairing_id = Uuid::from_u128(1);
            let now = DateTime::from_timestamp(1_785_326_400, 0).unwrap();
            let pairing_nonce = random::<[u8; 32]>();
            manager
                .begin_pairing_with_id(pairing_id, pairing_nonce, now + Duration::minutes(5))
                .unwrap();
            let signing_key = p256::SecretKey::from_slice(&[1; 32]).unwrap();
            let public_key = signing_key.public_key().to_encoded_point(false);
            Self {
                _temp: temp,
                manager,
                pairing_id,
                now,
                pairing_nonce,
                request: MobilePairingRequest {
                    protocol_version: MOBILE_PROTOCOL_VERSION,
                    pairing_nonce: URL_SAFE_NO_PAD.encode(pairing_nonce),
                    device_name: "Synthetic phone".to_owned(),
                    device_public_key: URL_SAFE_NO_PAD.encode(public_key.as_bytes()),
                    app_version: "1.0.0".to_owned(),
                    supported_protocol: super::super::contract::MobileProtocolRange {
                        minimum: 1,
                        maximum: 1,
                    },
                },
            }
        }

        fn enroll(&self) -> EnrolledPairing {
            self.enroll_with_nonce(self.pairing_nonce).unwrap()
        }

        fn enroll_with_nonce(&self, nonce: [u8; 32]) -> Result<EnrolledPairing, PairingError> {
            self.manager.enroll(
                self.pairing_id,
                nonce,
                self.request.clone(),
                [3; 32],
                self.now,
            )
        }

        fn enroll_after_expiry(&self) -> Result<EnrolledPairing, PairingError> {
            let nonce = random::<[u8; 32]>();
            self.manager.enroll(
                self.pairing_id,
                nonce,
                self.request.clone(),
                [3; 32],
                self.now + Duration::minutes(6),
            )
        }

        fn confirm_host(&self, phrase: &[String]) -> PairingProgress {
            self.manager
                .confirm_host(self.pairing_id, phrase, self.now)
                .unwrap()
        }

        fn confirm_device(&self, phrase: &[String]) -> PairingProgress {
            self.manager
                .confirm_device(self.pairing_id, phrase, self.now)
                .unwrap()
        }

        fn devices(&self) -> Vec<super::super::registry::DeviceStatusRecord> {
            self.manager.registry.list_status().unwrap()
        }
    }

    fn synthetic_transcript() -> PairingTranscript {
        PairingTranscript {
            protocol_version: 1,
            host_fingerprint: [3; 32],
            pairing_id: Uuid::from_u128(1),
            device_public_key: vec![4; 65],
            nonce: [7; 32],
        }
    }

    #[test]
    fn pairing_requires_host_and_device_confirmation() {
        let harness = PairingHarness::new();
        let pending = harness.enroll();
        assert_eq!(
            harness.confirm_host(&pending.phrase),
            PairingProgress::Pending
        );
        assert_eq!(
            assert_complete(harness.confirm_device(&pending.phrase), false),
            MobilePairedDevice {
                id: harness.devices()[0].id,
                display_name: "Synthetic phone".to_owned(),
                paired_at: harness.now,
                scopes: vec![MobileDeviceScope::MemoryRead],
            }
        );
    }

    #[test]
    fn pairing_nonce_is_consumed_on_first_enrollment_attempt() {
        let harness = PairingHarness::new();
        assert_eq!(
            harness.enroll_with_nonce([9; 32]).unwrap_err(),
            PairingError::PairingPhraseMismatch
        );
        assert_eq!(
            harness.enroll_with_nonce([7; 32]).unwrap_err(),
            PairingError::PairingConsumed
        );
    }

    #[test]
    fn incomplete_pairing_mismatch_invalidates_the_retry_window() {
        let harness = PairingHarness::new();
        let pending = harness.enroll();
        let mut wrong_phrase = pending.phrase.clone();
        wrong_phrase[0] = "wrong".to_owned();

        assert_eq!(
            harness
                .manager
                .confirm_host(harness.pairing_id, &wrong_phrase, harness.now)
                .unwrap_err(),
            PairingError::PairingPhraseMismatch
        );
        assert_eq!(
            harness
                .manager
                .confirm_host(harness.pairing_id, &pending.phrase, harness.now)
                .unwrap_err(),
            PairingError::PairingConsumed
        );
        assert!(harness.devices().is_empty());
    }

    #[test]
    fn device_confirmation_retry_reuses_completed_device() {
        let harness = PairingHarness::new();
        let pending = harness.enroll();

        assert_eq!(
            harness.confirm_device(&pending.phrase),
            PairingProgress::Pending
        );
        let device = assert_complete(harness.confirm_host(&pending.phrase), false);
        let replay = assert_complete(harness.confirm_device(&pending.phrase), true);

        assert_eq!(replay, device);
        assert_eq!(harness.devices().len(), 1);
    }

    #[test]
    fn host_confirmation_retry_reuses_completed_device() {
        let harness = PairingHarness::new();
        let pending = harness.enroll();

        assert_eq!(
            harness.confirm_host(&pending.phrase),
            PairingProgress::Pending
        );
        let device = assert_complete(harness.confirm_device(&pending.phrase), false);
        let replay = assert_complete(harness.confirm_host(&pending.phrase), true);

        assert_eq!(replay, device);
        assert_eq!(harness.devices().len(), 1);
    }

    #[test]
    fn completed_pairing_rejects_wrong_phrase_but_keeps_retry_window() {
        let harness = PairingHarness::new();
        let pending = harness.enroll();

        assert_eq!(
            harness.confirm_host(&pending.phrase),
            PairingProgress::Pending
        );
        let device = assert_complete(harness.confirm_device(&pending.phrase), false);
        let mut wrong_phrase = pending.phrase.clone();
        wrong_phrase[0] = "wrong".to_owned();

        assert_eq!(
            harness
                .manager
                .confirm_host(harness.pairing_id, &wrong_phrase, harness.now)
                .unwrap_err(),
            PairingError::PairingPhraseMismatch
        );
        assert_eq!(
            assert_complete(harness.confirm_host(&pending.phrase), true),
            device
        );
        assert_eq!(harness.devices().len(), 1);
    }

    #[test]
    fn completed_pairing_retry_expires_on_original_deadline() {
        let harness = PairingHarness::new();
        let pending = harness.enroll();

        assert_eq!(
            harness.confirm_host(&pending.phrase),
            PairingProgress::Pending
        );
        let device = assert_complete(harness.confirm_device(&pending.phrase), false);
        assert_eq!(
            assert_complete(harness.confirm_host(&pending.phrase), true),
            device
        );

        assert_eq!(
            harness
                .manager
                .confirm_device(
                    harness.pairing_id,
                    &pending.phrase,
                    harness.now + Duration::minutes(6),
                )
                .unwrap_err(),
            PairingError::PairingExpired
        );
        assert_eq!(harness.devices().len(), 1);
    }

    #[test]
    fn later_pairing_operations_prune_expired_completed_entries() {
        let temp = tempfile::tempdir().unwrap();
        let registry = Arc::new(DeviceRegistry::load(temp.path()).unwrap());
        let manager = PairingManager::new(registry);
        let pairing_id = Uuid::from_u128(1);
        let expired_now = Utc::now() - Duration::minutes(6);
        let pairing_nonce = [7; 32];
        let signing_key = p256::SecretKey::from_slice(&[1; 32]).unwrap();
        let public_key = signing_key.public_key().to_encoded_point(false);
        let request = MobilePairingRequest {
            protocol_version: MOBILE_PROTOCOL_VERSION,
            pairing_nonce: URL_SAFE_NO_PAD.encode(pairing_nonce),
            device_name: "Synthetic phone".to_owned(),
            device_public_key: URL_SAFE_NO_PAD.encode(public_key.as_bytes()),
            app_version: "1.0.0".to_owned(),
            supported_protocol: super::super::contract::MobileProtocolRange {
                minimum: 1,
                maximum: 1,
            },
        };
        manager
            .begin_pairing_with_id(
                pairing_id,
                pairing_nonce,
                expired_now + Duration::minutes(5),
            )
            .unwrap();
        let enrolled = manager
            .enroll(pairing_id, pairing_nonce, request, [3; 32], expired_now)
            .unwrap();
        assert_eq!(
            manager
                .confirm_host(pairing_id, &enrolled.phrase, expired_now)
                .unwrap(),
            PairingProgress::Pending
        );
        assert!(matches!(
            manager
                .confirm_device(pairing_id, &enrolled.phrase, expired_now)
                .unwrap(),
            PairingProgress::Complete {
                replayed: false,
                ..
            }
        ));
        assert_eq!(manager.pending.lock().unwrap().len(), 1);

        let next = manager
            .begin_pairing([9; 32], Utc::now() + Duration::minutes(5))
            .unwrap();

        assert_eq!(manager.pending.lock().unwrap().len(), 1);
        assert!(manager.pending.lock().unwrap().contains_key(&next.id));
        assert!(!manager.pending.lock().unwrap().contains_key(&pairing_id));
        assert_eq!(manager.registry.list_status().unwrap().len(), 1);
    }

    #[test]
    fn expired_or_mismatched_pairing_never_registers_device() {
        let harness = PairingHarness::new();
        assert_eq!(
            harness.enroll_after_expiry().unwrap_err(),
            PairingError::PairingExpired
        );
        assert!(harness.devices().is_empty());
    }

    #[test]
    fn phrase_is_deterministic_for_the_complete_transcript() {
        let transcript = synthetic_transcript();
        assert_eq!(
            derive_pairing_phrase(&transcript),
            ["athlete", "spatial", "stay", "border", "change", "report"].map(str::to_owned)
        );
    }

    #[test]
    fn pairing_word_list_has_2048_unique_words() {
        let words = pairing_words();
        assert_eq!(words.len(), 2048);
        assert_eq!(words.iter().copied().collect::<HashSet<_>>().len(), 2048);
    }

    #[test]
    fn terminal_pairing_output_prints_the_url_once() {
        let expires_at = DateTime::from_timestamp(1_785_326_700, 0).unwrap();
        let invitation = PairingInvitation {
            id: Uuid::from_u128(1),
            nonce: [7; 32],
            expires_at,
        };
        let url = build_pairing_url(&invitation, "https://192.0.2.1:7443", [3; 32]).unwrap();
        let output = render_pairing_invitation(&url, expires_at).unwrap();
        assert_eq!(output.matches(&url).count(), 1);
        assert!(output.contains("Waiting for device"));
        assert!(output.contains("Ctrl-C"));
    }
}
