//! Optional platform attestation as an assurance attribute. It never creates
//! identity or authority: an absent, unverifiable or unconfigured attestation
//! is simply `unattested_device`, and self-built clients keep working under
//! owner policy. Trust anchors and official app identities are owner-supplied
//! configuration; no vendor root is compiled into Coven.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use rustls_pki_types::pem::PemObject;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use x509_parser::prelude::{FromDer, X509Certificate};

use super::config::{atomic_replace_private, ensure_private_mobile_dir, validate_private_file};

const TRUST_FILE: &str = "attestation-trust.json";
const ATTESTATIONS_FILE: &str = "attestations.json";
const ANDROID_KEY_DESCRIPTION_OID: &str = "1.3.6.1.4.1.11129.2.1.17";
const APPLE_NONCE_OID: &str = "1.2.840.113635.100.8.2";
const APPLE_ATTESTATION_FORMAT: &str = "apple-appattest";
/// Production App Attest AAGUID: `appattest` padded with zeros. Development
/// keys (`appattestdevelop`) are not official-app evidence.
const APPLE_AAGUID: [u8; 16] = *b"appattest\0\0\0\0\0\0\0";
const MAX_CHAIN: usize = 6;
const MAX_RECORDS: usize = 256;
/// An attestation object is a few kilobytes; anything larger is not decoded.
const MAX_ATTESTATION_OBJECT: usize = 32 * 1024;
/// `{ fmt, attStmt: { x5c: [...] }, authData }` is three containers deep.
const MAX_CBOR_DEPTH: usize = 4;

/// Ordered: each level implies the ones below it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceAttestation {
    #[default]
    UnattestedDevice,
    VerifiedHardwareKey,
    VerifiedOfficialApp,
}

impl DeviceAttestation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnattestedDevice => "unattested_device",
            Self::VerifiedHardwareKey => "verified_hardware_key",
            Self::VerifiedOfficialApp => "verified_official_app",
        }
    }

    pub(super) fn is_unattested(&self) -> bool {
        *self == Self::UnattestedDevice
    }
}

/// Evidence a client may attach. Unknown platforms are rejected at parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", tag = "platform")]
pub enum AttestationEvidence {
    /// Android Keystore key attestation: leaf-first DER certificate chain for
    /// the device's own possession key.
    AndroidKey { chain: Vec<String> },
    /// Apple App Attest: the base64url CBOR attestation object for the
    /// device's own possession key, made with the request challenge as the
    /// `clientDataHash`.
    AppleAppAttest { attestation_object: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OfficialAndroidApp {
    pub package_name: String,
    /// SHA-256 of the app signing certificate, base64url.
    pub signature_digest: String,
}

/// An App ID is the team ID and bundle ID joined by a dot; App Attest binds
/// its SHA-256 into the authenticator data's RP ID hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OfficialAppleApp {
    pub team_id: String,
    pub bundle_id: String,
}

impl OfficialAppleApp {
    fn app_id(&self) -> String {
        format!("{}.{}", self.team_id, self.bundle_id)
    }

    fn rp_id_hash(&self) -> [u8; 32] {
        Sha256::digest(self.app_id()).into()
    }
}

/// Owner-managed. Empty by default: nothing is verifiable, so every device is
/// `unattested_device` and nothing else changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AttestationTrust {
    /// DER root certificates, base64url, chosen by the owner.
    #[serde(default)]
    pub android_roots: Vec<String>,
    #[serde(default)]
    pub official_android_apps: Vec<OfficialAndroidApp>,
    /// DER App Attest anchors, base64url, chosen by the owner.
    #[serde(default)]
    pub apple_roots: Vec<String>,
    #[serde(default)]
    pub official_apple_apps: Vec<OfficialAppleApp>,
}

impl AttestationTrust {
    pub fn load(home: &Path) -> Result<Self> {
        let path = ensure_private_mobile_dir(home)?.join(TRUST_FILE);
        match fs::symlink_metadata(&path) {
            Ok(_) => validate_private_file(&path)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => {
                return Err(error).with_context(|| format!("failed to inspect {}", path.display()))
            }
        }
        let trust: Self = serde_json::from_slice(&fs::read(&path)?)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        trust.validate()?;
        Ok(trust)
    }

