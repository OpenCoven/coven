//! Synthetic Android key-attestation chains built with rcgen; no vendor root,
//! device, Google service or network.
use super::*;
use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, CustomExtension, IsCa, KeyPair};

const KEY_DESCRIPTION_OID: &[u64] = &[1, 3, 6, 1, 4, 1, 11129, 2, 1, 17];
const PACKAGE: &str = "org.opencoven.pocket";

fn tlv(tag: &[u8], content: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    match content.len() {
        n if n < 0x80 => out.push(n as u8),
        n if n < 0x100 => out.extend([0x81, n as u8]),
        n => out.extend([0x82, (n >> 8) as u8, n as u8]),
    }
    out.extend_from_slice(content);
    out
}

pub(in crate::mobile_memory) fn key_description(
    level: u8,
    challenge: &[u8],
    app: Option<(&str, [u8; 32])>,
) -> Vec<u8> {
    let mut software = Vec::new();
    if let Some((package, digest)) = app {
        let info = tlv(
            &[0x30],
            &[tlv(&[0x04], package.as_bytes()), tlv(&[0x02], &[1])].concat(),
        );
        let body = tlv(
            &[0x30],
            &[tlv(&[0x31], &info), tlv(&[0x31], &tlv(&[0x04], &digest))].concat(),
        );
        software = tlv(&[0xbf, 0x85, 0x45], &tlv(&[0x04], &body));
    }
    tlv(
        &[0x30],
        &[
            tlv(&[0x02], &[4]),
            tlv(&[0x0a], &[level]),
            tlv(&[0x02], &[4]),
            tlv(&[0x0a], &[level]),
            tlv(&[0x04], challenge),
            tlv(&[0x04], &[]),
            tlv(&[0x30], &software),
            tlv(&[0x30], &[]),
        ]
        .concat(),
    )
}

pub(in crate::mobile_memory) struct Ca {
    issuer: CertifiedIssuer<'static, KeyPair>,
}

impl Ca {
    pub(in crate::mobile_memory) fn new() -> Self {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        Self {
            issuer: CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap(),
        }
    }

    pub(in crate::mobile_memory) fn root_b64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.issuer.der())
    }

    /// A leaf certifying `device` with the given key description.
    pub(in crate::mobile_memory) fn leaf(
        &self,
        device: &SigningKey,
        description: Vec<u8>,
    ) -> String {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params
            .custom_extensions
            .push(CustomExtension::from_oid_content(
                KEY_DESCRIPTION_OID,
                description,
            ));
        let pkcs8 = device.to_pkcs8_der().unwrap();
        let subject = KeyPair::try_from(pkcs8.as_bytes()).unwrap();
        URL_SAFE_NO_PAD.encode(params.signed_by(&subject, &self.issuer).unwrap().der())
    }
}

struct Fixture {
    now: DateTime<Utc>,
    device: SigningKey,
    challenge: [u8; 32],
    ca: Ca,
    digest: [u8; 32],
}

impl Fixture {
    fn new() -> Self {
        Self {
            now: DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            device: SigningKey::from_slice(&[3; 32]).unwrap(),
            challenge: attestation_challenge(b"exact transcript bytes"),
            ca: Ca::new(),
            digest: [44; 32],
        }
    }

    fn public(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.device.verifying_key().to_sec1_point(false).as_bytes())
    }

    fn trust(&self, official: bool) -> AttestationTrust {
        AttestationTrust {
            android_roots: vec![self.ca.root_b64()],
            official_android_apps: if official {
                vec![OfficialAndroidApp {
                    package_name: PACKAGE.into(),
                    signature_digest: URL_SAFE_NO_PAD.encode(self.digest),
                }]
            } else {
                Vec::new()
            },
        }
    }

    fn evidence(&self, level: u8, challenge: &[u8], app: bool) -> AttestationEvidence {
        let app = app.then_some((PACKAGE, self.digest));
        AttestationEvidence::AndroidKey {
            chain: vec![
                self.ca
                    .leaf(&self.device, key_description(level, challenge, app)),
                self.ca.root_b64(),
            ],
        }
    }

    fn evaluate(
        &self,
        evidence: Option<&AttestationEvidence>,
        trust: &AttestationTrust,
    ) -> DeviceAttestation {
        evaluate(evidence, trust, &self.public(), &self.challenge, self.now)
    }
}

#[test]
fn unattested_is_the_default_and_never_an_error() {
    let fixture = Fixture::new();
    let evidence = fixture.evidence(1, &fixture.challenge, true);
    assert_eq!(
        fixture.evaluate(None, &fixture.trust(true)),
        DeviceAttestation::UnattestedDevice
    );
    // With no owner-supplied anchors nothing is verifiable, so even a valid
    // chain leaves self-built and self-hosted setups exactly as before.
    assert_eq!(
        fixture.evaluate(Some(&evidence), &AttestationTrust::default()),
        DeviceAttestation::UnattestedDevice
    );
}

