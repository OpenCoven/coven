//! Synthetic Android key-attestation chains and Apple App Attest objects built
//! with rcgen; no vendor root, device, Apple or Google service, or network.
use super::*;
use ciborium::value::Value;
use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, CustomExtension, IsCa, KeyPair};

const KEY_DESCRIPTION_OID: &[u64] = &[1, 3, 6, 1, 4, 1, 11129, 2, 1, 17];
const APPLE_NONCE_OID: &[u64] = &[1, 2, 840, 113635, 100, 8, 2];
const PACKAGE: &str = "org.opencoven.pocket";
const TEAM_ID: &str = "ABCDE12345";
const BUNDLE_ID: &str = "org.opencoven.pocket";

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

    /// A leaf certifying `device` with the given Android key description.
    pub(in crate::mobile_memory) fn leaf(
        &self,
        device: &SigningKey,
        description: Vec<u8>,
    ) -> String {
        self.leaf_with(KEY_DESCRIPTION_OID, device, description)
    }

    /// A leaf certifying `device` carrying one custom extension.
    pub(in crate::mobile_memory) fn leaf_with(
        &self,
        oid: &[u64],
        device: &SigningKey,
        content: Vec<u8>,
    ) -> String {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params
            .custom_extensions
            .push(CustomExtension::from_oid_content(oid, content));
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
            ..AttestationTrust::default()
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
    let AttestationEvidence::AndroidKey { chain } = fixture.evidence(1, &fixture.challenge, true)
    else {
        unreachable!()
    };
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
        ..AttestationTrust::default()
    };
    assert!(bad.save(home.path()).is_err());
    let bad_apple_root = AttestationTrust {
        apple_roots: vec![URL_SAFE_NO_PAD.encode(b"not a certificate")],
        ..AttestationTrust::default()
    };
    assert!(bad_apple_root.save(home.path()).is_err());
    let bad_app = AttestationTrust {
        official_apple_apps: vec![OfficialAppleApp {
            team_id: "not a team".into(),
            bundle_id: BUNDLE_ID.into(),
        }],
        ..AttestationTrust::default()
    };
    assert!(bad_app.save(home.path()).is_err());
    // A trust file written before Apple anchors existed still loads.
    let legacy = serde_json::json!({
        "androidRoots": [fixture.ca.root_b64()],
        "officialAndroidApps": []
    });
    std::fs::write(
        home.path().join("mobile").join(TRUST_FILE),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    assert_eq!(
        AttestationTrust::load(home.path()).unwrap(),
        AttestationTrust {
            android_roots: vec![fixture.ca.root_b64()],
            ..AttestationTrust::default()
        }
    );
}

// ---------------------------------------------------------------------------
// Apple App Attest: synthetic chains and attestation objects only.
// ---------------------------------------------------------------------------

/// Production App Attest AAGUID: `appattest` padded with zeros.
pub(in crate::mobile_memory) const APPLE_AAGUID: [u8; 16] = *b"appattest\0\0\0\0\0\0\0";

fn uncompressed(device: &SigningKey) -> Vec<u8> {
    device
        .verifying_key()
        .to_sec1_point(false)
        .as_bytes()
        .to_vec()
}

/// Builds one synthetic App Attest attestation object. Every knob defaults to
/// a valid object for `app_id`, `device` and `client_data_hash`.
pub(in crate::mobile_memory) struct AppleAttestation<'a> {
    pub ca: &'a Ca,
    pub device: &'a SigningKey,
    pub app_id: String,
    pub client_data_hash: [u8; 32],
    pub aaguid: [u8; 16],
    pub counter: u32,
    /// The certified key, when it should differ from `device`.
    pub certified: Option<&'a SigningKey>,
    /// The credential ID, when it should not be SHA-256 of the certified key.
    pub credential_id: Option<Vec<u8>>,
    /// The nonce, when it should not be SHA-256(authData ‖ clientDataHash).
    pub nonce: Option<[u8; 32]>,
    pub fmt: &'static str,
}