    pub fn save(&self, home: &Path) -> Result<()> {
        self.validate()?;
        let mut encoded = serde_json::to_vec_pretty(self)?;
        encoded.push(b'\n');
        atomic_replace_private(&ensure_private_mobile_dir(home)?.join(TRUST_FILE), &encoded)
    }

    fn validate(&self) -> Result<()> {
        if self.android_roots.len() > 8
            || self.official_android_apps.len() > 32
            || self.apple_roots.len() > 8
            || self.official_apple_apps.len() > 32
        {
            bail!("attestation trust configuration is too large");
        }
        for root in self.android_roots.iter().chain(&self.apple_roots) {
            let der = decode(root)?;
            let (rest, _) = X509Certificate::from_der(&der)
                .map_err(|_| anyhow::anyhow!("attestation anchor is not a DER certificate"))?;
            if !rest.is_empty() {
                bail!("attestation anchor has trailing data");
            }
        }
        for app in &self.official_android_apps {
            if app.package_name.is_empty()
                || app.package_name.len() > 255
                || decode(&app.signature_digest)?.len() != 32
            {
                bail!("invalid official app identity");
            }
        }
        for app in &self.official_apple_apps {
            if !is_apple_team_id(&app.team_id) || !is_bundle_id(&app.bundle_id) {
                bail!("invalid official app identity");
            }
        }
        Ok(())
    }
}

/// Apple team IDs are ten ASCII letters or digits.
fn is_apple_team_id(value: &str) -> bool {
    value.len() == 10 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// Bundle IDs are dot-separated ASCII labels of letters, digits and hyphens.
fn is_bundle_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// The attestation challenge for one exact transcript and device key. Apple
/// clients pass it as the `clientDataHash`.
pub(super) fn attestation_challenge(transcript_bytes: &[u8]) -> [u8; 32] {
    let mut material = b"COVEN-ATTESTATION/1\0".to_vec();
    material.extend_from_slice(transcript_bytes);
    Sha256::digest(material).into()
}

/// Evaluates evidence for `public_key_x963`. Anything short of a full, fresh,
/// anchored, challenge-bound chain is `UnattestedDevice`, never an error:
/// attestation can only raise assurance, never block an otherwise valid client.
/// The caller applies any owner-policy minimum.
pub(super) fn evaluate(
    evidence: Option<&AttestationEvidence>,
    trust: &AttestationTrust,
    public_key_x963: &str,
    challenge: &[u8; 32],
    now: DateTime<Utc>,
) -> DeviceAttestation {
    match evidence {
        None => DeviceAttestation::UnattestedDevice,
        Some(AttestationEvidence::AndroidKey { chain }) => {
            android_key(chain, trust, public_key_x963, challenge, now)
                .unwrap_or(DeviceAttestation::UnattestedDevice)
        }
        Some(AttestationEvidence::AppleAppAttest { attestation_object }) => {
            apple_app_attest(attestation_object, trust, public_key_x963, challenge, now)
                .unwrap_or(DeviceAttestation::UnattestedDevice)
        }
    }
}

/// Decodes a leaf-first DER chain and checks that each certificate is valid
/// now, signed by the next, and that the last is, or is signed by, one of
/// the owner-supplied `anchors`.
fn verify_chain<'a>(
    ders: &'a [Vec<u8>],
    anchors: &[String],
    now: DateTime<Utc>,
) -> Result<Vec<X509Certificate<'a>>> {
    if ders.is_empty() || ders.len() > MAX_CHAIN || anchors.is_empty() {
        bail!("no verifiable attestation chain");
    }
    let certs = ders
        .iter()
        .map(|der| match X509Certificate::from_der(der) {
            Ok(([], cert)) => Ok(cert),
            _ => bail!("invalid attestation certificate"),
        })
        .collect::<Result<Vec<_>>>()?;
    let at = x509_parser::time::ASN1Time::from_timestamp(now.timestamp())
        .map_err(|_| anyhow::anyhow!("invalid time"))?;
    for cert in &certs {
        if !cert.validity().is_valid_at(at) {
            bail!("attestation certificate is not valid now");
        }
    }
    for pair in certs.windows(2) {
        pair[0]
            .verify_signature(Some(pair[1].public_key()))
            .map_err(|_| anyhow::anyhow!("attestation chain signature is invalid"))?;
    }
    let last = certs.last().context("empty chain")?;
    let last_der = ders.last().context("empty chain")?;
    let anchored = anchors.iter().any(|root| {
        let Ok(root_der) = decode(root) else {
            return false;
        };
        let Ok((_, anchor)) = X509Certificate::from_der(&root_der) else {
            return false;
        };
        root_der == *last_der || last.verify_signature(Some(anchor.public_key())).is_ok()
    });
    if !anchored {
        bail!("attestation chain does not end at an owner-supplied anchor");
    }
    Ok(certs)
}

