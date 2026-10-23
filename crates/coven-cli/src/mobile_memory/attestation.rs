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
const MAX_CHAIN: usize = 6;
const MAX_RECORDS: usize = 256;

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OfficialAndroidApp {
    pub package_name: String,
    /// SHA-256 of the app signing certificate, base64url.
    pub signature_digest: String,
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
        if self.android_roots.len() > 8 || self.official_android_apps.len() > 32 {
            bail!("attestation trust configuration is too large");
        }
        for root in &self.android_roots {
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
        Ok(())
    }
}

/// The attestation challenge for one exact transcript and device key.
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
    }
}

fn android_key(
    chain: &[String],
    trust: &AttestationTrust,
    public_key_x963: &str,
    challenge: &[u8; 32],
    now: DateTime<Utc>,
) -> Result<DeviceAttestation> {
    if chain.is_empty() || chain.len() > MAX_CHAIN || trust.android_roots.is_empty() {
        bail!("no verifiable android attestation chain");
    }
    let ders = chain
        .iter()
        .map(|cert| decode(cert))
        .collect::<Result<Vec<_>>>()?;
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
    // Each certificate is signed by the next; the last must be signed by, or
    // be, an owner-supplied anchor.
    for pair in certs.windows(2) {
        pair[0]
            .verify_signature(Some(pair[1].public_key()))
            .map_err(|_| anyhow::anyhow!("attestation chain signature is invalid"))?;
    }
    let last = certs.last().context("empty chain")?;
    let anchored = trust.android_roots.iter().any(|root| {
        let Ok(root_der) = decode(root) else {
            return false;
        };
        let Ok((_, anchor)) = X509Certificate::from_der(&root_der) else {
            return false;
        };
        root_der == *ders.last().unwrap_or(&Vec::new())
            || last.verify_signature(Some(anchor.public_key())).is_ok()
    });
    if !anchored {
        bail!("attestation chain does not end at an owner-supplied anchor");
    }
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
    Ok(())
}

pub fn run_trust_add_android_root(certificate: &Path) -> Result<()> {
    let home = crate::coven_home_dir()?;
    let bytes = fs::read(certificate)
        .with_context(|| format!("failed to read {}", certificate.display()))?;
    let der = match rustls_pki_types::CertificateDer::from_pem_slice(&bytes) {
        Ok(der) => der.to_vec(),
        Err(_) => bytes,
    };
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