impl<'a> AppleAttestation<'a> {
    pub(in crate::mobile_memory) fn new(
        ca: &'a Ca,
        device: &'a SigningKey,
        client_data_hash: [u8; 32],
    ) -> Self {
        Self {
            ca,
            device,
            app_id: format!("{TEAM_ID}.{BUNDLE_ID}"),
            client_data_hash,
            aaguid: APPLE_AAGUID,
            counter: 0,
            certified: None,
            credential_id: None,
            nonce: None,
            fmt: "apple-appattest",
        }
    }

    /// rpIdHash ‖ flags ‖ counter ‖ aaguid ‖ credentialIdLength ‖ credentialId.
    pub(in crate::mobile_memory) fn auth_data(&self) -> Vec<u8> {
        let certified = self.certified.unwrap_or(self.device);
        let credential_id = self
            .credential_id
            .clone()
            .unwrap_or_else(|| Sha256::digest(uncompressed(certified)).to_vec());
        let mut out = Sha256::digest(self.app_id.as_bytes()).to_vec();
        out.push(0x40);
        out.extend_from_slice(&self.counter.to_be_bytes());
        out.extend_from_slice(&self.aaguid);
        out.extend_from_slice(&(credential_id.len() as u16).to_be_bytes());
        out.extend_from_slice(&credential_id);
        out
    }

    pub(in crate::mobile_memory) fn object(&self) -> Vec<u8> {
        let auth_data = self.auth_data();
        let nonce = self.nonce.unwrap_or_else(|| {
            Sha256::new()
                .chain_update(&auth_data)
                .chain_update(self.client_data_hash)
                .finalize()
                .into()
        });
        // SEQUENCE { [1] EXPLICIT OCTET STRING nonce }
        let extension = tlv(&[0x30], &tlv(&[0xa1], &tlv(&[0x04], &nonce)));
        let certified = self.certified.unwrap_or(self.device);
        let leaf = self.ca.leaf_with(APPLE_NONCE_OID, certified, extension);
        let chain = [leaf, self.ca.root_b64()]
            .iter()
            .map(|cert| Value::Bytes(URL_SAFE_NO_PAD.decode(cert).unwrap()))
            .collect();
        let object = Value::Map(vec![
            (Value::Text("fmt".into()), Value::Text(self.fmt.into())),
            (
                Value::Text("attStmt".into()),
                Value::Map(vec![
                    (Value::Text("x5c".into()), Value::Array(chain)),
                    (Value::Text("receipt".into()), Value::Bytes(Vec::new())),
                ]),
            ),
            (Value::Text("authData".into()), Value::Bytes(auth_data)),
        ]);
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&object, &mut encoded).unwrap();
        encoded
    }

    pub(in crate::mobile_memory) fn evidence(&self) -> AttestationEvidence {
        AttestationEvidence::AppleAppAttest {
            attestation_object: URL_SAFE_NO_PAD.encode(self.object()),
        }
    }
}

pub(in crate::mobile_memory) fn apple_trust(ca: &Ca, official: bool) -> AttestationTrust {
    AttestationTrust {
        apple_roots: vec![ca.root_b64()],
        official_apple_apps: if official {
            vec![OfficialAppleApp {
                team_id: TEAM_ID.into(),
                bundle_id: BUNDLE_ID.into(),
            }]
        } else {
            Vec::new()
        },
        ..AttestationTrust::default()
    }
}

fn apple_object(bytes: Vec<u8>) -> AttestationEvidence {
    AttestationEvidence::AppleAppAttest {
        attestation_object: URL_SAFE_NO_PAD.encode(bytes),
    }
}