fn android_key(
    chain: &[String],
    trust: &AttestationTrust,
    public_key_x963: &str,
    challenge: &[u8; 32],
    now: DateTime<Utc>,
) -> Result<DeviceAttestation> {
    if chain.len() > MAX_CHAIN {
        bail!("attestation chain is too long");
    }
    let ders = chain
        .iter()
        .map(|cert| decode(cert))
        .collect::<Result<Vec<_>>>()?;
    let certs = verify_chain(&ders, &trust.android_roots, now)?;
    let leaf = &certs[0];
    if URL_SAFE_NO_PAD.encode(leaf.public_key().subject_public_key.data.as_ref()) != public_key_x963
    {
        bail!("attestation is for another key");
    }
    let extension = leaf
        .extensions()
        .iter()
        .find(|ext| ext.oid.to_id_string() == ANDROID_KEY_DESCRIPTION_OID)
        .context("attestation lacks a key description")?;
    let description = KeyDescription::parse(extension.value)?;
    if description.challenge != challenge {
        bail!("attestation is not bound to this request");
    }
    // 1 = TrustedEnvironment, 2 = StrongBox. 0 = Software proves nothing.
    if !matches!(description.security_level, 1 | 2) {
        bail!("attestation key is not hardware-backed");
    }
    let official = description.application.is_some_and(|app| {
        trust.official_android_apps.iter().any(|official| {
            app.packages
                .iter()
                .any(|name| name.as_bytes() == official.package_name.as_bytes())
                && app
                    .signature_digests
                    .iter()
                    .any(|digest| URL_SAFE_NO_PAD.encode(digest) == official.signature_digest)
        })
    });
    Ok(if official {
        DeviceAttestation::VerifiedOfficialApp
    } else {
        DeviceAttestation::VerifiedHardwareKey
    })
}

/// Apple App Attest. The object is `{ fmt: "apple-appattest", attStmt: { x5c,
/// receipt }, authData }`. Every condition below must hold, else
/// `UnattestedDevice`: x5c chains to an owner-supplied Apple anchor, the leaf
/// certifies exactly the device key, its nonce extension equals
/// SHA-256(authData ‖ challenge), and authData names an owner-named app ID with
/// a fresh production key whose credential ID is SHA-256 of that key. The
/// receipt is Apple-server material and is ignored; no network call is made.
fn apple_app_attest(
    attestation_object: &str,
    trust: &AttestationTrust,
    public_key_x963: &str,
    challenge: &[u8; 32],
    now: DateTime<Utc>,
) -> Result<DeviceAttestation> {
    if trust.apple_roots.is_empty() || trust.official_apple_apps.is_empty() {
        bail!("no verifiable apple attestation");
    }
    if attestation_object.len() > MAX_ATTESTATION_OBJECT.div_ceil(3) * 4 {
        bail!("attestation object is too large");
    }
    let object = decode(attestation_object)?;
    if object.len() > MAX_ATTESTATION_OBJECT {
        bail!("attestation object is too large");
    }
    let object = AppleAttestationObject::parse(&object)?;
    let certs = verify_chain(&object.x5c, &trust.apple_roots, now)?;
    let leaf = &certs[0];
    let key = leaf.public_key().subject_public_key.data.as_ref();
    if URL_SAFE_NO_PAD.encode(key) != public_key_x963 {
        bail!("attestation is for another key");
    }
    let extension = leaf
        .extensions()
        .iter()
        .find(|ext| ext.oid.to_id_string() == APPLE_NONCE_OID)
        .context("attestation lacks a nonce")?;
    let expected: [u8; 32] = Sha256::new()
        .chain_update(&object.auth_data)
        .chain_update(challenge)
        .finalize()
        .into();
    if apple_nonce(extension.value)? != expected {
        bail!("attestation is not bound to this request");
    }
    let auth_data = AuthenticatorData::parse(&object.auth_data)?;
    if auth_data.credential_id != Sha256::digest(key).as_slice() {
        bail!("attestation credential is not the device key");
    }
    if auth_data.aaguid != APPLE_AAGUID || auth_data.counter != 0 {
        bail!("attestation is not a fresh production App Attest key");
    }
    if !trust
        .official_apple_apps
        .iter()
        .any(|app| app.rp_id_hash() == auth_data.rp_id_hash)
    {
        bail!("attestation is for an app the owner has not named");
    }
    Ok(DeviceAttestation::VerifiedOfficialApp)
}

