use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

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
pub const PAIRING_PROTOCOL_MINIMUM_VERSION: u16 = 1;
pub const PAIRING_PROTOCOL_VERSION: u16 = 2;
const PAIRING_SCOPE_MEMORY_READ: &str = "memory_read";
const PAIRING_TRANSCRIPT_V2_DOMAIN: &[u8] = b"COVEN-PAIR/2";
const PAIRING_OFFER_V2_DOMAIN: &[u8] = b"COVEN-PAIR-OFFER/2";

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
    pub terminal: Option<PairingTerminal>,
}

/// Terminal lifecycle states reached before completion. `cancelled` is set by
/// an explicit owner cancellation, `expired` by the natural deadline; both are
/// terminal for further enrollment or confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingTerminal {
    Cancelled,
    Expired,
}

#[derive(Debug, Clone)]
pub struct PendingDevice {
    pub display_name: String,
    pub public_key_x963: String,
    pub app_version: String,
    pub scopes: Vec<DeviceScope>,
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

/// Owner-visible pairing lifecycle. The waiting states distinguish a pairing
/// that has not seen a device yet from one whose phrase is awaiting
/// confirmation, and the terminal states report why a pairing can no longer
/// complete. This surfaces on the owner-only local control route only — a
/// device caller never observes `cancelled` as a distinct state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingState {
    Created,
    Enrolled,
    PartiallyConfirmed,
    Completed,
    Cancelled,
    Expired,
}

/// Owner-facing pairing status. The phrase is only present while the
/// transcript it was derived from still exists, so terminal states never
/// carry material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingStatusReport {
    pub state: PairingState,
    pub phrase: Option<[String; 6]>,
}

/// Outcome of an explicit cancellation. Cancellation is idempotent: every
/// non-`Cancelled` outcome leaves the pairing exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingCancellation {
    /// A live pairing was retired; enrollment and confirmation now fail closed.
    Cancelled,
    /// The pairing had already completed; the registered device grant is kept.
    AlreadyCompleted,
    /// The pairing was already cancelled or expired, or the id is unknown.
    AlreadyTerminal,
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

impl PendingPairing {
    /// Lifecycle state for the owner-facing status surface. A terminal state
    /// always wins, so an expired or cancelled pairing can never look alive
    /// again.
    fn state(&self) -> PairingState {
        if let Some(terminal) = self.terminal {
            return match terminal {
                PairingTerminal::Cancelled => PairingState::Cancelled,
                PairingTerminal::Expired => PairingState::Expired,
            };
        }
        if self.completed.is_some() {
            return PairingState::Completed;
        }
        match (
            self.transcript_hash.is_some(),
            self.host_confirmed || self.device_confirmed,
        ) {
            (false, _) => PairingState::Created,
            (true, false) => PairingState::Enrolled,
            (true, true) => PairingState::PartiallyConfirmed,
        }
    }

    /// Retire a live, uncompleted pairing at its deadline, dropping every
    /// device and transcript material it still holds. Completed pairings are
    /// already terminal, and a terminal state is never overwritten: a
    /// cancelled pairing stays cancelled after its deadline passes.
    fn mark_expired(&mut self) {
        if self.completed.is_some() || self.terminal.is_some() {
            return;
        }
        self.terminal = Some(PairingTerminal::Expired);
        self.transcript_hash = None;
        self.device = None;
    }

