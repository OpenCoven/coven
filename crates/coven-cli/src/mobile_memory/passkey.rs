//! Optional passkey recovery factor. A passkey is an owner credential, never a
//! device: it has no `DeviceRecord`, grant or introducer identity, and it may
//! count as at most one approval of a threshold introduction that still needs
//! at least one trusted device. WebAuthn ES256 assertions are verified here in
//! Rust; there is no vendor SDK or hosted account service in the decision.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use p256::elliptic_curve::sec1::ToSec1Point;
use p256::pkcs8::DecodePublicKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::assurance::{validate_public_key, verify_assurance_signature};
use super::audit::{append_event, append_policy_event, MobileAuditEvent};
use super::config::{atomic_replace_private, ensure_private_mobile_dir, validate_private_file};
use super::grant::{DeviceActionIntent, DeviceScope, DEVICE_ACTION_VERSION};
use super::owner_policy::{InstalledPolicy, OwnerPolicyStore};

const PASSKEY_FILE: &str = "passkeys.json";
const PASSKEY_LOCK_FILE: &str = ".passkeys.lock";
const PASSKEY_STORE_VERSION: u16 = 2;
const MAX_PASSKEYS: usize = 8;
const REGISTER_OPERATION: &str = "owner.passkey.register";
const FLAG_USER_PRESENT: u8 = 0x01;
const FLAG_USER_VERIFIED: u8 = 0x04;
const FLAG_BACKUP_ELIGIBLE: u8 = 0x08;
const FLAG_BACKUP_STATE: u8 = 0x10;
static PASSKEY_WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug)]
struct PasskeyRegistrationUncertain;

impl std::fmt::Display for PasskeyRegistrationUncertain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("passkey registration outcome uncertain; reload persisted state")
    }
}

#[derive(Debug)]
struct PasskeyRegistrationAuditPending;

impl std::fmt::Display for PasskeyRegistrationAuditPending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("passkey registration committed; audit reconciliation pending")
    }
}

#[cfg(test)]
thread_local! {
    /// Runs inside registration after every check passes, before the store write.
    static BEFORE_PASSKEY_WRITE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static FAIL_AFTER_PASSKEY_REPLACE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// One WebAuthn `navigator.credentials.get()` result, as the client returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PasskeyAssertion {
    pub credential_id: String,
    pub authenticator_data: String,
    pub client_data_json: String,
    pub signature: String,
}

/// What the owner registers. `public_key_spki` is the credential's SPKI DER
/// (`AuthenticatorAttestationResponse.getPublicKey()`), base64url.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PasskeyRegistration {
    pub credential_id: String,
    pub rp_id: String,
    pub origins: Vec<String>,
    pub public_key_spki: String,
    pub nonce: [u8; 32],
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PasskeyCredential {
    pub credential_id: String,
    pub rp_id: String,
    pub origins: Vec<String>,
    pub public_key_x963: String,
    /// From authenticator flags at registration: a backed-up (synced) passkey
    /// is recorded as such and is never evidence of one physical device.
    pub backup_eligible: bool,
    pub backed_up: bool,
    pub sign_count: u32,
    pub registered_at: DateTime<Utc>,
    pub owner_root_key: String,
    pub owner_root_generation: String,
    pub registration_transition_id: Uuid,
    pub registration_audited_at: Option<DateTime<Utc>>,
    pub registration_action: DeviceActionIntent,
    pub root_signature: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredPasskeys {
    version: u16,
    audience: String,
    credentials: Vec<PasskeyCredential>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationMaterial<'a> {
    audience: &'a str,
    owner_root_key: &'a str,
    owner_root_generation: &'a str,
    credential_id: &'a str,
    rp_id: &'a str,
    origins: &'a [String],
    public_key_x963: &'a str,
}

impl RegistrationMaterial<'_> {
    fn digest(&self) -> Result<String> {
        let mut material = b"COVEN-PASSKEY/1\0".to_vec();
        material.extend(serde_jcs::to_vec(self)?);
        Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(material)))
    }
}

