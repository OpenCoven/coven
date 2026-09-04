use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::authority::{
    validate_authority_profile, AuthorityConsumerClass, AuthorityEvidenceVerifier,
    AuthorityProfileDisposition, AuthorityProfileError, AuthorityProfileErrorCode,
    AuthorityValidationPhase, AutomationAuthorityExtension, AUTHORITY_EXTENSION_KEY,
};
use super::types::ExtensionBag;

const VECTORS: &str =
    include_str!("../../../../../spec/coven-automations/authority/v1/test-vectors.json");

fn vectors() -> Value {
    serde_json::from_str(VECTORS).expect("authority vectors must be strict JSON")
}

fn authority_extensions() -> ExtensionBag {
    let vectors = vectors();
    let value = json!({
        AUTHORITY_EXTENSION_KEY: {
            "profile": "coven.automations.authority.v1",
            "kind": "AutomationAuthorityExtension",
            "executionBinding": vectors["fixtures"]["binding"].clone(),
            "receiptEvidence": vectors["fixtures"]["receiptEvidence"].clone()
        }
    });
    serde_json::from_value(value).expect("authority extension bag")
}

#[derive(Debug)]
struct AcceptingVerifier;

impl AuthorityEvidenceVerifier for AcceptingVerifier {
    fn verify(
        &self,
        _extension: &AutomationAuthorityExtension,
        _phase: AuthorityValidationPhase,
    ) -> Result<(), AuthorityProfileError> {
        Ok(())
    }
}

#[derive(Debug)]
struct StaleVerifier;

impl AuthorityEvidenceVerifier for StaleVerifier {
    fn verify(
        &self,
        _extension: &AutomationAuthorityExtension,
        _phase: AuthorityValidationPhase,
    ) -> Result<(), AuthorityProfileError> {
        Err(AuthorityProfileError::new(
            AuthorityProfileErrorCode::Stale,
            "trusted authority snapshot is stale",
        ))
    }
}

#[test]
fn generic_base_consumer_preserves_unknown_extensions_without_interpretation() {
    let values = BTreeMap::from([(
        "org.example.future".to_owned(),
        json!({
            "profile": "future.authority.v9",
            "nested": [null, {"opaque": true}]
        }),
    )]);
    let extensions = ExtensionBag::new(values).expect("namespaced extension");

    let result = validate_authority_profile(
        &extensions,
        AuthorityConsumerClass::GenericBaseV1,
        &["coven.automations.v1"],
        &[],
        AuthorityValidationPhase::Terminal,
        None,
    )
    .expect("generic consumers preserve opaque extensions");

    assert_eq!(
        result,
        AuthorityProfileDisposition::PreservedOpaque(extensions)
    );
}

#[test]
fn runtime_authority_requires_explicit_profile_and_capability_advertisement() {
    let error = validate_authority_profile(
        &authority_extensions(),
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &["coven.automations.v1"],
        &[],
        AuthorityValidationPhase::Terminal,
        Some(&AcceptingVerifier),
    )
    .expect_err("missing advertisement must fail closed");

    assert_eq!(error.code(), AuthorityProfileErrorCode::ProfileRequired);
}

#[test]
fn runtime_authority_requires_a_verification_adapter() {
    let error = validate_authority_profile(
        &authority_extensions(),
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &["coven.automations.v1", "coven.automations.authority.v1"],
        &["automations.runtime-authority.v1"],
        AuthorityValidationPhase::Terminal,
        None,
    )
    .expect_err("missing verification adapter must fail closed");

    assert_eq!(error.code(), AuthorityProfileErrorCode::AdapterMissing);
}

#[test]
fn runtime_authority_projects_and_verifies_the_closed_companion() {
    let expected = serde_json::to_value(authority_extensions()).expect("serialize extension bag")
        [AUTHORITY_EXTENSION_KEY]
        .clone();
    let result = validate_authority_profile(
        &authority_extensions(),
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &["coven.automations.v1", "coven.automations.authority.v1"],
        &["automations.runtime-authority.v1"],
        AuthorityValidationPhase::Terminal,
        Some(&AcceptingVerifier),
    )
    .expect("valid authority extension");

    let AuthorityProfileDisposition::Validated(extension) = result else {
        panic!("Runtime Authority must return a validated projection");
    };
    assert_eq!(
        serde_json::to_value(extension).expect("serialize authority projection"),
        expected
    );
}

#[test]
fn runtime_authority_propagates_typed_semantic_refusals() {
    let error = validate_authority_profile(
        &authority_extensions(),
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &["coven.automations.v1", "coven.automations.authority.v1"],
        &["automations.runtime-authority.v1"],
        AuthorityValidationPhase::Terminal,
        Some(&StaleVerifier),
    )
    .expect_err("stale adapter evidence must fail closed");

    assert_eq!(error.code(), AuthorityProfileErrorCode::Stale);
}