    /// Retire a live, uncompleted pairing on explicit owner cancellation,
    /// dropping the device record and transcript material immediately. The
    /// nonce hash is kept so device callers still find the tombstone and fail
    /// closed instead of treating the pairing as never having existed.
    fn mark_cancelled(&mut self) {
        self.terminal = Some(PairingTerminal::Cancelled);
        self.transcript_hash = None;
        self.device = None;
    }
}

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
            terminal: None,
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

    /// Take the pending map, dropping every expired pairing except `addressed`.
    ///
    /// Completed pairings are held until their invitation expires so either
    /// confirmer can retry, so nothing else would evict them once both sides
    /// stop calling; sweeping on each operation bounds the map without a timer.
    /// The addressed pairing survives the sweep so the caller still reports its
    /// own expiry as `PairingExpired` rather than `PairingConsumed`.
    fn lock_pending(
        &self,
        now: DateTime<Utc>,
        addressed: Uuid,
    ) -> Result<MutexGuard<'_, HashMap<Uuid, PendingPairing>>, PairingError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| PairingError::InvalidRequest)?;
        Self::prune_expired(&mut pending, now, |pairing| pairing.id == addressed);
        Ok(pending)
    }

    pub fn enroll(
        &self,
        pairing_id: Uuid,
        nonce: [u8; 32],
        request: MobilePairingRequest,
        host_fingerprint: [u8; 32],
        now: DateTime<Utc>,
    ) -> Result<EnrolledPairing, PairingError> {
        let mut pending = self.lock_pending(now, pairing_id)?;
        let pairing = pending
            .get_mut(&pairing_id)
            .ok_or(PairingError::PairingConsumed)?;
        if now >= pairing.expires_at {
            pairing.mark_expired();
            return Err(PairingError::PairingExpired);
        }
        if pairing.consumed {
            return Err(PairingError::PairingConsumed);
        }
        pairing.consumed = true;
        if Sha256::digest(nonce).as_slice() != pairing.nonce_hash {
            return Err(PairingError::PairingPhraseMismatch);
        }
        if pairing.terminal.is_some() {
            // A cancelled pairing must fail closed even for the correct nonce.
            // Reporting it as consumed keeps the device answer identical to an
            // already-used pairing, so cancellation stays indistinguishable
            // from ordinary consumption until the deadline passes.
            return Err(PairingError::PairingConsumed);
        }
        validate_pairing_request(&request)?;
        let public_key = URL_SAFE_NO_PAD
            .decode(&request.device_public_key)
            .map_err(|_| PairingError::InvalidRequest)?;
        let transcript = PairingTranscript::for_request(
            &request,
            PairingOfferV2 {
                host_fingerprint,
                pairing_id,
                nonce,
                expires_at: pairing.expires_at,
            },
            public_key,
        );
        let transcript_hash = transcript.hash();
        let phrase = derive_pairing_phrase(&transcript);
        pairing.transcript_hash = Some(transcript_hash);
        pairing.device = Some(PendingDevice {
            display_name: request.device_name,
            public_key_x963: request.device_public_key,
            app_version: request.app_version,
            scopes: vec![DeviceScope::MemoryRead],
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

    /// Report the owner-visible lifecycle state of a pairing. Only the phrase
    /// the owner already displays is exposed — never the nonce, transcript, or
    /// device material.
    pub fn status(
        &self,
        pairing_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<PairingStatusReport, PairingError> {
        let mut pending = self.lock_pending(now, pairing_id)?;
        let pairing = pending
            .get_mut(&pairing_id)
            .ok_or(PairingError::PairingConsumed)?;
        if now >= pairing.expires_at {
            pairing.mark_expired();
        }
        Ok(PairingStatusReport {
            state: pairing.state(),
            phrase: pairing.transcript_hash.map(phrase_for_hash),
        })
    }

    /// Cancel a pending pairing on owner request.
    ///
    /// Cancellation is bounded — it sweeps expired entries like every other
    /// operation — and idempotent: cancelling an unknown, already cancelled,
    /// or already expired pairing reports `AlreadyTerminal`, and cancelling a
    /// completed pairing reports `AlreadyCompleted` while leaving the enrolled
    /// device grant untouched. A cancelled pairing drops its transcript and
    /// device material immediately, so enrollment and confirmation fail closed
    /// at once. The tombstone stays until the original deadline and then
    /// answers exactly like a naturally expired pairing, which keeps
    /// cancellation and expiry indistinguishable to an untrusted mobile
    /// caller.
    pub fn cancel(
        &self,
        pairing_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<PairingCancellation, PairingError> {
        let mut pending = self.lock_pending(now, pairing_id)?;
        let Some(pairing) = pending.get_mut(&pairing_id) else {
            return Ok(PairingCancellation::AlreadyTerminal);
        };
        if now >= pairing.expires_at {
            pairing.mark_expired();
            return Ok(PairingCancellation::AlreadyTerminal);
        }
        if pairing.completed.is_some() {
            return Ok(PairingCancellation::AlreadyCompleted);
        }
        if pairing.terminal.is_some() {
            return Ok(PairingCancellation::AlreadyTerminal);
        }
        pairing.mark_cancelled();
        Ok(PairingCancellation::Cancelled)
    }

    fn confirm(
        &self,
        pairing_id: Uuid,
        phrase: &[String],
        now: DateTime<Utc>,
        host: bool,
    ) -> Result<PairingProgress, PairingError> {
        let mut pending = self.lock_pending(now, pairing_id)?;
        let pairing = pending
            .get_mut(&pairing_id)
            .ok_or(PairingError::PairingConsumed)?;
        if now >= pairing.expires_at {
            pairing.mark_expired();
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
            scopes: device.scopes,
        };
        self.registry
            .register(record.clone())
            .map_err(|_| PairingError::InvalidRequest)?;
        let completed = MobilePairedDevice {
            id: record.id,
            display_name: record.display_name,
            paired_at: record.paired_at,
            scopes: record
                .scopes
                .iter()
                .filter_map(|scope| match scope {
                    DeviceScope::MemoryRead => Some(MobileDeviceScope::MemoryRead),
                    _ => None,
                })
                .collect(),
        };
        pairing.device = None;
        pairing.completed = Some(completed.clone());
        Ok(PairingProgress::Complete {
            device: completed,
            replayed: false,
        })
    }
}

#[derive(Debug, Clone)]
enum PairingTranscript {
    V1 {
        protocol_version: u16,
        host_fingerprint: [u8; 32],
        pairing_id: Uuid,
        device_public_key: Vec<u8>,
        nonce: [u8; 32],
    },
    V2 {
        offer_digest: [u8; 32],
        protocol_version: u16,
        supported_minimum: u16,
        supported_maximum: u16,
        device_public_key: Vec<u8>,
        device_name: String,
        app_version: String,
    },
}

impl PairingTranscript {
    fn for_request(
        request: &MobilePairingRequest,
        offer: PairingOfferV2,
        device_public_key: Vec<u8>,
    ) -> Self {
        if request.protocol_version == MOBILE_PROTOCOL_VERSION {
            Self::V1 {
                protocol_version: request.protocol_version,
                host_fingerprint: offer.host_fingerprint,
                pairing_id: offer.pairing_id,
                device_public_key,
                nonce: offer.nonce,
            }
        } else {
            Self::V2 {
                offer_digest: offer.hash(),
                protocol_version: request.protocol_version,
                supported_minimum: request.supported_protocol.minimum,
                supported_maximum: request.supported_protocol.maximum,
                device_public_key,
                device_name: request.device_name.clone(),
                app_version: request.app_version.clone(),
            }
        }
    }

    fn hash(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        match self {
            Self::V1 {
                protocol_version,
                host_fingerprint,
                pairing_id,
                device_public_key,
                nonce,
            } => {
                let protocol_version = protocol_version.to_be_bytes();
                for field in [
                    protocol_version.as_slice(),
                    host_fingerprint.as_slice(),
                    pairing_id.as_bytes(),
                    device_public_key.as_slice(),
                    nonce.as_slice(),
                ] {
                    update_length_prefixed(&mut digest, field);
                }
            }
            Self::V2 {
                offer_digest,
                protocol_version,
                supported_minimum,
                supported_maximum,
                device_public_key,
                device_name,
                app_version,
            } => {
                let protocol_version = protocol_version.to_be_bytes();
                let supported_minimum = supported_minimum.to_be_bytes();
                let supported_maximum = supported_maximum.to_be_bytes();
                for field in [
                    PAIRING_TRANSCRIPT_V2_DOMAIN,
                    offer_digest.as_slice(),
                    protocol_version.as_slice(),
                    supported_minimum.as_slice(),
                    supported_maximum.as_slice(),
                    device_public_key.as_slice(),
                    device_name.as_bytes(),
                    app_version.as_bytes(),
                ] {
                    update_length_prefixed(&mut digest, field);
                }
            }
        }
        digest.finalize().into()
    }
}

#[derive(Debug, Clone, Copy)]
struct PairingOfferV2 {
    host_fingerprint: [u8; 32],
    pairing_id: Uuid,
    nonce: [u8; 32],
    expires_at: DateTime<Utc>,
}

impl PairingOfferV2 {
    fn hash(&self) -> [u8; 32] {
        let minimum_version = PAIRING_PROTOCOL_MINIMUM_VERSION.to_be_bytes();
        let maximum_version = PAIRING_PROTOCOL_VERSION.to_be_bytes();
        let expires_at = self.expires_at.timestamp().to_be_bytes();
        let mut digest = Sha256::new();
        for field in [
            PAIRING_OFFER_V2_DOMAIN,
            minimum_version.as_slice(),
            maximum_version.as_slice(),
            self.host_fingerprint.as_slice(),
            self.pairing_id.as_bytes(),
            self.nonce.as_slice(),
            expires_at.as_slice(),
            PAIRING_SCOPE_MEMORY_READ.as_bytes(),
        ] {
            update_length_prefixed(&mut digest, field);
        }
        digest.finalize().into()
    }
}

fn update_length_prefixed(digest: &mut Sha256, field: &[u8]) {
    digest.update((field.len() as u32).to_be_bytes());
    digest.update(field);
}

fn validate_pairing_request(request: &MobilePairingRequest) -> Result<(), PairingError> {
    let selected_version_supported = request.supported_protocol.minimum <= request.protocol_version
        && request.supported_protocol.maximum >= request.protocol_version;
    let supported_version = matches!(
        request.protocol_version,
        MOBILE_PROTOCOL_VERSION | PAIRING_PROTOCOL_VERSION
    );
    if !supported_version
        || !selected_version_supported
        || request.supported_protocol.minimum > request.supported_protocol.maximum
        || request.device_name.is_empty()
        || request.device_name.trim() != request.device_name
        || request.device_name.chars().count() > 80
        || request.device_name.chars().any(char::is_control)
        || request.app_version.is_empty()
        || request.app_version.len() > 64
        || !request.app_version.is_ascii()
        || request.app_version.chars().any(char::is_control)
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
    let endpoint = validate_pairing_endpoint(endpoint)?;
    let offer = PairingOfferV2 {
        host_fingerprint,
        pairing_id: invitation.id,
        nonce: invitation.nonce,
        expires_at: invitation.expires_at,
    };
    let mut url = Url::parse("coven-memory://pair").map_err(|_| PairingError::InvalidRequest)?;
    url.query_pairs_mut()
        .append_pair("version", &PAIRING_PROTOCOL_VERSION.to_string())
        .append_pair(
            "minimumVersion",
            &PAIRING_PROTOCOL_MINIMUM_VERSION.to_string(),
        )
        .append_pair("maximumVersion", &PAIRING_PROTOCOL_VERSION.to_string())
        .append_pair("pairingId", &invitation.id.to_string())
        .append_pair("endpoint", endpoint.as_str())
        .append_pair("fingerprint", &URL_SAFE_NO_PAD.encode(host_fingerprint))
        .append_pair("nonce", &URL_SAFE_NO_PAD.encode(invitation.nonce))
        .append_pair("expires", &invitation.expires_at.timestamp().to_string())
        .append_pair("scope", PAIRING_SCOPE_MEMORY_READ)
        .append_pair("offerDigest", &URL_SAFE_NO_PAD.encode(offer.hash()));
    Ok(url.into())
}

fn validate_pairing_endpoint(endpoint: &str) -> Result<Url, PairingError> {
    let mut endpoint = Url::parse(endpoint).map_err(|_| PairingError::InvalidRequest)?;
    if endpoint.scheme() != "https"
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.path() != "/"
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(PairingError::InvalidRequest);
    }
    endpoint.set_fragment(None);
    Ok(endpoint)
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

        fn use_v2(&mut self) {
            self.request.protocol_version = PAIRING_PROTOCOL_VERSION;
            self.request.supported_protocol.minimum = PAIRING_PROTOCOL_MINIMUM_VERSION;
            self.request.supported_protocol.maximum = PAIRING_PROTOCOL_VERSION;
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
        PairingTranscript::V1 {
            protocol_version: 1,
            host_fingerprint: [3; 32],
            pairing_id: Uuid::from_u128(1),
            device_public_key: vec![4; 65],
            nonce: [7; 32],
        }
    }

    fn synthetic_v2_transcript() -> PairingTranscript {
        PairingTranscript::V2 {
            offer_digest: [2; 32],
            protocol_version: 2,
            supported_minimum: 1,
            supported_maximum: 2,
            device_public_key: vec![4; 65],
            device_name: "Synthetic phone".to_owned(),
            app_version: "1.0.0".to_owned(),
        }
    }

    fn decode_hex(value: &str) -> Vec<u8> {
        assert_eq!(value.len() % 2, 0);
        value
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
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
    fn pairing_v2_requires_host_and_device_confirmation() {
        let mut harness = PairingHarness::new();
        harness.use_v2();
        let pending = harness.enroll();
        assert_eq!(
            harness.confirm_device(&pending.phrase),
            PairingProgress::Pending
        );
        let device = assert_complete(harness.confirm_host(&pending.phrase), false);
        assert_eq!(device.scopes, [MobileDeviceScope::MemoryRead]);
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
    fn unsupported_pairing_protocol_is_rejected() {
        let mut harness = PairingHarness::new();
        harness.request.protocol_version = 3;
        harness.request.supported_protocol.maximum = 3;
        assert_eq!(
            harness
                .enroll_with_nonce(harness.pairing_nonce)
                .unwrap_err(),
            PairingError::InvalidRequest
        );
        assert!(harness.devices().is_empty());
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
    fn any_pairing_operation_prunes_other_expired_entries() {
        for prune_via in ["phrase", "confirm", "enroll"] {
            let harness = PairingHarness::new();
            let pending = harness.enroll();
            harness.confirm_host(&pending.phrase);
            assert_complete(harness.confirm_device(&pending.phrase), false);

            let live_id = Uuid::from_u128(2);
            let live_nonce = [11; 32];
            harness
                .manager
                .begin_pairing_with_id(live_id, live_nonce, harness.now + Duration::minutes(30))
                .unwrap();
            assert_eq!(harness.manager.pending.lock().unwrap().len(), 2);

            let after_first_expiry = harness.now + Duration::minutes(6);
            match prune_via {
                "phrase" => {
                    let report = harness.manager.status(live_id, after_first_expiry).unwrap();
                    assert_eq!(report.state, PairingState::Created);
                    assert_eq!(report.phrase, None);
                }
                "confirm" => assert_eq!(
                    harness
                        .manager
                        .confirm_host(live_id, &pending.phrase, after_first_expiry)
                        .unwrap_err(),
                    PairingError::PairingConfirmationRequired
                ),
                _ => {
                    harness
                        .manager
                        .enroll(
                            live_id,
                            live_nonce,
                            harness.request.clone(),
                            [3; 32],
                            after_first_expiry,
                        )
                        .unwrap();
                }
            }

            let remaining = harness.manager.pending.lock().unwrap();
            assert_eq!(
                remaining.len(),
                1,
                "{prune_via} left the expired completed pairing behind"
            );
            assert!(remaining.contains_key(&live_id));
        }
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
    fn cancel_before_enrollment_fails_closed() {
        let harness = PairingHarness::new();
        assert_eq!(
            harness
                .manager
                .cancel(harness.pairing_id, harness.now)
                .unwrap(),
            PairingCancellation::Cancelled
        );
        assert_eq!(
            harness
                .enroll_with_nonce(harness.pairing_nonce)
                .unwrap_err(),
            PairingError::PairingConsumed
        );
        let report = harness
            .manager
            .status(harness.pairing_id, harness.now)
            .unwrap();
        assert_eq!(report.state, PairingState::Cancelled);
        assert_eq!(report.phrase, None);
        assert!(harness.devices().is_empty());
    }

    #[test]
    fn cancel_after_enrollment_blocks_confirmation_and_drops_material() {
        let harness = PairingHarness::new();
        let pending = harness.enroll();
        assert_eq!(
            harness
                .manager
                .cancel(harness.pairing_id, harness.now)
                .unwrap(),
            PairingCancellation::Cancelled
        );
        assert_eq!(
            harness
                .manager
                .confirm_host(harness.pairing_id, &pending.phrase, harness.now)
                .unwrap_err(),
            PairingError::PairingConfirmationRequired
        );
        assert_eq!(
            harness
                .manager
                .confirm_device(harness.pairing_id, &pending.phrase, harness.now)
                .unwrap_err(),
            PairingError::PairingConfirmationRequired
        );
        let report = harness
            .manager
            .status(harness.pairing_id, harness.now)
            .unwrap();
        assert_eq!(report.state, PairingState::Cancelled);
        assert_eq!(report.phrase, None);
        assert!(harness.devices().is_empty());
    }

    #[test]
    fn cancel_after_one_confirmation_registers_no_device() {
        let harness = PairingHarness::new();
        let pending = harness.enroll();
        assert_eq!(
            harness.confirm_host(&pending.phrase),
            PairingProgress::Pending
        );
        assert_eq!(
            harness
                .manager
                .cancel(harness.pairing_id, harness.now)
                .unwrap(),
            PairingCancellation::Cancelled
        );
        assert_eq!(
            harness
                .manager
                .confirm_device(harness.pairing_id, &pending.phrase, harness.now)
                .unwrap_err(),
            PairingError::PairingConfirmationRequired
        );
        assert!(harness.devices().is_empty());
    }

    #[test]
    fn repeated_cancellation_is_idempotent_and_cannot_resurrect() {
        let harness = PairingHarness::new();
        let pending = harness.enroll();
        assert_eq!(
            harness
                .manager
                .cancel(harness.pairing_id, harness.now)
                .unwrap(),
            PairingCancellation::Cancelled
        );
        assert_eq!(
            harness
                .manager
                .cancel(harness.pairing_id, harness.now)
                .unwrap(),
            PairingCancellation::AlreadyTerminal
        );
        assert_eq!(
            harness
                .manager
                .cancel(harness.pairing_id, harness.now + Duration::minutes(6))
                .unwrap(),
            PairingCancellation::AlreadyTerminal
        );
        assert_eq!(
            harness
                .enroll_with_nonce(harness.pairing_nonce)
                .unwrap_err(),
            PairingError::PairingConsumed
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
        assert!(harness.devices().is_empty());
    }

    #[test]
    fn cancel_of_completed_pairing_preserves_the_device_grant() {
        let harness = PairingHarness::new();
        let pending = harness.enroll();
        assert_eq!(
            harness.confirm_host(&pending.phrase),
            PairingProgress::Pending
        );
        let device = assert_complete(harness.confirm_device(&pending.phrase), false);
        assert_eq!(
            harness
                .manager
                .cancel(harness.pairing_id, harness.now)
                .unwrap(),
            PairingCancellation::AlreadyCompleted
        );
        assert_eq!(harness.devices().len(), 1);
        assert_eq!(harness.devices()[0].id, device.id);
        assert_eq!(
            assert_complete(harness.confirm_host(&pending.phrase), true),
            device
        );
    }

    #[test]
    fn cancelled_pairings_expire_like_naturally_expired_pairings() {
        let harness = PairingHarness::new();
        let cancelled_id = harness.pairing_id;
        let expired_id = Uuid::from_u128(2);
        let expired_nonce = [31_u8; 32];
        harness
            .manager
            .begin_pairing_with_id(
                expired_id,
                expired_nonce,
                harness.now + Duration::minutes(5),
            )
            .unwrap();
        let cancelled_phrase = harness.enroll().phrase;
        let expired_phrase = harness
            .manager
            .enroll(
                expired_id,
                expired_nonce,
                harness.request.clone(),
                [3; 32],
                harness.now,
            )
            .unwrap()
            .phrase;
        assert_eq!(
            harness.manager.cancel(cancelled_id, harness.now).unwrap(),
            PairingCancellation::Cancelled
        );
        let after = harness.now + Duration::minutes(6);

        for (pairing_id, nonce, phrase) in [
            (cancelled_id, harness.pairing_nonce, cancelled_phrase),
            (expired_id, expired_nonce, expired_phrase),
        ] {
            assert_eq!(
                harness
                    .manager
                    .confirm_device(pairing_id, &phrase, after)
                    .unwrap_err(),
                PairingError::PairingExpired
            );
            assert_eq!(
                harness
                    .manager
                    .enroll_by_nonce(nonce, harness.request.clone(), [3; 32], after)
                    .unwrap_err(),
                PairingError::PairingExpired
            );
        }
        assert!(harness.devices().is_empty());
    }

    #[test]
    fn status_reports_each_lifecycle_state() {
        let harness = PairingHarness::new();
        assert_eq!(
            harness
                .manager
                .status(harness.pairing_id, harness.now)
                .unwrap()
                .state,
            PairingState::Created
        );
        let pending = harness.enroll();
        assert_eq!(
            harness
                .manager
                .status(harness.pairing_id, harness.now)
                .unwrap()
                .state,
            PairingState::Enrolled
        );
        assert_eq!(
            harness.confirm_host(&pending.phrase),
            PairingProgress::Pending
        );
        assert_eq!(
            harness
                .manager
                .status(harness.pairing_id, harness.now)
                .unwrap()
                .state,
            PairingState::PartiallyConfirmed
        );
        assert_complete(harness.confirm_device(&pending.phrase), false);
        assert_eq!(
            harness
                .manager
                .status(harness.pairing_id, harness.now)
                .unwrap()
                .state,
            PairingState::Completed
        );
    }

    #[test]
    fn status_reports_expiry_without_keeping_material() {
        let harness = PairingHarness::new();
        harness.enroll();
        let report = harness
            .manager
            .status(harness.pairing_id, harness.now + Duration::minutes(6))
            .unwrap();
        assert_eq!(report.state, PairingState::Expired);
        assert_eq!(report.phrase, None);
        assert!(harness.devices().is_empty());
    }

    #[test]
    fn phrase_is_deterministic_for_the_v1_transcript() {
        let transcript = synthetic_transcript();
        assert_eq!(
            derive_pairing_phrase(&transcript),
            ["athlete", "spatial", "stay", "border", "change", "report"].map(str::to_owned)
        );
    }

    #[test]
    fn pairing_v2_binds_offer_and_client_metadata() {
        let baseline = synthetic_v2_transcript();
        let baseline_hash = baseline.hash();

        let mut changed = synthetic_v2_transcript();
        if let PairingTranscript::V2 { offer_digest, .. } = &mut changed {
            *offer_digest = [3; 32];
        }
        assert_ne!(baseline_hash, changed.hash());

        let mut changed = synthetic_v2_transcript();
        if let PairingTranscript::V2 { device_name, .. } = &mut changed {
            device_name.push_str(" other");
        }
        assert_ne!(baseline_hash, changed.hash());

        let mut changed = synthetic_v2_transcript();
        if let PairingTranscript::V2 { app_version, .. } = &mut changed {
            app_version.push_str("-other");
        }
        assert_ne!(baseline_hash, changed.hash());
    }

    #[test]
    fn pairing_offer_binds_security_relevant_fields() {
        let expires_at = DateTime::from_timestamp(1_785_326_700, 0).unwrap();
        let baseline = PairingOfferV2 {
            host_fingerprint: [3; 32],
            pairing_id: Uuid::from_u128(1),
            nonce: [7; 32],
            expires_at,
        };
        let baseline_hash = baseline.hash();

        let mut changed = baseline;
        changed.host_fingerprint = [4; 32];
        assert_ne!(baseline_hash, changed.hash());
        let mut changed = baseline;
        changed.pairing_id = Uuid::from_u128(2);
        assert_ne!(baseline_hash, changed.hash());
        let mut changed = baseline;
        changed.nonce = [8; 32];
        assert_ne!(baseline_hash, changed.hash());
        let mut changed = baseline;
        changed.expires_at += Duration::seconds(1);
        assert_ne!(baseline_hash, changed.hash());
    }

    #[test]
    fn portable_pairing_v2_vector_matches_the_contract() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/mobile-pairing-v2/transcript-vector.json"
        ))
        .unwrap();
        let host_fingerprint: [u8; 32] = decode_hex(vector["hostFingerprintHex"].as_str().unwrap())
            .try_into()
            .unwrap();
        let nonce: [u8; 32] = decode_hex(vector["pairingNonceHex"].as_str().unwrap())
            .try_into()
            .unwrap();
        let offer = PairingOfferV2 {
            host_fingerprint,
            pairing_id: Uuid::parse_str(vector["pairingId"].as_str().unwrap()).unwrap(),
            nonce,
            expires_at: DateTime::from_timestamp(vector["expiresAtUnix"].as_i64().unwrap(), 0)
                .unwrap(),
        };
        assert_eq!(
            encode_hex(&offer.hash()),
            vector["offerDigestHex"].as_str().unwrap()
        );
        assert_eq!(
            vector["requestedScopes"],
            serde_json::json!(["memory_read"])
        );

        let transcript = PairingTranscript::V2 {
            offer_digest: offer.hash(),
            protocol_version: vector["pairingProtocolVersion"].as_u64().unwrap() as u16,
            supported_minimum: vector["minimumPairingProtocolVersion"].as_u64().unwrap() as u16,
            supported_maximum: vector["maximumPairingProtocolVersion"].as_u64().unwrap() as u16,
            device_public_key: decode_hex(vector["devicePublicKeyX963Hex"].as_str().unwrap()),
            device_name: vector["deviceName"].as_str().unwrap().to_owned(),
            app_version: vector["appVersion"].as_str().unwrap().to_owned(),
        };
        assert_eq!(
            encode_hex(&transcript.hash()),
            vector["transcriptDigestHex"].as_str().unwrap()
        );
    }

    #[test]
    fn pairing_word_list_has_2048_unique_words() {
        let words = pairing_words();
        assert_eq!(words.len(), 2048);
        assert_eq!(words.iter().copied().collect::<HashSet<_>>().len(), 2048);
    }

    #[test]
    fn terminal_pairing_output_prints_a_v2_offer_once() {
        let expires_at = DateTime::from_timestamp(1_785_326_700, 0).unwrap();
        let invitation = PairingInvitation {
            id: Uuid::from_u128(1),
            nonce: [7; 32],
            expires_at,
        };
        let url = build_pairing_url(&invitation, "https://192.0.2.1:7443", [3; 32]).unwrap();
        let parsed = Url::parse(&url).unwrap();
        let query: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(query["version"], PAIRING_PROTOCOL_VERSION.to_string());
        assert_eq!(
            query["minimumVersion"],
            PAIRING_PROTOCOL_MINIMUM_VERSION.to_string()
        );
        assert_eq!(query["pairingId"], invitation.id.to_string());
        assert_eq!(query["scope"], PAIRING_SCOPE_MEMORY_READ);
        let expected_offer = PairingOfferV2 {
            host_fingerprint: [3; 32],
            pairing_id: invitation.id,
            nonce: invitation.nonce,
            expires_at,
        };
        assert_eq!(
            query["offerDigest"],
            URL_SAFE_NO_PAD.encode(expected_offer.hash())
        );

        let output = render_pairing_invitation(&url, expires_at).unwrap();
        assert_eq!(output.matches(&url).count(), 1);
        assert!(output.contains("Waiting for device"));
        assert!(output.contains("Ctrl-C"));
    }

    #[test]
    fn pairing_url_rejects_ambiguous_or_unencrypted_endpoints() {
        let invitation = PairingInvitation {
            id: Uuid::from_u128(1),
            nonce: [7; 32],
            expires_at: DateTime::from_timestamp(1_785_326_700, 0).unwrap(),
        };
        for endpoint in [
            "http://192.0.2.1:7443",
            "https://user@192.0.2.1:7443",
            "https://192.0.2.1:7443/path",
            "https://192.0.2.1:7443?other=value",
            "https://192.0.2.1:7443#fragment",
        ] {
            assert_eq!(
                build_pairing_url(&invitation, endpoint, [3; 32]).unwrap_err(),
                PairingError::InvalidRequest
            );
        }
    }
}
