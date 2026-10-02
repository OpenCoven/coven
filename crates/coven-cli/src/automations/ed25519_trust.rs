//! Ed25519 authentication of Runtime Authority evidence against keys that are
//! trusted out of band (coven#857).
//!
//! Execution bindings, receipt authority evidence and runtime terminal
//! evidence each carry `authentication { method: "ed25519", keyId, proofRef,
//! signedDigest, signature }`. The contract modules already check that
//! `signedDigest` is SHA-256 over `domain || 0x00 || JCS(body)`. This module
//! checks the Ed25519 signature over those raw 32 digest bytes, which is how
//! the published authority vectors are signed.
//!
//! A key authenticates only its exact `keyId` and `proofRef`, only for its
//! producer when it names one, only inside its validity window, and never once
//! revoked. An empty set authenticates nothing. Authentication is not
//! authorization: a binding still needs the trusted-state checks a Runtime
//! Authority adapter performs, and nothing here constructs that mode or
//! advertises its capability.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ring::signature::{UnparsedPublicKey, ED25519};

use super::contract::authority::{
    AuthorityAuthentication, AuthorityProducer, AuthorityProfileError, AuthorityProfileErrorCode,
    AuthorityTimestamp, AuthorityValidationPhase, AutomationAuthorityExtension,
};
use super::contract::runtime_terminal_evidence::{
    validate_integrity, RuntimeTerminalEvidence, RuntimeTerminalEvidenceError,
    RuntimeTerminalEvidenceErrorCode, RuntimeTerminalEvidenceVerifier,
};

/// The fixed SubjectPublicKeyInfo prefix of an Ed25519 public key (RFC 8410).
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// Why a trusted key could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrustedKeyError {
    /// Not lowercase-hex DER of an Ed25519 SubjectPublicKeyInfo.
    PublicKeyInvalid,
    /// The window closes before it opens.
    ValidityInvalid,
    /// The set already trusts this `keyId` and `proofRef`.
    Duplicate,
}