#[test]
fn runtime_authority_rejects_explicit_null_and_unknown_fields() {
    let null_extensions = ExtensionBag::new(BTreeMap::from([(
        AUTHORITY_EXTENSION_KEY.to_owned(),
        Value::Null,
    )]))
    .expect("authority key is namespaced");
    let null_error = validate_authority_profile(
        &null_extensions,
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &["coven.automations.v1", "coven.automations.authority.v1"],
        &["automations.runtime-authority.v1"],
        AuthorityValidationPhase::Terminal,
        Some(&AcceptingVerifier),
    )
    .expect_err("explicit null must fail closed");
    assert_eq!(null_error.code(), AuthorityProfileErrorCode::ProfileMissing);

    let mut value = serde_json::to_value(authority_extensions()).expect("serialize extension bag");
    value[AUTHORITY_EXTENSION_KEY]["executionBinding"]["runtime"]["ambientAuthority"] = json!(true);
    let malformed: ExtensionBag = serde_json::from_value(value).expect("opaque base extension bag");
    let malformed_error = validate_authority_profile(
        &malformed,
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &["coven.automations.v1", "coven.automations.authority.v1"],
        &["automations.runtime-authority.v1"],
        AuthorityValidationPhase::PreDispatch,
        Some(&AcceptingVerifier),
    )
    .expect_err("closed authority projection must reject unknown fields");
    assert_eq!(
        malformed_error.code(),
        AuthorityProfileErrorCode::SchemaUnknownField
    );
}

#[test]
fn runtime_authority_checks_familiar_validity_at_the_decision_boundary() {
    let mut value = serde_json::to_value(authority_extensions()).expect("serialize extension bag");
    value[AUTHORITY_EXTENSION_KEY]["executionBinding"]["familiar"]["verifiedAt"] =
        json!("2026-09-03T11:59:58.000Z");
    value[AUTHORITY_EXTENSION_KEY]["executionBinding"]["familiar"]["validTime"]["notAfter"] =
        json!("2026-09-03T11:59:58.500Z");
    value[AUTHORITY_EXTENSION_KEY]["executionBinding"]["familiar"]["revocation"]["checkedAt"] =
        json!("2026-09-03T11:59:58.000Z");
    value[AUTHORITY_EXTENSION_KEY]["executionBinding"]["familiar"]["retirement"]["checkedAt"] =
        json!("2026-09-03T11:59:58.000Z");
    let extensions: ExtensionBag =
        serde_json::from_value(value).expect("opaque base extension bag");

    let error = validate_authority_profile(
        &extensions,
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &["coven.automations.v1", "coven.automations.authority.v1"],
        &["automations.runtime-authority.v1"],
        AuthorityValidationPhase::Terminal,
        Some(&AcceptingVerifier),
    )
    .expect_err("familiar validity ending before the decision must fail closed");

    assert_eq!(error.code(), AuthorityProfileErrorCode::FamiliarStale);
}

#[test]
fn rust_error_vocabulary_matches_the_published_capability_contract() {
    let capabilities: Value = serde_json::from_str(include_str!(
        "../../../../../spec/coven-automations/authority/v1/capabilities.json"
    ))
    .expect("authority capabilities must be JSON");
    let mut published = capabilities["errorCodes"]
        .as_array()
        .expect("errorCodes array")
        .iter()
        .map(|value| value.as_str().expect("error code string").to_owned())
        .collect::<Vec<_>>();
    let mut rust = AuthorityProfileErrorCode::ALL
        .iter()
        .map(|code| code.as_str().to_owned())
        .collect::<Vec<_>>();
    published.sort();
    rust.sort();
    assert_eq!(rust, published);
}

#[test]
fn runtime_authority_rejects_a_decision_outside_authorization_validity() {
    let mut value = serde_json::to_value(authority_extensions()).expect("serialize extension bag");
    value[AUTHORITY_EXTENSION_KEY]["executionBinding"]["authorization"]["validUntil"] =
        json!("2026-09-03T11:59:30.000Z");
    let extensions: ExtensionBag =
        serde_json::from_value(value).expect("opaque base extension bag");

    let error = validate_authority_profile(
        &extensions,
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &["coven.automations.v1", "coven.automations.authority.v1"],
        &["automations.runtime-authority.v1"],
        AuthorityValidationPhase::Terminal,
        Some(&AcceptingVerifier),
    )
    .expect_err("decision outside authorization validity must fail closed");

    assert_eq!(error.code(), AuthorityProfileErrorCode::ChronologyInvalid);
}