struct AppleAttestationObject {
    /// Leaf-first DER chain.
    x5c: Vec<Vec<u8>>,
    auth_data: Vec<u8>,
}

impl AppleAttestationObject {
    /// Strict: a text-keyed map with exactly the members above, no duplicate
    /// members, no trailing bytes, and bounded nesting.
    fn parse(bytes: &[u8]) -> Result<Self> {
        let mut reader = bytes;
        let value: ciborium::Value =
            ciborium::de::from_reader_with_recursion_limit(&mut reader, MAX_CBOR_DEPTH)
                .map_err(|_| anyhow::anyhow!("attestation object is not CBOR"))?;
        if !reader.is_empty() {
            bail!("attestation object has trailing data");
        }
        let mut fmt = None;
        let mut statement = None;
        let mut auth_data = None;
        for (key, member) in cbor_map(value)? {
            let slot = match key.as_str() {
                "fmt" => &mut fmt,
                "attStmt" => &mut statement,
                "authData" => &mut auth_data,
                _ => bail!("unexpected attestation object member"),
            };
            if slot.replace(member).is_some() {
                bail!("duplicate attestation object member");
            }
        }
        match fmt {
            Some(ciborium::Value::Text(fmt)) if fmt == APPLE_ATTESTATION_FORMAT => {}
            _ => bail!("attestation object is not apple-appattest"),
        }
        let mut x5c = None;
        for (key, member) in cbor_map(statement.context("attestation object lacks attStmt")?)? {
            match key.as_str() {
                "x5c" if x5c.is_none() => x5c = Some(member),
                "receipt" => {}
                _ => bail!("unexpected attestation statement member"),
            }
        }
        let Some(ciborium::Value::Array(chain)) = x5c else {
            bail!("attestation statement lacks x5c");
        };
        let x5c = chain
            .into_iter()
            .map(|cert| match cert {
                ciborium::Value::Bytes(der) => Ok(der),
                _ => bail!("x5c entry is not a byte string"),
            })
            .collect::<Result<Vec<_>>>()?;
        let Some(ciborium::Value::Bytes(auth_data)) = auth_data else {
            bail!("attestation object lacks authData");
        };
        Ok(Self { x5c, auth_data })
    }
}

fn cbor_map(value: ciborium::Value) -> Result<Vec<(String, ciborium::Value)>> {
    let ciborium::Value::Map(entries) = value else {
        bail!("expected a CBOR map");
    };
    entries
        .into_iter()
        .map(|(key, member)| match key {
            ciborium::Value::Text(key) => Ok((key, member)),
            _ => bail!("expected a text CBOR key"),
        })
        .collect()
}

/// The nonce extension is `SEQUENCE { [1] EXPLICIT OCTET STRING nonce }`.
fn apple_nonce(der: &[u8]) -> Result<[u8; 32]> {
    let mut outer = Der::new(der);
    let mut sequence = Der::new(outer.expect(0x30)?);
    outer.end()?;
    let mut tagged = Der::new(sequence.expect(0xa1)?);
    sequence.end()?;
    let nonce = tagged.expect(0x04)?;
    tagged.end()?;
    nonce
        .try_into()
        .map_err(|_| anyhow::anyhow!("nonce is not 32 bytes"))
}