struct PasskeyStoreLock {
    _process: MutexGuard<'static, ()>,
    file: fs::File,
}

impl PasskeyStoreLock {
    fn acquire(directory: &Path) -> Result<Self> {
        let process = PASSKEY_WRITE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("passkey store lock was poisoned"))?;
        let path = directory.join(PASSKEY_LOCK_FILE);
        let file = crate::state_lock::open_lock_file(&path)?;
        file.lock_exclusive()
            .with_context(|| format!("failed to lock {}", path.display()))?;
        Ok(Self {
            _process: process,
            file,
        })
    }
}

impl Drop for PasskeyStoreLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub struct PasskeyStore {
    home: PathBuf,
    path: PathBuf,
    audience: String,
}

impl PasskeyStore {
    pub fn open(home: &Path, audience: [u8; 32]) -> Result<Self> {
        Ok(Self {
            home: home.to_owned(),
            path: ensure_private_mobile_dir(home)?.join(PASSKEY_FILE),
            audience: URL_SAFE_NO_PAD.encode(audience),
        })
    }

    pub fn list(&self) -> Result<Vec<PasskeyCredential>> {
        let _lock = PasskeyStoreLock::acquire(self.lock_dir()?)?;
        self.read()
    }

    /// The exact action the owner root signs, and whose canonical-bytes hash is
    /// the WebAuthn challenge of the possession assertion.
    pub fn registration_action(
        &self,
        registration: &PasskeyRegistration,
    ) -> Result<DeviceActionIntent> {
        let head = self.owner_policy()?;
        self.registration_action_for(registration, &head)
    }

    fn registration_action_for(
        &self,
        registration: &PasskeyRegistration,
        head: &InstalledPolicy,
    ) -> Result<DeviceActionIntent> {
        let public = spki_to_x963(&registration.public_key_spki)?;
        validate_registration(registration)?;
        let effect_digest = RegistrationMaterial {
            audience: &self.audience,
            owner_root_key: &head.owner_root_key,
            owner_root_generation: &head.root_generation,
            credential_id: &registration.credential_id,
            rp_id: &registration.rp_id,
            origins: &registration.origins,
            public_key_x963: &public,
        }
        .digest()?;
        Ok(DeviceActionIntent {
            version: DEVICE_ACTION_VERSION,
            scope: DeviceScope::DeviceAdmin,
            operation: REGISTER_OPERATION.into(),
            target: self.audience.clone(),
            effect_digest,
            nonce: URL_SAFE_NO_PAD.encode(registration.nonce),
            issued_at: registration.issued_at,
            expires_at: registration.expires_at,
        })
    }