#[test]
fn runtime_authority_rejects_approval_bypass_before_adapter_verification() {
    for path in ["executionBinding", "receiptEvidence"] {
        let mut value =
            serde_json::to_value(authority_extensions()).expect("serialize extension bag");
        value[AUTHORITY_EXTENSION_KEY][path]["authorization"]["outcome"] = json!("permit");
        let extensions: ExtensionBag =
            serde_json::from_value(value).expect("opaque base extension bag");

        let error = validate_authority_profile(
            &extensions,
            AuthorityConsumerClass::RuntimeAuthorityV1,
            &["coven.automations.v1", "coven.automations.authority.v1"],
            &["automations.runtime-authority.v1"],
            AuthorityValidationPhase::Terminal,
            Some(&AcceptingVerifier),
        )
        .expect_err("permit with required approval must fail closed");

        assert_eq!(error.code(), AuthorityProfileErrorCode::ApprovalRequired);
    }
}

#[test]
fn runtime_authority_rejects_ungranted_exercised_capabilities() {
    let mut value = serde_json::to_value(authority_extensions()).expect("serialize extension bag");
    value[AUTHORITY_EXTENSION_KEY]["receiptEvidence"]["capabilities"]["exercised"] =
        json!(["analysis.read", "artifact.write", "network.publish"]);
    let extensions: ExtensionBag =
        serde_json::from_value(value).expect("opaque base extension bag");

    let error = validate_authority_profile(
        &extensions,
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &["coven.automations.v1", "coven.automations.authority.v1"],
        &["automations.runtime-authority.v1"],
        AuthorityValidationPhase::Terminal,
        Some(&AcceptingVerifier),
    )
    .expect_err("receipt cannot exercise an ungranted capability");

    assert_eq!(
        error.code(),
        AuthorityProfileErrorCode::CapabilityEscalation
    );
}

#[test]
fn runtime_authority_rejects_expanded_binding_capabilities() {
    let mut value = serde_json::to_value(authority_extensions()).expect("serialize extension bag");
    value[AUTHORITY_EXTENSION_KEY]["executionBinding"]["capabilities"]["granted"] =
        json!(["analysis.read", "artifact.write", "network.publish"]);
    let extensions: ExtensionBag =
        serde_json::from_value(value).expect("opaque base extension bag");

    let error = validate_authority_profile(
        &extensions,
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &["coven.automations.v1", "coven.automations.authority.v1"],
        &["automations.runtime-authority.v1"],
        AuthorityValidationPhase::Terminal,
        Some(&AcceptingVerifier),
    )
    .expect_err("binding cannot grant an unrequested or unavailable capability");

    assert_eq!(
        error.code(),
        AuthorityProfileErrorCode::CapabilityEscalation
    );
}

#[test]
fn rust_projection_accepts_the_shared_astral_runtime_id_vector() {
    let vectors = vectors();
    let vector = vectors["cases"]
        .as_array()
        .expect("cases array")
        .iter()
        .find(|vector| vector["id"] == "astral-runtime-id-64-code-points")
        .expect("astral runtime vector");
    let mut value = serde_json::to_value(authority_extensions()).expect("serialize extension bag");
    value[AUTHORITY_EXTENSION_KEY]["receiptEvidence"] = Value::Null;
    for mutation in vector["mutations"].as_array().expect("vector mutations") {
        let path = mutation["path"].as_str().expect("mutation path");
        let target = match path {
            "/runtime/runtimeId" => {
                &mut value[AUTHORITY_EXTENSION_KEY]["executionBinding"]["runtime"]["runtimeId"]
            }
            "/integrity/value" => {
                &mut value[AUTHORITY_EXTENSION_KEY]["executionBinding"]["integrity"]["value"]
            }
            "/authentication/signedDigest" => {
                &mut value[AUTHORITY_EXTENSION_KEY]["executionBinding"]["authentication"]
                    ["signedDigest"]
            }
            "/authentication/signature" => {
                &mut value[AUTHORITY_EXTENSION_KEY]["executionBinding"]["authentication"]
                    ["signature"]
            }
            other => panic!("unexpected astral vector mutation {other}"),
        };
        *target = mutation["value"].clone();
    }
    let extensions: ExtensionBag =
        serde_json::from_value(value).expect("opaque base extension bag");

    validate_authority_profile(
        &extensions,
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &["coven.automations.v1", "coven.automations.authority.v1"],
        &["automations.runtime-authority.v1"],
        AuthorityValidationPhase::PreDispatch,
        Some(&AcceptingVerifier),
    )
    .expect("Rust and JS must count JSON Schema string length by Unicode code point");
}