/// Why evidence was not authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyRefusal {
    /// No trusted key has this `keyId` and `proofRef`, or it is scoped to
    /// another producer.
    Untrusted,
    /// The key is revoked, or the evidence claims a time outside its window.
    Stale,
    /// The signature does not verify, or the authentication is malformed.
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProducerScope {
    component: String,
    instance_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedKey {
    key_id: String,
    proof_ref: String,
    public_key: [u8; 32],
    valid_from: DateTime<Utc>,
    valid_until: Option<DateTime<Utc>>,
    producer: Option<ProducerScope>,
    revoked: bool,
}

impl TrustedKey {
    /// A key for one `keyId` and `proofRef`, from the lowercase-hex DER the
    /// contract objects use, valid from `valid_from` until `valid_until`
    /// (exclusive) or indefinitely.
    pub(crate) fn from_spki_der_hex(
        key_id: &str,
        proof_ref: &str,
        public_key_der_hex: &str,
        valid_from: DateTime<Utc>,
        valid_until: Option<DateTime<Utc>>,
    ) -> Result<Self, TrustedKeyError> {
        let der = decode_lower_hex(public_key_der_hex).ok_or(TrustedKeyError::PublicKeyInvalid)?;
        let public_key = der
            .strip_prefix(&ED25519_SPKI_PREFIX)
            .and_then(|key| <[u8; 32]>::try_from(key).ok())
            .ok_or(TrustedKeyError::PublicKeyInvalid)?;
        if valid_until.is_some_and(|until| until <= valid_from) {
            return Err(TrustedKeyError::ValidityInvalid);
        }
        Ok(Self {
            key_id: key_id.to_owned(),
            proof_ref: proof_ref.to_owned(),
            public_key,
            valid_from,
            valid_until,
            producer: None,
            revoked: false,
        })
    }

    /// Authenticates only evidence produced by this exact producer.
    pub(crate) fn for_producer(mut self, component: &str, instance_id: &str) -> Self {
        self.producer = Some(ProducerScope {
            component: component.to_owned(),
            instance_id: instance_id.to_owned(),
        });
        self
    }

    /// Authenticates nothing, whenever the evidence claims it was signed.
    pub(crate) fn revoked(mut self) -> Self {
        self.revoked = true;
        self
    }
}

/// Keys trusted out of band, by `keyId` and `proofRef`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TrustedKeys(BTreeMap<(String, String), TrustedKey>);

impl TrustedKeys {
    pub(crate) fn insert(&mut self, key: TrustedKey) -> Result<(), TrustedKeyError> {
        let slot = (key.key_id.clone(), key.proof_ref.clone());
        if self.0.contains_key(&slot) {
            return Err(TrustedKeyError::Duplicate);
        }
        self.0.insert(slot, key);
        Ok(())
    }

    /// Checks one signature over the raw bytes of `signed_digest` (64
    /// lowercase hex characters) by the key trusted for `key_id` and
    /// `proof_ref`, for evidence that `producer` claims to have signed at
    /// `signed_at`.
    pub(crate) fn authenticate(
        &self,
        key_id: &str,
        proof_ref: &str,
        producer: (&str, &str),
        signed_at: DateTime<Utc>,
        signed_digest: &str,
        signature: &str,
    ) -> Result<(), KeyRefusal> {
        let key = self
            .0
            .get(&(key_id.to_owned(), proof_ref.to_owned()))
            .ok_or(KeyRefusal::Untrusted)?;
        if key
            .producer
            .as_ref()
            .is_some_and(|scope| (scope.component.as_str(), scope.instance_id.as_str()) != producer)
        {
            return Err(KeyRefusal::Untrusted);
        }
        if key.revoked
            || signed_at < key.valid_from
            || key.valid_until.is_some_and(|until| signed_at >= until)
        {
            return Err(KeyRefusal::Stale);
        }
        let digest = decode_lower_hex(signed_digest)
            .filter(|digest| digest.len() == 32)
            .ok_or(KeyRefusal::Invalid)?;
        let signature = decode_lower_hex(signature)
            .filter(|signature| signature.len() == 64)
            .ok_or(KeyRefusal::Invalid)?;
        UnparsedPublicKey::new(&ED25519, key.public_key)
            .verify(&digest, &signature)
            .map_err(|_| KeyRefusal::Invalid)
    }
}

/// Authenticates the execution binding, and at the terminal boundary the
/// receipt authority evidence too, each by its own producer and decision time.
/// The extension's structure for `phase` is validated first, including each
/// object's integrity, so a signature only ever vouches for the body it is
/// about. A Runtime Authority adapter composes this with its trusted-state
/// checks; on its own it proves who signed, not that the binding may run.
pub(crate) fn authenticate_authority_extension(
    keys: &TrustedKeys,
    extension: &AutomationAuthorityExtension,
    phase: AuthorityValidationPhase,
) -> Result<(), AuthorityProfileError> {
    extension.validate_structure(phase)?;
    let binding = &extension.execution_binding;
    authenticate_authority_object(
        keys,
        &binding.authentication,
        &binding.producer,
        &binding.decision_timestamp,
    )?;
    match extension.receipt_evidence.0.as_deref() {
        Some(evidence) => authenticate_authority_object(
            keys,
            &evidence.authentication,
            &evidence.producer,
            &evidence.decision_timestamp,
        ),
        None => Ok(()),
    }
}

fn authenticate_authority_object(
    keys: &TrustedKeys,
    authentication: &AuthorityAuthentication,
    producer: &AuthorityProducer,
    decided_at: &AuthorityTimestamp,
) -> Result<(), AuthorityProfileError> {
    let signed_at = DateTime::parse_from_rfc3339(decided_at.as_str())
        .map_err(|_| {
            AuthorityProfileError::new(
                AuthorityProfileErrorCode::SchemaInvalid,
                "authority decision timestamp is invalid",
            )
        })?
        .with_timezone(&Utc);
    keys.authenticate(
        authentication.key_id.as_str(),
        authentication.proof_ref.as_str(),
        (producer.component.as_str(), producer.instance_id.as_str()),
        signed_at,
        authentication.signed_digest.as_str(),
        authentication.signature.as_str(),
    )
    .map_err(|refusal| match refusal {
        KeyRefusal::Untrusted => AuthorityProfileError::new(
            AuthorityProfileErrorCode::AuthenticationUnverifiable,
            "authority evidence is not signed by a trusted key",
        ),
        KeyRefusal::Stale => AuthorityProfileError::new(
            AuthorityProfileErrorCode::Stale,
            "authority evidence key is revoked or outside its validity",
        ),
        KeyRefusal::Invalid => AuthorityProfileError::new(
            AuthorityProfileErrorCode::AuthenticationInvalid,
            "authority evidence signature does not verify",
        ),
    })
}

/// Authenticates runtime terminal evidence by its producer and `producedAt`.
/// It rechecks the evidence's integrity itself, so it vouches only for the
/// body the signature is about even when called outside
/// `verify_runtime_terminal_evidence`.
pub(crate) struct Ed25519TerminalEvidenceVerifier<'a>(pub(crate) &'a TrustedKeys);