#[test]
fn hardware_backed_chain_to_an_owner_anchor_raises_assurance() {
    let fixture = Fixture::new();
    for level in [1, 2] {
        let evidence = fixture.evidence(level, &fixture.challenge, false);
        assert_eq!(
            fixture.evaluate(Some(&evidence), &fixture.trust(true)),
            DeviceAttestation::VerifiedHardwareKey
        );
    }
    let official = fixture.evidence(1, &fixture.challenge, true);
    assert_eq!(
        fixture.evaluate(Some(&official), &fixture.trust(true)),
        DeviceAttestation::VerifiedOfficialApp
    );
    // The official-app level needs the owner to name the app.
    assert_eq!(
        fixture.evaluate(Some(&official), &fixture.trust(false)),
        DeviceAttestation::VerifiedHardwareKey
    );
}

#[test]
fn weak_foreign_or_replayed_evidence_stays_unattested() {
    let fixture = Fixture::new();
    let trust = fixture.trust(true);
    let unattested = DeviceAttestation::UnattestedDevice;
    // Software-only keys prove nothing.
    assert_eq!(
        fixture.evaluate(Some(&fixture.evidence(0, &fixture.challenge, true)), &trust),
        unattested
    );
    // Evidence made for another request cannot be replayed onto this one.
    assert_eq!(
        fixture.evaluate(Some(&fixture.evidence(1, &[9; 32], true)), &trust),
        unattested
    );
    // A chain for another key.
    let other = Fixture {
        device: SigningKey::from_slice(&[4; 32]).unwrap(),
        ..Fixture::new()
    };
    let foreign_key = AttestationEvidence::AndroidKey {
        chain: vec![
            fixture
                .ca
                .leaf(&other.device, key_description(1, &fixture.challenge, None)),
            fixture.ca.root_b64(),
        ],
    };
    assert_eq!(fixture.evaluate(Some(&foreign_key), &trust), unattested);
    // A relay/account service's own CA is not an owner anchor.
    let attacker = Ca::new();
    let rogue = AttestationEvidence::AndroidKey {
        chain: vec![
            attacker.leaf(
                &fixture.device,
                key_description(2, &fixture.challenge, Some((PACKAGE, fixture.digest))),
            ),
            attacker.root_b64(),
        ],
    };
    assert_eq!(fixture.evaluate(Some(&rogue), &trust), unattested);
    // A tampered leaf fails its chain signature.
    let AttestationEvidence::AndroidKey { chain } = fixture.evidence(1, &fixture.challenge, true);
    let mut leaf = URL_SAFE_NO_PAD.decode(&chain[0]).unwrap();
    let last = leaf.len() - 1;
    leaf[last] ^= 1;
    let tampered = AttestationEvidence::AndroidKey {
        chain: vec![URL_SAFE_NO_PAD.encode(leaf), chain[1].clone()],
    };
    assert_eq!(fixture.evaluate(Some(&tampered), &trust), unattested);
    // Unknown platforms are rejected at parse time.
    assert!(
        serde_json::from_str::<AttestationEvidence>(r#"{"platform":"vendorX","chain":[]}"#)
            .is_err()
    );
}

#[test]
fn records_hold_only_raised_attributes_for_the_exact_key() {
    let home = tempfile::tempdir().unwrap();
    let records = AttestationRecords::open(home.path()).unwrap();
    let fixture = Fixture::new();
    let id = Uuid::from_u128(2);
    records
        .record(AttestationRecord {
            device_id: id,
            public_key_x963: fixture.public(),
            attestation: DeviceAttestation::UnattestedDevice,
            evaluated_at: fixture.now,
        })
        .unwrap();
    assert!(!home.path().join("mobile").join(ATTESTATIONS_FILE).exists());
    records
        .record(AttestationRecord {
            device_id: id,
            public_key_x963: fixture.public(),
            attestation: DeviceAttestation::VerifiedHardwareKey,
            evaluated_at: fixture.now,
        })
        .unwrap();
    assert_eq!(
        records.attestation(id, &fixture.public()).unwrap(),
        DeviceAttestation::VerifiedHardwareKey
    );
    // A rotated device key does not inherit the old key's attribute.
    assert_eq!(
        records.attestation(id, "another-key").unwrap(),
        DeviceAttestation::UnattestedDevice
    );
}

#[test]
fn owner_trust_configuration_is_validated() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        AttestationTrust::load(home.path()).unwrap(),
        AttestationTrust::default()
    );
    let fixture = Fixture::new();
    fixture.trust(true).save(home.path()).unwrap();
    assert_eq!(
        AttestationTrust::load(home.path()).unwrap(),
        fixture.trust(true)
    );
    let bad = AttestationTrust {
        android_roots: vec![URL_SAFE_NO_PAD.encode(b"not a certificate")],
        official_android_apps: Vec::new(),
    };
    assert!(bad.save(home.path()).is_err());
}