    /// Registers a passkey only with the owner-root signature and a WebAuthn
    /// assertion from that credential over the same action.
    pub fn register(
        &self,
        registration: &PasskeyRegistration,
        possession: &PasskeyAssertion,
        root_signature: &str,
        now: DateTime<Utc>,
    ) -> Result<PasskeyCredential> {
        // Policy → passkey: the root is read and the registration written under
        // one policy lock, so a rotation cannot land between them.
        let result = self.policy_store().and_then(|policy| {
            policy.with_current_locked(|head| {
                let head = head.context("passkeys require an installed owner policy and root")?;
                let root = &head.owner_root_key;
                let _lock = PasskeyStoreLock::acquire(self.lock_dir()?)?;
                let mut credentials = self.read()?;
                let action = self.registration_action_for(registration, head)?;
                if action.issued_at > now || now >= action.expires_at {
                    bail!("passkey registration is outside its validity window");
                }
                let bytes = action.canonical_bytes()?;
                verify_assurance_signature(root, &bytes, root_signature)
                    .context("passkey registration requires the owner-root signature")?;
                let public = spki_to_x963(&registration.public_key_spki)?;
                let mut credential = PasskeyCredential {
                    credential_id: registration.credential_id.clone(),
                    rp_id: registration.rp_id.clone(),
                    origins: registration.origins.clone(),
                    public_key_x963: public,
                    backup_eligible: false,
                    backed_up: false,
                    sign_count: 0,
                    registered_at: now,
                    owner_root_key: head.owner_root_key.clone(),
                    owner_root_generation: head.root_generation.clone(),
                    registration_transition_id: Uuid::new_v4(),
                    registration_audited_at: None,
                    registration_action: action,
                    root_signature: root_signature.to_owned(),
                };
                let verified =
                    verify_assertion(&credential, possession, &Sha256::digest(&bytes).into())?;
                credential.backup_eligible = verified.backup_eligible;
                credential.backed_up = verified.backed_up;
                credential.sign_count = verified.sign_count;
                if credentials
                    .iter()
                    .any(|existing| existing.credential_id == credential.credential_id)
                {
                    bail!("passkey is already registered");
                }
                if credentials.len() >= MAX_PASSKEYS {
                    bail!("too many passkeys are registered");
                }
                credentials.push(credential.clone());
                #[cfg(test)]
                BEFORE_PASSKEY_WRITE.with(|hook| hook.borrow_mut().take().map(|hook| hook()));
                // The outbox is persisted with the credential. A replacement
                // error can occur after rename, so it is never a rejection.
                self.write(&credentials)
                    .context(PasskeyRegistrationUncertain)?;
                self.read()
                    .context(PasskeyRegistrationAuditPending)?
                    .into_iter()
                    .find(|record| {
                        record.registration_transition_id == credential.registration_transition_id
                    })
                    .context(PasskeyRegistrationAuditPending)
            })
        });
        match result {
            Ok(credential) => Ok(credential),
            Err(error) => {
                let event = if error.is::<PasskeyRegistrationUncertain>()
                    || error.is::<PasskeyRegistrationAuditPending>()
                {
                    MobileAuditEvent::PasskeyRegistrationUncertain
                } else {
                    MobileAuditEvent::PasskeyRegistrationRejected
                };
                if let Err(audit_error) = append_event(&self.home, now, event, None) {
                    return Err(
                        error.context(format!("passkey audit delivery failed: {audit_error:#}"))
                    );
                }
                Err(error)
            }
        }
    }

    /// Verifies one assertion over `challenge` and advances its sign counter.
    /// Callers hold their own commit locks; this store's lock is taken last.
    pub(super) fn verify_and_advance(
        &self,
        assertion: &PasskeyAssertion,
        challenge: &[u8; 32],
    ) -> Result<PasskeyCredential> {
        let _lock = PasskeyStoreLock::acquire(self.lock_dir()?)?;
        let mut credentials = self.read()?;
        let credential = credentials
            .iter_mut()
            .find(|credential| credential.credential_id == assertion.credential_id)
            .context("passkey is not registered")?;
        let verified = verify_assertion(credential, assertion, challenge)?;
        if verified.backup_eligible != credential.backup_eligible {
            bail!("passkey backup eligibility changed from its registered value");
        }
        credential.sign_count = verified.sign_count;
        credential.backed_up = verified.backed_up;
        let result = credential.clone();
        self.write(&credentials)?;
        Ok(result)
    }

    fn policy_store(&self) -> Result<OwnerPolicyStore> {
        let audience: [u8; 32] = URL_SAFE_NO_PAD
            .decode(&self.audience)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid passkey store audience"))?;
        OwnerPolicyStore::open(&self.home, audience)
    }

    fn owner_policy(&self) -> Result<InstalledPolicy> {
        self.policy_store()?
            .current()?
            .context("passkeys require an installed owner policy and root")
    }

    fn lock_dir(&self) -> Result<&Path> {
        self.path
            .parent()
            .context("passkey store must have a parent directory")
    }