impl RuntimeTerminalEvidenceVerifier for Ed25519TerminalEvidenceVerifier<'_> {
    fn verify(
        &self,
        evidence: &RuntimeTerminalEvidence,
    ) -> Result<(), RuntimeTerminalEvidenceError> {
        validate_integrity(evidence)?;
        let refuse = RuntimeTerminalEvidenceError::new;
        let signed_at = DateTime::parse_from_rfc3339(evidence.produced_at.as_str())
            .map_err(|_| refuse(RuntimeTerminalEvidenceErrorCode::SchemaInvalid))?
            .with_timezone(&Utc);
        let authentication = &evidence.authentication;
        self.0
            .authenticate(
                authentication.key_id.as_str(),
                authentication.proof_ref.as_str(),
                (
                    evidence.producer.component.as_str(),
                    evidence.producer.instance_id.as_str(),
                ),
                signed_at,
                authentication.signed_digest.value.as_str(),
                authentication.signature.as_str(),
            )
            .map_err(|refusal| {
                refuse(match refusal {
                    KeyRefusal::Untrusted => {
                        RuntimeTerminalEvidenceErrorCode::AuthenticationUnverifiable
                    }
                    KeyRefusal::Stale => RuntimeTerminalEvidenceErrorCode::AuthenticationStale,
                    KeyRefusal::Invalid => RuntimeTerminalEvidenceErrorCode::AuthenticationInvalid,
                })
            })
    }
}