#[test]
fn apple_app_attest_for_an_owner_named_app_is_verified_official_app() {
    let fixture = Fixture::new();
    let attestation = AppleAttestation::new(&fixture.ca, &fixture.device, fixture.challenge);
    let trust = apple_trust(&fixture.ca, true);
    assert_eq!(
        fixture.evaluate(Some(&attestation.evidence()), &trust),
        DeviceAttestation::VerifiedOfficialApp
    );
    // App Attest always names the app, so without an owner-named app ID the
    // RP ID hash is unverifiable and nothing is raised.
    assert_eq!(
        fixture.evaluate(
            Some(&attestation.evidence()),
            &apple_trust(&fixture.ca, false)
        ),
        DeviceAttestation::UnattestedDevice
    );
    // Android anchors never anchor an Apple chain, and vice versa.
    assert_eq!(
        fixture.evaluate(Some(&attestation.evidence()), &fixture.trust(true)),
        DeviceAttestation::UnattestedDevice
    );
    assert_eq!(
        fixture.evaluate(Some(&fixture.evidence(1, &fixture.challenge, true)), &trust),
        DeviceAttestation::UnattestedDevice
    );
    // The evidence round-trips through the envelope encoding. Signatures are
    // randomized, so build the object once.
    let evidence = attestation.evidence();
    let json = serde_json::to_string(&evidence).unwrap();
    assert!(json.contains(r#""platform":"appleAppAttest""#));
    assert_eq!(
        serde_json::from_str::<AttestationEvidence>(&json).unwrap(),
        evidence
    );
}

#[test]
fn apple_app_attest_rejects_wrong_nonce_rp_id_anchor_key_and_replay() {
    let fixture = Fixture::new();
    let trust = apple_trust(&fixture.ca, true);
    let unattested = DeviceAttestation::UnattestedDevice;
    let check =
        |attestation: &AppleAttestation| fixture.evaluate(Some(&attestation.evidence()), &trust);
    let mut attestation = AppleAttestation::new(&fixture.ca, &fixture.device, fixture.challenge);
    assert_eq!(check(&attestation), DeviceAttestation::VerifiedOfficialApp);

    // Wrong nonce: the certificate does not commit to this authData.
    attestation.nonce = Some([9; 32]);
    assert_eq!(check(&attestation), unattested);
    attestation.nonce = None;

    // Replay: a valid object made for another request's challenge.
    attestation.client_data_hash = attestation_challenge(b"another transcript");
    assert_eq!(check(&attestation), unattested);
    attestation.client_data_hash = fixture.challenge;

    // Wrong RP ID: another team or bundle, even under the same anchor.
    attestation.app_id = format!("ZZZZZ99999.{BUNDLE_ID}");
    assert_eq!(check(&attestation), unattested);
    attestation.app_id = format!("{TEAM_ID}.org.example.other");
    assert_eq!(check(&attestation), unattested);
    attestation.app_id = format!("{TEAM_ID}.{BUNDLE_ID}");

    // Unanchored: a chain to a CA the owner never supplied.
    let rogue = Ca::new();
    let unanchored = AppleAttestation::new(&rogue, &fixture.device, fixture.challenge);
    assert_eq!(check(&unanchored), unattested);
    // Including the rogue root in the trust store for another platform
    // changes nothing either.
    let mut android_only = trust.clone();
    android_only.android_roots.push(rogue.root_b64());
    assert_eq!(
        fixture.evaluate(Some(&unanchored.evidence()), &android_only),
        unattested
    );

    // Wrong key: the leaf certifies another key, with a consistent
    // credential ID, or the credential ID names another key.
    let other = SigningKey::from_slice(&[4; 32]).unwrap();
    attestation.certified = Some(&other);
    assert_eq!(check(&attestation), unattested);
    attestation.certified = None;
    attestation.credential_id = Some(Sha256::digest(uncompressed(&other)).to_vec());
    assert_eq!(check(&attestation), unattested);
    attestation.credential_id = None;

    // A development-environment or already-used key proves nothing here.
    attestation.aaguid = *b"appattestdevelop";
    assert_eq!(check(&attestation), unattested);
    attestation.aaguid = APPLE_AAGUID;
    attestation.counter = 1;
    assert_eq!(check(&attestation), unattested);
    attestation.counter = 0;

    // Another format under the same keys is not App Attest.
    attestation.fmt = "packed";
    assert_eq!(check(&attestation), unattested);
    attestation.fmt = "apple-appattest";

    // Expired or not-yet-valid certificates fail as for Android.
    assert_eq!(
        evaluate(
            Some(&attestation.evidence()),
            &trust,
            &fixture.public(),
            &fixture.challenge,
            DateTime::from_timestamp(0, 0).unwrap(),
        ),
        unattested
    );
    assert_eq!(check(&attestation), DeviceAttestation::VerifiedOfficialApp);
}

#[test]
fn apple_app_attest_malformed_objects_are_unattested_not_errors() {
    let fixture = Fixture::new();
    let trust = apple_trust(&fixture.ca, true);
    let unattested = DeviceAttestation::UnattestedDevice;
    let valid = AppleAttestation::new(&fixture.ca, &fixture.device, fixture.challenge).object();
    let check = |bytes: Vec<u8>| fixture.evaluate(Some(&apple_object(bytes)), &trust);
    assert_eq!(check(valid.clone()), DeviceAttestation::VerifiedOfficialApp);

    // Not CBOR, truncated, trailing data, and wrong top-level type.
    assert_eq!(check(b"not cbor at all".to_vec()), unattested);
    assert_eq!(check(valid[..valid.len() / 2].to_vec()), unattested);
    assert_eq!(check([valid.clone(), vec![0x00]].concat()), unattested);
    assert_eq!(check(vec![0x80]), unattested);
    assert_eq!(check(Vec::new()), unattested);
    // Not base64url at all.
    assert_eq!(
        fixture.evaluate(
            Some(&AttestationEvidence::AppleAppAttest {
                attestation_object: "***".into(),
            }),
            &trust
        ),
        unattested
    );
    // Missing or duplicated members.
    let mut without_auth_data = Vec::new();
    ciborium::ser::into_writer(
        &Value::Map(vec![
            (
                Value::Text("fmt".into()),
                Value::Text("apple-appattest".into()),
            ),
            (Value::Text("attStmt".into()), Value::Map(Vec::new())),
        ]),
        &mut without_auth_data,
    )
    .unwrap();
    assert_eq!(check(without_auth_data), unattested);
    let Value::Map(mut members) = ciborium::de::from_reader::<Value, _>(&valid[..]).unwrap() else {
        unreachable!()
    };
    members.push(members[0].clone());
    let mut duplicated = Vec::new();
    ciborium::ser::into_writer(&Value::Map(members), &mut duplicated).unwrap();
    assert_eq!(check(duplicated), unattested);
    // Deep nesting and oversized input are refused by bound, not by stack.
    let mut nested = vec![0x81u8; 10_000];
    nested.push(0x00);
    assert_eq!(check(nested), unattested);
    let mut oversized = Vec::new();
    ciborium::ser::into_writer(&Value::Bytes(vec![0u8; 64 * 1024]), &mut oversized).unwrap();
    assert_eq!(check(oversized), unattested);
    // An x5c that is not a byte-string array, or empty.
    let mut bad_chain = Vec::new();
    ciborium::ser::into_writer(
        &Value::Map(vec![
            (
                Value::Text("fmt".into()),
                Value::Text("apple-appattest".into()),
            ),
            (
                Value::Text("attStmt".into()),
                Value::Map(vec![(
                    Value::Text("x5c".into()),
                    Value::Array(vec![Value::Text("leaf".into())]),
                )]),
            ),
            (Value::Text("authData".into()), Value::Bytes(vec![0; 87])),
        ]),
        &mut bad_chain,
    )
    .unwrap();
    assert_eq!(check(bad_chain), unattested);
}