    fn read(&self) -> Result<Vec<PasskeyCredential>> {
        match fs::symlink_metadata(&self.path) {
            Ok(_) => validate_private_file(&self.path)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect {}", self.path.display()))
            }
        }
        let mut stored: StoredPasskeys = serde_json::from_slice(
            &fs::read(&self.path)
                .with_context(|| format!("failed to read {}", self.path.display()))?,
        )
        .with_context(|| format!("failed to parse {}", self.path.display()))?;
        if stored.version != PASSKEY_STORE_VERSION || stored.audience != self.audience {
            bail!("passkey store is bound to another version or installation");
        }
        // Validate every record against its captured signing root before
        // delivering pending audit, including records retired by rotation.
        for credential in &stored.credentials {
            validate_public_key(&credential.public_key_x963)?;
            validate_public_key(&credential.owner_root_key)?;
            decode(&credential.owner_root_generation, 32, 32)?;
            if credential.registration_action.operation != REGISTER_OPERATION
                || credential.registration_action.target != self.audience
                || credential.registration_action.scope != DeviceScope::DeviceAdmin
                || credential.registration_transition_id.is_nil()
                || credential
                    .registration_audited_at
                    .is_some_and(|at| at < credential.registered_at)
            {
                bail!("passkey registration does not match its record");
            }
            verify_assurance_signature(
                &credential.owner_root_key,
                &credential.registration_action.canonical_bytes()?,
                &credential.root_signature,
            )
            .context("passkey registration root signature is invalid")?;
            let digest = RegistrationMaterial {
                audience: &self.audience,
                owner_root_key: &credential.owner_root_key,
                owner_root_generation: &credential.owner_root_generation,
                credential_id: &credential.credential_id,
                rp_id: &credential.rp_id,
                origins: &credential.origins,
                public_key_x963: &credential.public_key_x963,
            }
            .digest()?;
            if digest != credential.registration_action.effect_digest {
                bail!("passkey registration material does not match the signed action");
            }
        }
        let mut reconciled = false;
        for credential in &mut stored.credentials {
            if credential.registration_audited_at.is_none() {
                // The shared audit transition writer deduplicates by UUID,
                // including retries after audit succeeded but receipt failed.
                append_policy_event(
                    &self.home,
                    credential.registration_transition_id,
                    credential.registered_at,
                    MobileAuditEvent::PasskeyRegistered,
                )
                .context(PasskeyRegistrationAuditPending)?;
                credential.registration_audited_at = Some(credential.registered_at);
                reconciled = true;
            }
        }
        if reconciled {
            self.write(&stored.credentials)
                .context(PasskeyRegistrationAuditPending)?;
        }
        let head = self.owner_policy()?;
        Ok(stored
            .credentials
            .into_iter()
            .filter(|credential| {
                credential.owner_root_key == head.owner_root_key
                    && credential.owner_root_generation == head.root_generation
            })
            .collect())
    }

    fn write(&self, credentials: &[PasskeyCredential]) -> Result<()> {
        let mut encoded = serde_json::to_vec_pretty(&StoredPasskeys {
            version: PASSKEY_STORE_VERSION,
            audience: self.audience.clone(),
            credentials: credentials.to_vec(),
        })?;
        encoded.push(b'\n');
        atomic_replace_private(&self.path, &encoded)?;
        #[cfg(test)]
        if FAIL_AFTER_PASSKEY_REPLACE.with(|fail| fail.replace(false)) {
            bail!("injected failure after passkey replacement");
        }
        Ok(())
    }
}

struct VerifiedAssertion {
    sign_count: u32,
    backup_eligible: bool,
    backed_up: bool,
}

#[derive(Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    kind: String,
    challenge: String,
    origin: String,
    #[serde(rename = "crossOrigin", default)]
    cross_origin: bool,
}