/// WebAuthn authenticator data with attested credential data:
/// rpIdHash(32) ‖ flags(1) ‖ counter(4) ‖ aaguid(16) ‖ credentialIdLength(2)
/// ‖ credentialId. Anything after the credential ID is covered by the nonce
/// and otherwise ignored.
struct AuthenticatorData<'a> {
    rp_id_hash: [u8; 32],
    counter: u32,
    aaguid: [u8; 16],
    credential_id: &'a [u8],
}

impl<'a> AuthenticatorData<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self> {
        let (rp_id_hash, rest) = bytes.split_at_checked(32).context("short authData")?;
        let (flags, rest) = rest.split_first().context("short authData")?;
        // Bit 6: attested credential data included.
        if flags & 0x40 == 0 {
            bail!("authData carries no attested credential");
        }
        let (counter, rest) = rest.split_at_checked(4).context("short authData")?;
        let (aaguid, rest) = rest.split_at_checked(16).context("short authData")?;
        let (length, rest) = rest.split_at_checked(2).context("short authData")?;
        let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
        let credential_id = rest.get(..length).context("short authData")?;
        Ok(Self {
            rp_id_hash: rp_id_hash.try_into()?,
            counter: u32::from_be_bytes(counter.try_into()?),
            aaguid: aaguid.try_into()?,
            credential_id,
        })
    }
}

struct AttestationApplication {
    packages: Vec<String>,
    signature_digests: Vec<Vec<u8>>,
}

struct KeyDescription {
    security_level: u64,
    challenge: Vec<u8>,
    application: Option<AttestationApplication>,
}

impl KeyDescription {
    /// KeyDescription ::= SEQUENCE { attestationVersion INTEGER,
    ///   attestationSecurityLevel ENUMERATED, keyMintVersion INTEGER,
    ///   keyMintSecurityLevel ENUMERATED, attestationChallenge OCTET STRING,
    ///   uniqueId OCTET STRING, softwareEnforced AuthorizationList,
    ///   hardwareEnforced AuthorizationList }
    fn parse(der: &[u8]) -> Result<Self> {
        let mut outer = Der::new(der);
        let mut fields = Der::new(outer.expect(0x30)?);
        outer.end()?;
        fields.expect(0x02)?;
        let security_level = uint(fields.expect(0x0a)?)?;
        fields.expect(0x02)?;
        fields.expect(0x0a)?;
        let challenge = fields.expect(0x04)?.to_vec();
        fields.expect(0x04)?;
        let software = fields.expect(0x30)?;
        fields.expect(0x30)?;
        Ok(Self {
            security_level,
            challenge,
            application: attestation_application(software)?,
        })
    }
}

/// attestationApplicationId is `[709] EXPLICIT OCTET STRING` holding
/// SEQUENCE { SET OF SEQUENCE { packageName OCTET STRING, version INTEGER },
///            SET OF OCTET STRING (signature digests) }.
fn attestation_application(authorizations: &[u8]) -> Result<Option<AttestationApplication>> {
    let mut list = Der::new(authorizations);
    while !list.is_empty() {
        let (tag, value) = list.any()?;
        if tag != CONTEXT_709 {
            continue;
        }
        let mut wrapper = Der::new(value);
        let mut application = Der::new(wrapper.expect(0x04)?);
        let mut body = Der::new(application.expect(0x30)?);
        let mut packages_set = Der::new(body.expect(0x31)?);
        let mut packages = Vec::new();
        while !packages_set.is_empty() {
            let mut info = Der::new(packages_set.expect(0x30)?);
            packages.push(String::from_utf8(info.expect(0x04)?.to_vec())?);
        }
        let mut digests_set = Der::new(body.expect(0x31)?);
        let mut signature_digests = Vec::new();
        while !digests_set.is_empty() {
            signature_digests.push(digests_set.expect(0x04)?.to_vec());
        }
        return Ok(Some(AttestationApplication {
            packages,
            signature_digests,
        }));
    }
    Ok(None)
}

/// Context-specific, constructed, tag number 709 (high-tag-number form).
const CONTEXT_709: u32 = 0xbf_85_45;

/// Minimal strict DER reader for the fields above: definite lengths only.
struct Der<'a> {
    bytes: &'a [u8],
}