fn decode_lower_hex(value: &str) -> Option<Vec<u8>> {
    let nibble = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|pair| Some((nibble(pair[0])? << 4) | nibble(pair[1])?))
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use serde_json::{json, Value};

    use super::{
        authenticate_authority_extension, Ed25519TerminalEvidenceVerifier, TrustedKey,
        TrustedKeyError, TrustedKeys, ED25519_SPKI_PREFIX,
    };
    use crate::automations::contract::authority::test_support::{
        authority_extensions_value, resign_authority_extensions,
    };
    use crate::automations::contract::authority::{
        AuthorityProfileErrorCode, AuthorityValidationPhase, AutomationAuthorityExtension,
        AUTHORITY_EXTENSION_KEY,
    };
    use crate::automations::contract::runtime_terminal_evidence::test_support::{
        binding, evidence_value, seal,
    };
    use crate::automations::contract::runtime_terminal_evidence::{
        verify_runtime_terminal_evidence, RuntimeTerminalEvidence,
        RuntimeTerminalEvidenceClassification, RuntimeTerminalEvidenceErrorCode,
        RuntimeTerminalEvidenceVerifier,
    };

    const VECTORS: &str =
        include_str!("../../../../spec/coven-automations/authority/v1/test-vectors.json");
    const RUNTIME_SEED: [u8; 32] = [7; 32];

    fn at(timestamp: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(timestamp)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn signed_digest_bytes(evidence: &Value) -> Vec<u8> {
        super::decode_lower_hex(
            evidence["authentication"]["signedDigest"]["value"]
                .as_str()
                .unwrap(),
        )
        .unwrap()
    }

    /// The keys the published authority vectors trust, one per proof.
    fn published_keys(edit: impl Fn(TrustedKey) -> TrustedKey) -> TrustedKeys {
        let vectors: Value = serde_json::from_str(VECTORS).unwrap();
        let mut keys = TrustedKeys::default();
        for (proof_ref, proof) in vectors["trusted"]["authenticationProofs"]
            .as_object()
            .unwrap()
        {
            let key = TrustedKey::from_spki_der_hex(
                proof["keyId"].as_str().unwrap(),
                proof_ref,
                proof["publicKeyDerHex"].as_str().unwrap(),
                at("2026-09-01T00:00:00.000Z"),
                None,
            )
            .unwrap();
            keys.insert(edit(key)).unwrap();
        }
        keys
    }

    fn extension(edit: impl FnOnce(&mut Value)) -> AutomationAuthorityExtension {
        let mut value = authority_extensions_value();
        edit(&mut value[AUTHORITY_EXTENSION_KEY]);
        serde_json::from_value(value[AUTHORITY_EXTENSION_KEY].clone()).unwrap()
    }

    fn refusal(
        keys: &TrustedKeys,
        extension: &AutomationAuthorityExtension,
    ) -> AuthorityProfileErrorCode {
        authenticate_authority_extension(keys, extension, AuthorityValidationPhase::Terminal)
            .unwrap_err()
            .code()
    }

    #[test]
    fn published_authority_vectors_authenticate_with_their_trusted_keys() {
        let keys = published_keys(|key| key);
        // Receipt evidence exists only at the terminal boundary.
        let dispatched = extension(|extension| extension["receiptEvidence"] = Value::Null);
        authenticate_authority_extension(&keys, &dispatched, AuthorityValidationPhase::PreDispatch)
            .unwrap();
        authenticate_authority_extension(
            &keys,
            &extension(|_| {}),
            AuthorityValidationPhase::Terminal,
        )
        .unwrap();
        let scoped = published_keys(|key| key.for_producer("coven-daemon", "daemon:host-a"));
        authenticate_authority_extension(
            &scoped,
            &extension(|_| {}),
            AuthorityValidationPhase::Terminal,
        )
        .unwrap();
    }

    #[test]
    fn authority_evidence_is_refused_without_an_exactly_trusted_live_key() {
        let published = extension(|_| {});
        let unverifiable = AuthorityProfileErrorCode::AuthenticationUnverifiable;
        let stale = AuthorityProfileErrorCode::Stale;
        let cases: [(TrustedKeys, AuthorityProfileErrorCode); 5] = [
            (TrustedKeys::default(), unverifiable),
            (
                published_keys(|key| key.for_producer("coven-daemon", "daemon:host-b")),
                unverifiable,
            ),
            (published_keys(TrustedKey::revoked), stale),
            (
                published_keys(|key| TrustedKey {
                    valid_from: at("2026-09-03T12:00:00.000Z"),
                    ..key
                }),
                stale,
            ),
            (
                published_keys(|key| TrustedKey {
                    valid_until: Some(at("2026-09-03T11:59:59.000Z")),
                    ..key
                }),
                stale,
            ),
        ];
        for (keys, code) in cases {
            assert_eq!(refusal(&keys, &published), code);
        }

        // The binding's key is trusted for the receipt proof only.
        let vectors: Value = serde_json::from_str(VECTORS).unwrap();
        let mut receipt_only = TrustedKeys::default();
        receipt_only
            .insert(
                TrustedKey::from_spki_der_hex(
                    "key:coven-authority-1",
                    "proof:authority-receipt-1",
                    vectors["trusted"]["authenticationProofs"]["proof:authority-receipt-1"]
                        ["publicKeyDerHex"]
                        .as_str()
                        .unwrap(),
                    at("2026-09-01T00:00:00.000Z"),
                    None,
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(refusal(&receipt_only, &published), unverifiable);
    }

    #[test]
    fn a_signature_never_vouches_for_an_edited_body() {
        let keys = published_keys(|key| key);
        // A signed field changes while integrity, signedDigest and signature
        // stay as published.
        for object in ["executionBinding", "receiptEvidence"] {
            let edited = extension(|extension| {
                extension[object]["privacy"]["retention"] = json!("authority_evidence_1y");
            });
            assert_eq!(
                refusal(&keys, &edited),
                AuthorityProfileErrorCode::IntegrityInvalid,
                "{object}"
            );
        }

        let mut value = serde_json::to_value(signed_evidence(|_| {})).unwrap();
        value["sessionId"] = json!("session-attacker");
        let edited: RuntimeTerminalEvidence = serde_json::from_value(value).unwrap();
        let verifier_keys = runtime_keys(|key| key);
        assert_eq!(
            Ed25519TerminalEvidenceVerifier(&verifier_keys)
                .verify(&edited)
                .unwrap_err()
                .code(),
            RuntimeTerminalEvidenceErrorCode::IntegrityInvalid
        );
    }

    #[test]
    fn authority_signatures_cover_exactly_the_signed_digest() {
        let keys = published_keys(|key| key);
        // A flipped signature byte.
        let flipped = extension(|extension| {
            let signature = extension["executionBinding"]["authentication"]["signature"]
                .as_str()
                .unwrap()
                .to_owned();
            let first = if signature.starts_with('0') { "1" } else { "0" };
            extension["executionBinding"]["authentication"]["signature"] =
                json!(format!("{first}{}", &signature[1..]));
        });
        assert_eq!(
            refusal(&keys, &flipped),
            AuthorityProfileErrorCode::AuthenticationInvalid
        );
        // A changed body whose digests are recomputed keeps the old signatures.
        let mut value = authority_extensions_value();
        for object in ["executionBinding", "receiptEvidence"] {
            value[AUTHORITY_EXTENSION_KEY][object]["privacy"]["retention"] =
                json!("authority_evidence_1y");
        }
        resign_authority_extensions(&mut value);
        let resealed: AutomationAuthorityExtension =
            serde_json::from_value(value[AUTHORITY_EXTENSION_KEY].clone()).unwrap();
        assert_eq!(
            refusal(&keys, &resealed),
            AuthorityProfileErrorCode::AuthenticationInvalid
        );
    }

    #[test]
    fn terminal_authority_evidence_requires_receipt_evidence() {
        let keys = published_keys(|key| key);
        let without_receipt = extension(|extension| extension["receiptEvidence"] = Value::Null);
        authenticate_authority_extension(
            &keys,
            &without_receipt,
            AuthorityValidationPhase::PreDispatch,
        )
        .unwrap();
        assert_eq!(
            refusal(&keys, &without_receipt),
            AuthorityProfileErrorCode::ReceiptEvidenceRequired
        );
    }

    #[test]
    fn trusted_keys_accept_only_exact_ed25519_public_keys() {
        let vectors: Value = serde_json::from_str(VECTORS).unwrap();
        let der = vectors["trusted"]["authenticationProofs"]["proof:authority-binding-1"]
            ["publicKeyDerHex"]
            .as_str()
            .unwrap()
            .to_owned();
        let build = |der: &str| {
            TrustedKey::from_spki_der_hex(
                "key:a",
                "proof:a",
                der,
                at("2026-09-01T00:00:00.000Z"),
                None,
            )
        };
        assert!(build(&der).is_ok());
        for invalid in [
            der.to_uppercase(),
            der[2..].to_owned(),
            format!("{der}00"),
            der.replacen("2b6570", "2b6571", 1),
            format!("{}{}", hex(&ED25519_SPKI_PREFIX), "00".repeat(31)),
            "0".repeat(der.len() - 1),
        ] {
            assert_eq!(build(&invalid), Err(TrustedKeyError::PublicKeyInvalid));
        }
        assert_eq!(
            TrustedKey::from_spki_der_hex(
                "key:a",
                "proof:a",
                &der,
                at("2026-09-02T00:00:00.000Z"),
                Some(at("2026-09-02T00:00:00.000Z")),
            ),
            Err(TrustedKeyError::ValidityInvalid)
        );
        let mut keys = TrustedKeys::default();
        keys.insert(build(&der).unwrap()).unwrap();
        assert_eq!(
            keys.insert(build(&der).unwrap()),
            Err(TrustedKeyError::Duplicate)
        );
    }

    fn runtime_key() -> Ed25519KeyPair {
        Ed25519KeyPair::from_seed_unchecked(&RUNTIME_SEED).unwrap()
    }

    fn runtime_keys(edit: impl FnOnce(TrustedKey) -> TrustedKey) -> TrustedKeys {
        let der = [
            ED25519_SPKI_PREFIX.as_slice(),
            runtime_key().public_key().as_ref(),
        ]
        .concat();
        let key = TrustedKey::from_spki_der_hex(
            "key:runtime-instance-1",
            "proof:runtime-instance-1",
            &hex(&der),
            at("2026-09-01T00:00:00.000Z"),
            None,
        )
        .unwrap();
        let mut keys = TrustedKeys::default();
        keys.insert(edit(key)).unwrap();
        keys
    }

    /// Terminal evidence for the published binding, signed by the runtime key.
    fn signed_evidence(edit: impl FnOnce(&mut Value)) -> RuntimeTerminalEvidence {
        let mut value = evidence_value();
        edit(&mut value);
        let mut sealed = seal(value);
        let digest = signed_digest_bytes(&sealed);
        sealed["authentication"]["signature"] = json!(hex(runtime_key().sign(&digest).as_ref()));
        serde_json::from_value(sealed).unwrap()
    }

    fn terminal_refusal(
        keys: &TrustedKeys,
        evidence: RuntimeTerminalEvidence,
    ) -> RuntimeTerminalEvidenceErrorCode {
        verify_runtime_terminal_evidence(
            evidence,
            &binding(),
            &Ed25519TerminalEvidenceVerifier(keys),
        )
        .unwrap_err()
        .code()
    }

    #[test]
    fn signed_runtime_terminal_evidence_is_verified_and_classified() {
        let scoped = runtime_keys(|key| key.for_producer("runtime-adapter", "runtime-instance-1"));
        let verified = verify_runtime_terminal_evidence(
            signed_evidence(|_| {}),
            &binding(),
            &Ed25519TerminalEvidenceVerifier(&scoped),
        )
        .unwrap();
        assert_eq!(
            verified.classification,
            RuntimeTerminalEvidenceClassification::ReceiptEligibleComplete
        );
    }

    #[test]
    fn runtime_terminal_evidence_is_refused_without_a_trusted_signature() {
        use RuntimeTerminalEvidenceErrorCode::{
            AuthenticationInvalid, AuthenticationStale, AuthenticationUnverifiable,
        };
        let evidence = || signed_evidence(|_| {});
        assert_eq!(
            terminal_refusal(&TrustedKeys::default(), evidence()),
            AuthenticationUnverifiable
        );
        assert_eq!(
            terminal_refusal(
                &runtime_keys(|key| key.for_producer("runtime-adapter", "runtime-instance-2")),
                evidence()
            ),
            AuthenticationUnverifiable
        );
        assert_eq!(
            terminal_refusal(&runtime_keys(TrustedKey::revoked), evidence()),
            AuthenticationStale
        );
        assert_eq!(
            terminal_refusal(
                &runtime_keys(|key| TrustedKey {
                    valid_until: Some(at("2026-09-03T12:30:00.000Z")),
                    ..key
                }),
                evidence()
            ),
            AuthenticationStale
        );
        // The placeholder signature `seal` writes is not a signature.
        let unsigned: RuntimeTerminalEvidence =
            serde_json::from_value(seal(evidence_value())).unwrap();
        assert_eq!(
            terminal_refusal(&runtime_keys(|key| key), unsigned),
            AuthenticationInvalid
        );
        // A signature by another key.
        let mut forged = serde_json::to_value(evidence()).unwrap();
        let digest = signed_digest_bytes(&forged);
        let other = Ed25519KeyPair::from_seed_unchecked(&[9; 32]).unwrap();
        forged["authentication"]["signature"] = json!(hex(other.sign(&digest).as_ref()));
        assert_eq!(
            terminal_refusal(
                &runtime_keys(|key| key),
                serde_json::from_value(forged).unwrap()
            ),
            AuthenticationInvalid
        );
    }
}