/// WebAuthn L2 §7.2 for ES256: client data type, challenge and origin, RP ID
/// hash, user presence *and* verification, signature, and sign counter.
fn verify_assertion(
    credential: &PasskeyCredential,
    assertion: &PasskeyAssertion,
    challenge: &[u8; 32],
) -> Result<VerifiedAssertion> {
    if assertion.credential_id != credential.credential_id {
        bail!("passkey assertion is for another credential");
    }
    let authenticator_data = decode(&assertion.authenticator_data, 37, 1024)?;
    let client_data_json = decode(&assertion.client_data_json, 2, 4096)?;
    let client: ClientData =
        serde_json::from_slice(&client_data_json).context("passkey client data is malformed")?;
    if client.kind != "webauthn.get"
        || client.challenge != URL_SAFE_NO_PAD.encode(challenge)
        || client.cross_origin
        || !credential.origins.contains(&client.origin)
    {
        bail!("passkey assertion is not bound to this exact action and origin");
    }
    if authenticator_data[..32] != Sha256::digest(credential.rp_id.as_bytes())[..] {
        bail!("passkey assertion is for another relying party");
    }
    let flags = authenticator_data[32];
    if flags & FLAG_USER_PRESENT == 0 || flags & FLAG_USER_VERIFIED == 0 {
        bail!("passkey assertion lacks user presence and verification");
    }
    let backup_eligible = flags & FLAG_BACKUP_ELIGIBLE != 0;
    let backed_up = flags & FLAG_BACKUP_STATE != 0;
    if backed_up && !backup_eligible {
        bail!("passkey assertion has inconsistent backup flags");
    }
    let sign_count = u32::from_be_bytes(authenticator_data[33..37].try_into()?);
    if (sign_count != 0 || credential.sign_count != 0) && sign_count <= credential.sign_count {
        bail!("passkey sign counter did not advance; possible cloned authenticator");
    }
    let mut signed = authenticator_data.clone();
    signed.extend_from_slice(&Sha256::digest(&client_data_json));
    verify_assurance_signature(&credential.public_key_x963, &signed, &assertion.signature)
        .context("passkey assertion signature is invalid")?;
    Ok(VerifiedAssertion {
        sign_count,
        backup_eligible,
        backed_up,
    })
}

/// The WebAuthn challenge binding a passkey to an exact action.
pub(super) fn action_challenge(action: &DeviceActionIntent) -> Result<[u8; 32]> {
    Ok(Sha256::digest(action.canonical_bytes()?).into())
}

fn validate_registration(registration: &PasskeyRegistration) -> Result<()> {
    let id = decode(&registration.credential_id, 16, 1023)?;
    let lifetime = registration.expires_at - registration.issued_at;
    if id.is_empty()
        || registration.nonce == [0; 32]
        || registration.rp_id.is_empty()
        || registration.rp_id.len() > 253
        || registration
            .rp_id
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        || registration.origins.is_empty()
        || registration.origins.len() > 4
        || registration.origins.iter().any(|origin| {
            origin.len() > 255
                || origin.chars().any(char::is_control)
                || url::Url::parse(origin).map_or(true, |url| {
                    url.scheme() != "https" || url.origin().ascii_serialization() != *origin
                })
        })
        || lifetime <= chrono::Duration::zero()
        || lifetime > chrono::Duration::seconds(300)
    {
        bail!("invalid passkey registration");
    }
    Ok(())
}

fn spki_to_x963(spki_b64: &str) -> Result<String> {
    let der = decode(spki_b64, 26, 512)?;
    let key = p256::PublicKey::from_public_key_der(&der)
        .context("passkey public key must be a P-256 (ES256) SPKI key")?;
    Ok(URL_SAFE_NO_PAD.encode(key.to_sec1_point(false).as_bytes()))
}

fn decode(value: &str, min: usize, max: usize) -> Result<Vec<u8>> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .context("value is not base64url")?;
    if bytes.len() < min || bytes.len() > max || URL_SAFE_NO_PAD.encode(&bytes) != value {
        bail!("value is not canonical or has the wrong length");
    }
    Ok(bytes)
}