impl<'a> Der<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    fn end(&self) -> Result<()> {
        if !self.bytes.is_empty() {
            bail!("trailing DER data");
        }
        Ok(())
    }

    fn expect(&mut self, tag: u32) -> Result<&'a [u8]> {
        let (found, value) = self.any()?;
        if found != tag {
            bail!("unexpected DER tag");
        }
        Ok(value)
    }

    fn any(&mut self) -> Result<(u32, &'a [u8])> {
        let bytes = self.bytes;
        let first = *bytes.first().context("truncated DER")?;
        let mut index = 1;
        let mut tag = u32::from(first);
        if first & 0x1f == 0x1f {
            loop {
                let next = *bytes.get(index).context("truncated DER tag")?;
                index += 1;
                tag = tag.checked_mul(256).context("DER tag too long")? | u32::from(next);
                if next & 0x80 == 0 {
                    break;
                }
                if index > 4 {
                    bail!("DER tag too long");
                }
            }
        }
        let length_byte = *bytes.get(index).context("truncated DER length")?;
        index += 1;
        let length = if length_byte & 0x80 == 0 {
            usize::from(length_byte)
        } else {
            let count = usize::from(length_byte & 0x7f);
            if count == 0 || count > 3 {
                bail!("unsupported DER length");
            }
            let mut length = 0usize;
            for _ in 0..count {
                length = length * 256 + usize::from(*bytes.get(index).context("truncated DER")?);
                index += 1;
            }
            length
        };
        let end = index.checked_add(length).context("DER overflow")?;
        let value = bytes.get(index..end).context("truncated DER value")?;
        self.bytes = &bytes[end..];
        Ok((tag, value))
    }
}

fn uint(bytes: &[u8]) -> Result<u64> {
    if bytes.is_empty() || bytes.len() > 8 {
        bail!("invalid DER integer");
    }
    Ok(bytes
        .iter()
        .fold(0, |value, byte| value << 8 | u64::from(*byte)))
}

fn decode(value: &str) -> Result<Vec<u8>> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .context("value is not base64url")?;
    if URL_SAFE_NO_PAD.encode(&bytes) != value {
        bail!("value is not canonical base64url");
    }
    Ok(bytes)
}

/// The attribute recorded for an enrolled device key. Display and policy
/// input only; it is not part of the device's identity or grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AttestationRecord {
    pub device_id: Uuid,
    pub public_key_x963: String,
    pub attestation: DeviceAttestation,
    pub evaluated_at: DateTime<Utc>,
}

pub struct AttestationRecords {
    path: PathBuf,
}

impl AttestationRecords {
    pub fn open(home: &Path) -> Result<Self> {
        Ok(Self {
            path: ensure_private_mobile_dir(home)?.join(ATTESTATIONS_FILE),
        })
    }

    /// Absent means `unattested_device`; a record for another key is ignored.
    pub fn attestation(&self, device_id: Uuid, public_key_x963: &str) -> Result<DeviceAttestation> {
        Ok(self
            .read()?
            .into_iter()
            .find(|record| {
                record.device_id == device_id && record.public_key_x963 == public_key_x963
            })
            .map_or(DeviceAttestation::UnattestedDevice, |record| {
                record.attestation
            }))
    }

    pub(super) fn record(&self, record: AttestationRecord) -> Result<()> {
        if record.attestation.is_unattested() {
            return Ok(());
        }
        let mut records = self.read()?;
        records.retain(|existing| existing.device_id != record.device_id);
        if records.len() >= MAX_RECORDS {
            bail!("attestation record store is full");
        }
        records.push(record);
        let mut encoded = serde_json::to_vec_pretty(&records)?;
        encoded.push(b'\n');
        atomic_replace_private(&self.path, &encoded)
    }

    fn read(&self) -> Result<Vec<AttestationRecord>> {
        match fs::symlink_metadata(&self.path) {
            Ok(_) => validate_private_file(&self.path)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect {}", self.path.display()))
            }
        }
        serde_json::from_slice(&fs::read(&self.path)?)
            .with_context(|| format!("failed to parse {}", self.path.display()))
    }
}