fn render_registration_review(
    registration: &PasskeyRegistration,
    action: &DeviceActionIntent,
    owner: &InstalledPolicy,
) -> Result<String> {
    use std::fmt::Write;

    let mut review = String::from("Review the exact passkey registration:\n");
    for (name, value) in [
        ("version", action.version.to_string()),
        ("scope", action.scope.as_str().to_owned()),
        ("operation", action.operation.clone()),
        ("installation (audience)", action.target.clone()),
        ("owner-root public key", owner.owner_root_key.clone()),
        ("owner-root generation", owner.root_generation.clone()),
        ("credential id", registration.credential_id.clone()),
        (
            "credential public key",
            spki_to_x963(&registration.public_key_spki)?,
        ),
        ("relying party id", registration.rp_id.clone()),
        ("allowed origins", registration.origins.join(", ")),
        ("nonce", action.nonce.clone()),
        ("effect digest", action.effect_digest.clone()),
        (
            "issued at",
            action
                .issued_at
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        ),
        (
            "expires at",
            action
                .expires_at
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        ),
    ] {
        writeln!(review, "  {name:<28} {value}")?;
    }
    review.push_str(
        "  A passkey is a recovery factor, never a device. It can fill at most one approval of a\n  \
         threshold introduction that still needs a trusted device.\n",
    );
    Ok(review)
}

/// Owner CLI: register a passkey the client created, after reviewing every
/// field the owner-root signature covers.
pub fn run_register(
    registration_path: &Path,
    assertion_path: &Path,
    root_key: &Path,
    assume_yes: bool,
) -> Result<()> {
    use std::io::{BufRead, Write};

    let home = crate::coven_home_dir()?;
    let read = |path: &Path| -> Result<Vec<u8>> {
        let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
        if bytes.len() > super::MAX_MOBILE_REQUEST_BYTES {
            bail!("{} is too large", path.display());
        }
        Ok(bytes)
    };
    let registration: PasskeyRegistration = serde_json::from_slice(&read(registration_path)?)
        .context("passkey registration is malformed")?;
    let possession: PasskeyAssertion =
        serde_json::from_slice(&read(assertion_path)?).context("passkey assertion is malformed")?;
    let key = super::policy_cli::load_owner_root(&home, root_key)?;
    let store = PasskeyStore::open(&home, super::identity::installed_host_fingerprint(&home)?)?;
    let owner = store.owner_policy()?;
    if owner.owner_root_key != super::policy_cli::public_key(&key) {
        bail!("this key is not the installed owner-root credential");
    }
    let action = store.registration_action_for(&registration, &owner)?;
    print!(
        "{}",
        render_registration_review(&registration, &action, &owner)?
    );
    if !assume_yes {
        print!("Type `register` to sign this exact action with the owner-root key (anything else cancels): ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        if line.trim() != "register" {
            bail!("passkey registration cancelled; nothing was signed");
        }
    }
    let signature = super::policy_cli::sign_action(&key, &action)?;
    let credential = store.register(&registration, &possession, &signature, Utc::now())?;
    println!(
        "Registered passkey {} ({}).",
        credential.credential_id,
        if credential.backed_up {
            "synced"
        } else {
            "not synced"
        }
    );
    Ok(())
}

pub fn run_list(json: bool) -> Result<()> {
    let home = crate::coven_home_dir()?;
    let credentials =
        PasskeyStore::open(&home, super::identity::installed_host_fingerprint(&home)?)?.list()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&credentials)?);
        return Ok(());
    }
    if credentials.is_empty() {
        println!("No passkeys are registered.");
    }
    for credential in credentials {
        println!(
            "{}\t{}\t{}\tsign count {}",
            credential.credential_id,
            credential.rp_id,
            if credential.backed_up {
                "synced"
            } else {
                "not synced"
            },
            credential.sign_count
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/fixtures/passkey-recovery/mod.rs"]
mod tests;