pub fn run_trust_show(json: bool) -> Result<()> {
    let trust = AttestationTrust::load(&crate::coven_home_dir()?)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&trust)?);
        return Ok(());
    }
    if trust == AttestationTrust::default() {
        println!("No attestation anchors are configured; every device is unattested_device.");
        return Ok(());
    }
    for root in &trust.android_roots {
        let digest = Sha256::digest(decode(root)?);
        println!(
            "android root  sha256:{}",
            URL_SAFE_NO_PAD.encode(&digest[..12])
        );
    }
    for app in &trust.official_android_apps {
        println!(
            "official app  {} {}",
            app.package_name, app.signature_digest
        );
    }
    for root in &trust.apple_roots {
        let digest = Sha256::digest(decode(root)?);
        println!(
            "apple root    sha256:{}",
            URL_SAFE_NO_PAD.encode(&digest[..12])
        );
    }
    for app in &trust.official_apple_apps {
        println!("official app  {}", app.app_id());
    }
    Ok(())
}

/// Reads an owner-supplied anchor certificate, PEM or DER.
fn read_anchor(certificate: &Path) -> Result<Vec<u8>> {
    let bytes = fs::read(certificate)
        .with_context(|| format!("failed to read {}", certificate.display()))?;
    Ok(
        match rustls_pki_types::CertificateDer::from_pem_slice(&bytes) {
            Ok(der) => der.to_vec(),
            Err(_) => bytes,
        },
    )
}

pub fn run_trust_add_android_root(certificate: &Path) -> Result<()> {
    let home = crate::coven_home_dir()?;
    let der = read_anchor(certificate)?;
    let mut trust = AttestationTrust::load(&home)?;
    let encoded = URL_SAFE_NO_PAD.encode(&der);
    if !trust.android_roots.contains(&encoded) {
        trust.android_roots.push(encoded);
    }
    trust.save(&home)?;
    println!(
        "Added Android attestation anchor sha256:{}. It can only raise a device's assurance \
         attribute; it never grants authority.",
        URL_SAFE_NO_PAD.encode(&Sha256::digest(&der)[..12])
    );
    Ok(())
}

pub fn run_trust_add_apple_root(certificate: &Path) -> Result<()> {
    let home = crate::coven_home_dir()?;
    let der = read_anchor(certificate)?;
    let mut trust = AttestationTrust::load(&home)?;
    let encoded = URL_SAFE_NO_PAD.encode(&der);
    if !trust.apple_roots.contains(&encoded) {
        trust.apple_roots.push(encoded);
    }
    trust.save(&home)?;
    println!(
        "Added Apple App Attest anchor sha256:{}. It can only raise a device's assurance \
         attribute; it never grants authority.",
        URL_SAFE_NO_PAD.encode(&Sha256::digest(&der)[..12])
    );
    Ok(())
}

pub fn run_trust_add_official_apple_app(team_id: &str, bundle_id: &str) -> Result<()> {
    let home = crate::coven_home_dir()?;
    let mut trust = AttestationTrust::load(&home)?;
    let app = OfficialAppleApp {
        team_id: team_id.to_owned(),
        bundle_id: bundle_id.to_owned(),
    };
    let app_id = app.app_id();
    if !trust.official_apple_apps.contains(&app) {
        trust.official_apple_apps.push(app);
    }
    trust.save(&home)?;
    println!("Added official Apple app {app_id}.");
    Ok(())
}

pub fn run_trust_add_official_android_app(
    package_name: &str,
    signature_digest: &str,
) -> Result<()> {
    let home = crate::coven_home_dir()?;
    let mut trust = AttestationTrust::load(&home)?;
    let app = OfficialAndroidApp {
        package_name: package_name.to_owned(),
        signature_digest: signature_digest.to_owned(),
    };
    if !trust.official_android_apps.contains(&app) {
        trust.official_android_apps.push(app);
    }
    trust.save(&home)?;
    println!("Added official Android app {package_name}.");
    Ok(())
}

pub fn run_trust_clear() -> Result<()> {
    let home = crate::coven_home_dir()?;
    AttestationTrust::default().save(&home)?;
    println!("Cleared attestation anchors; every device is now unattested_device.");
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/fixtures/device-attestation/mod.rs"]
pub(super) mod tests;
