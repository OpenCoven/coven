use std::fs;
use std::path::Path;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::{DateTime, Duration, Utc};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use serde_json::{json, Value};

use super::canonical_json::{canonicalize, sha256_hex, MAX_SAFE_INTEGER};
use super::conformance::{
    verify_conformance_result, ConformanceDecisionScope, ConformanceEnvironment,
    ConformanceProfile, ConformanceProfileRequirement, ConformanceResult, ConformanceResultError,
    ConformanceTrustPolicy, ConformanceVerificationClass, ExpectedArtifactBinding,
    ExpectedPolicyBinding, ExpectedProtocolArtifactBinding, ExpectedRunnerBinding,
    ExpectedSourceBinding, ExpectedSubjectArtifactBinding,
};

const SOURCE_REPOSITORY: &str = "https://example.invalid/OpenCoven/coven";
const SOURCE_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const BUNDLE_SCHEMA_VERSION: &str = "coven.automations.bundle.v1";
const BUNDLE_SHA256: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const CONTENT_SHA256: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const RUNNER_NAME: &str = "fixture-conformance-runner";
const RUNNER_VERSION: &str = "1.0.0-test";
const RUNNER_SHA256: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const VECTOR_SHA256: &str = "4444444444444444444444444444444444444444444444444444444444444444";
const SUBJECT_ARTIFACT_ID: &str = "coven-cli-macos-aarch64";
const SUBJECT_ARTIFACT_VERSION: &str = "0.0.0-test";
const SUBJECT_ARTIFACT_SHA256: &str =
    "6666666666666666666666666666666666666666666666666666666666666666";
const POLICY_ID: &str = "release-policy";
const POLICY_VERSION: &str = "2026-09-10";
const POLICY_DIGEST: &str = "5555555555555555555555555555555555555555555555555555555555555555";
const KEY_ID: &str = "fixture-release-key";

type MutationCase = (&'static str, fn(&mut Value), ConformanceResultError);

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-10T13:00:00.000Z")
        .expect("fixed test timestamp")
        .with_timezone(&Utc)
}

fn digest(seed: char) -> String {
    seed.to_string().repeat(64)
}

fn suite_result(suite_id: &str, seed: char) -> Value {
    json!({
        "suiteId": suite_id,
        "status": "passed",
        "evidenceDigest": {
            "algorithm": "sha256",
            "canonicalization": "jcs-rfc8785",
            "value": digest(seed)
        }
    })
}

fn passed_profile(profile: &str, suite_id: &str, seed: char) -> Value {
    json!({
        "profile": profile,
        "status": "passed",
        "requiredSuites": [suite_id],
        "suiteResults": [suite_result(suite_id, seed)]
    })
}

fn base_statement(
    decision_scope: Value,
    profile_results: Vec<Value>,
    overall_status: &str,
) -> Value {
    json!({
        "contractProfile": "coven.automations.v1",
        "resultId": "fixture-conformance-result",
        "decisionScope": decision_scope,
        "source": {
            "repository": SOURCE_REPOSITORY,
            "commit": SOURCE_COMMIT
        },
        "protocolArtifact": {
            "bundleSchemaVersion": BUNDLE_SCHEMA_VERSION,
            "sourceCommit": SOURCE_COMMIT,
            "bundleSha256": BUNDLE_SHA256,
            "contractContentSha256": CONTENT_SHA256,
            "fileCount": 19
        },
        "runner": {
            "name": RUNNER_NAME,
            "version": RUNNER_VERSION,
            "artifactSha256": RUNNER_SHA256,
            "vectorSetSha256": VECTOR_SHA256
        },
        "subjectArtifact": {
            "artifactId": SUBJECT_ARTIFACT_ID,
            "artifactVersion": SUBJECT_ARTIFACT_VERSION,
            "platform": {
                "os": "macos",
                "arch": "aarch64"
            },
            "sha256": SUBJECT_ARTIFACT_SHA256
        },
        "environment": {
            "os": "linux",
            "arch": "x86_64",
            "runtime": "rust-1.89"
        },
        "observedAt": "2026-09-10T12:55:00.000Z",
        "expiresAt": "2026-09-10T13:30:00.000Z",
        "profileResults": profile_results,
        "overallStatus": overall_status
    })
}

fn audit_result() -> Value {
    let statement = base_statement(
        json!({"kind": "audit_only"}),
        vec![json!({
            "profile": "structural",
            "status": "incomplete",
            "requiredSuites": ["schema-validation"],
            "suiteResults": [{
                "suiteId": "schema-validation",
                "status": "incomplete"
            }]
        })],
        "incomplete",
    );
    result_with_statement(statement)
}

fn release_result() -> Value {
    let profiles = vec![
        passed_profile("structural", "schema-validation", 'a'),
        passed_profile("scheduler_reliability", "scheduler-reliability", 'b'),
        passed_profile("runtime_authority", "runtime-authority", 'c'),
        passed_profile("continuity", "continuity", 'd'),
        passed_profile("privacy", "privacy", 'e'),
        passed_profile("interoperability", "interoperability", 'f'),
        passed_profile("full", "full-profile", '6'),
    ];
    let statement = base_statement(
        json!({
            "kind": "release_eligibility",
            "policyBinding": {
                "policyId": POLICY_ID,
                "policyVersion": POLICY_VERSION,
                "digest": POLICY_DIGEST
            }
        }),
        profiles,
        "passed",
    );
    result_with_statement(statement)
}

fn result_with_statement(statement: Value) -> Value {
    let statement_digest = sha256_hex(&canonicalize(&statement).expect("canonical statement"));
    json!({
        "schemaVersion": "coven.automations.conformance-result.v1",
        "statement": statement,
        "statementDigest": {
            "algorithm": "sha256",
            "canonicalization": "jcs-rfc8785",
            "value": statement_digest
        }
    })
}

fn refresh_digest(result: &mut Value) {
    result["statementDigest"]["value"] = json!(sha256_hex(
        &canonicalize(&result["statement"]).expect("canonical statement")
    ));
}

fn signing_key(seed: u8) -> SigningKey {
    let deterministic_scalar =
        p256::SecretKey::from_slice(&[seed; 32]).expect("valid deterministic scalar");
    SigningKey::from(deterministic_scalar)
}

fn sign_result(result: &mut Value, key_id: &str, key: &SigningKey) {
    refresh_digest(result);
    let canonical = canonicalize(&result["statement"]).expect("canonical statement");
    let signature: Signature = key.sign(&canonical);
    result["authentication"] = json!({
        "method": "p256-sha256",
        "keyId": key_id,
        "signature": URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes())
    });
}

fn expected_binding() -> ExpectedArtifactBinding {
    expected_binding_with_file_count(19)
}

fn expected_binding_with_file_count(file_count: u64) -> ExpectedArtifactBinding {
    ExpectedArtifactBinding::new(
        ExpectedSourceBinding::new(SOURCE_REPOSITORY, SOURCE_COMMIT)
            .expect("valid expected source"),
        ExpectedProtocolArtifactBinding::new(
            BUNDLE_SCHEMA_VERSION,
            SOURCE_COMMIT,
            BUNDLE_SHA256,
            CONTENT_SHA256,
            file_count,
        )
        .expect("valid expected protocol artifact"),
        ExpectedRunnerBinding::new(RUNNER_NAME, RUNNER_VERSION, RUNNER_SHA256, VECTOR_SHA256)
            .expect("valid expected runner"),
        ExpectedSubjectArtifactBinding::new(
            SUBJECT_ARTIFACT_ID,
            SUBJECT_ARTIFACT_VERSION,
            "macos",
            "aarch64",
            SUBJECT_ARTIFACT_SHA256,
        )
        .expect("valid expected subject artifact"),
    )
}

fn expected_policy() -> ExpectedPolicyBinding {
    ExpectedPolicyBinding::new(POLICY_ID, POLICY_VERSION, POLICY_DIGEST)
        .expect("valid expected policy")
}

fn audit_trust_policy() -> ConformanceTrustPolicy {
    ConformanceTrustPolicy::new(None, [], [], Duration::hours(1)).expect("valid audit trust policy")
}

fn environment(os: &str, arch: &str, runtime: &str) -> ConformanceEnvironment {
    ConformanceEnvironment::new(os, arch, runtime).expect("valid conformance environment")
}

fn fixture_environment() -> ConformanceEnvironment {
    environment("linux", "x86_64", "rust-1.89")
}

fn profile_requirement(profile: ConformanceProfile) -> ConformanceProfileRequirement {
    let suite = match profile {
        ConformanceProfile::Structural => "schema-validation",
        ConformanceProfile::SchedulerReliability => "scheduler-reliability",
        ConformanceProfile::RuntimeAuthority => "runtime-authority",
        ConformanceProfile::Continuity => "continuity",
        ConformanceProfile::Privacy => "privacy",
        ConformanceProfile::Interoperability => "interoperability",
        ConformanceProfile::Full => "full-profile",
    };
    ConformanceProfileRequirement::new(profile, [suite]).expect("valid profile requirement")
}

fn release_trust_policy(
    required_profiles: impl IntoIterator<Item = ConformanceProfile>,
) -> ConformanceTrustPolicy {
    let mut required_profiles = required_profiles.into_iter().collect::<Vec<_>>();
    if required_profiles.contains(&ConformanceProfile::Full) {
        for component in [
            ConformanceProfile::Structural,
            ConformanceProfile::SchedulerReliability,
            ConformanceProfile::RuntimeAuthority,
            ConformanceProfile::Continuity,
            ConformanceProfile::Privacy,
            ConformanceProfile::Interoperability,
        ] {
            if !required_profiles.contains(&component) {
                required_profiles.push(component);
            }
        }
    }
    let requirements = required_profiles
        .into_iter()
        .map(profile_requirement)
        .collect::<Vec<_>>();
    let mut policy = ConformanceTrustPolicy::new(
        Some(expected_policy()),
        requirements,
        [fixture_environment()],
        Duration::hours(1),
    )
    .expect("valid release trust policy");
    policy
        .trust_key(KEY_ID, signing_key(7).verifying_key().to_owned())
        .expect("valid trusted key");
    policy
}

fn verify(
    result: &Value,
    trust_policy: &ConformanceTrustPolicy,
) -> Result<super::conformance::VerifiedConformanceResult, ConformanceResultError> {
    verify_conformance_result(
        &serde_json::to_vec(result).expect("serialize fixture"),
        &expected_binding(),
        now(),
        trust_policy,
    )
}

fn bytes_with_file_count_lexeme(result: &Value, lexeme: &str) -> Vec<u8> {
    let serialized = serde_json::to_string(result).expect("serialize fixture");
    let file_count = result["statement"]["protocolArtifact"]["fileCount"]
        .as_u64()
        .expect("integer fixture fileCount");
    let replaced = serialized.replacen(
        &format!("\"fileCount\":{file_count}"),
        &format!("\"fileCount\":{lexeme}"),
        1,
    );
    assert_ne!(replaced, serialized, "fixture fileCount must be replaced");
    replaced.into_bytes()
}

fn remove_profile(result: &mut Value, profile: &str) {
    result["statement"]["profileResults"]
        .as_array_mut()
        .expect("profile results")
        .retain(|candidate| candidate["profile"] != profile);
    refresh_digest(result);
}

fn profile_mut<'a>(result: &'a mut Value, profile: &str) -> &'a mut Value {
    result["statement"]["profileResults"]
        .as_array_mut()
        .expect("profile results")
        .iter_mut()
        .find(|candidate| candidate["profile"] == profile)
        .expect("profile fixture")
}

#[test]
fn checked_in_schema_vectors_manifest_and_types_are_listed() {
    let spec_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/coven-automations/v1");
    let schema: Value = serde_json::from_str(
        &fs::read_to_string(spec_dir.join("conformance-result.schema.json"))
            .expect("checked-in conformance result schema"),
    )
    .expect("schema JSON");
    let vectors: Value = serde_json::from_str(
        &fs::read_to_string(spec_dir.join("conformance-result.vectors.json"))
            .expect("checked-in conformance result vectors"),
    )
    .expect("vectors JSON");
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(spec_dir.join("conformance-manifest.json"))
            .expect("checked-in conformance manifest"),
    )
    .expect("manifest JSON");
    let declarations = fs::read_to_string(spec_dir.join("coven.automations.v1.d.ts"))
        .expect("TypeScript contract");

    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(
        vectors["schemaVersion"],
        "coven.automations.conformance-result-vectors.v1"
    );
    let case_names = vectors["cases"]
        .as_array()
        .expect("conformance result cases")
        .iter()
        .map(|case| case["name"].as_str().expect("case name"))
        .collect::<Vec<_>>();
    for expected in [
        "unsigned-audit-incomplete-valid",
        "statement-digest-tampered-invalid",
        "all-not-applicable-cannot-pass",
        "full-pass-missing-component-invalid",
    ] {
        assert!(
            case_names.contains(&expected),
            "missing vector case: {expected}"
        );
    }
    for case in vectors["cases"]
        .as_array()
        .expect("conformance result cases")
    {
        assert!(
            case["object"]["statement"]["subjectArtifact"].is_object(),
            "vector case must bind the tested subject artifact: {}",
            case["name"]
        );
    }
    assert!(manifest["schemas"]
        .as_array()
        .expect("schema list")
        .contains(&json!("conformance-result.schema.json")));
    assert_eq!(
        manifest["conformanceResultVectors"],
        "conformance-result.vectors.json"
    );
    for expected in [
        "export type ConformanceProfile =",
        "\"scheduler_reliability\"",
        "\"runtime_authority\"",
        "export type ConformanceStatus = \"passed\" | \"failed\" | \"incomplete\" | \"not_applicable\";",
        "export interface ConformanceReleaseEligibilityStatement",
        "export type ConformanceResultStatement =",
        "export type ConformanceResult =",
        "export interface ConformanceSubjectArtifactBinding",
        "subjectArtifact: ConformanceSubjectArtifactBinding;",
        "status: \"passed\";",
        "evidenceDigest: Digest;",
        "authentication: ConformanceResultAuthentication;",
        "method: \"p256-sha256\";",
    ] {
        assert!(
            declarations.contains(expected),
            "missing TypeScript projection: {expected}"
        );
    }
}

#[test]
fn checked_in_vectors_match_the_published_json_schema() {
    let spec_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/coven-automations/v1");
    let schema: Value = serde_json::from_str(
        &fs::read_to_string(spec_dir.join("conformance-result.schema.json"))
            .expect("checked-in conformance result schema"),
    )
    .expect("schema JSON");
    let common_schema: Value = serde_json::from_str(
        &fs::read_to_string(spec_dir.join("common.schema.json")).expect("checked-in common schema"),
    )
    .expect("common schema JSON");
    let vectors: Value = serde_json::from_str(
        &fs::read_to_string(spec_dir.join("conformance-result.vectors.json"))
            .expect("checked-in conformance result vectors"),
    )
    .expect("vectors JSON");

    assert!(
        jsonschema::draft202012::meta::is_valid(&schema),
        "conformance-result schema must be valid Draft 2020-12"
    );
    let common_schema_id = common_schema["$id"]
        .as_str()
        .expect("common schema $id")
        .to_owned();
    let registry = jsonschema::Registry::new()
        .add(common_schema_id, common_schema)
        .expect("register common schema")
        .prepare()
        .expect("prepare schema registry");
    let validator = jsonschema::draft202012::options()
        .with_registry(&registry)
        .offline()
        .build(&schema)
        .expect("compile conformance-result schema");

    for case in vectors["cases"].as_array().expect("vector cases") {
        assert!(
            validator.is_valid(&case["object"]),
            "checked-in vector must satisfy the structural schema even when it is a semantic negative case: {}",
            case["name"]
        );
    }

    let mut valid_release = release_result();
    sign_result(&mut valid_release, KEY_ID, &signing_key(7));
    assert!(validator.is_valid(&valid_release));

    let mut invalid_cases = Vec::new();

    let mut missing_authentication = valid_release.clone();
    missing_authentication
        .as_object_mut()
        .expect("result object")
        .remove("authentication");
    invalid_cases.push(("release authentication", missing_authentication));

    let mut missing_expiry = valid_release.clone();
    missing_expiry["statement"]
        .as_object_mut()
        .expect("statement object")
        .remove("expiresAt");
    invalid_cases.push(("release expiry", missing_expiry));

    let mut unknown_member = valid_release.clone();
    unknown_member["statement"]["unexpected"] = json!(true);
    invalid_cases.push(("closed statement shape", unknown_member));

    let mut fractional_file_count = valid_release.clone();
    fractional_file_count["statement"]["protocolArtifact"]["fileCount"] = json!(19.5);
    invalid_cases.push(("integral file count", fractional_file_count));

    let mut missing_passed_evidence = valid_release;
    profile_mut(&mut missing_passed_evidence, "full")["suiteResults"][0]
        .as_object_mut()
        .expect("suite result object")
        .remove("evidenceDigest");
    invalid_cases.push(("passed-suite evidence", missing_passed_evidence));

    for (name, invalid) in invalid_cases {
        assert!(
            !validator.is_valid(&invalid),
            "schema must reject invalid {name}"
        );
    }
}

#[test]
fn checked_in_conformance_result_vectors_match_verifier_semantics() {
    let spec_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/coven-automations/v1");
    let vectors: Value = serde_json::from_str(
        &fs::read_to_string(spec_dir.join("conformance-result.vectors.json"))
            .expect("checked-in conformance result vectors"),
    )
    .expect("vectors JSON");

    for case in vectors["cases"].as_array().expect("vector cases") {
        let name = case["name"].as_str().expect("case name");
        let object = &case["object"];
        match case["expectation"].as_str().expect("case expectation") {
            "accept" => {
                let verified = verify(object, &audit_trust_policy())
                    .unwrap_or_else(|error| panic!("{name} must verify: {error}"));
                assert_eq!(
                    verified.classification(),
                    ConformanceVerificationClass::AuditOnly,
                    "{name}"
                );
            }
            "reject" => {
                let expected = match case["expectedError"].as_str().expect("expected error") {
                    "statement_digest_mismatch" => ConformanceResultError::StatementDigestMismatch,
                    "profile_status_inconsistent" => {
                        ConformanceResultError::ProfileStatusInconsistent
                    }
                    "full_profile_incomplete" => ConformanceResultError::FullProfileIncomplete,
                    _ => panic!("unknown checked-in expected error for {name}"),
                };
                assert_eq!(
                    verify(object, &audit_trust_policy()).expect_err("invalid vector must fail"),
                    expected,
                    "{name}"
                );
            }
            _ => panic!("unknown checked-in expectation for {name}"),
        }
    }
}

#[test]
fn duplicate_json_members_are_rejected_as_schema_invalid() {
    let encoded = serde_json::to_string(&audit_result()).expect("serialize audit fixture");
    let cases = [
        (
            "top-level member",
            encoded.replacen(
                r#""schemaVersion":"coven.automations.conformance-result.v1""#,
                r#""schemaVersion":"coven.automations.conformance-result.v1","schemaVersion":"coven.automations.conformance-result.v1""#,
                1,
            ),
        ),
        (
            "escaped top-level member",
            encoded.replacen(
                r#""schemaVersion":"coven.automations.conformance-result.v1""#,
                r#""schemaVersion":"coven.automations.conformance-result.v1","\u0073chemaVersion":"coven.automations.conformance-result.v1""#,
                1,
            ),
        ),
        (
            "nested member",
            encoded.replacen(
                r#""resultId":"fixture-conformance-result""#,
                r#""resultId":"fixture-conformance-result","resultId":"fixture-conformance-result""#,
                1,
            ),
        ),
    ];

    for (name, bytes) in cases {
        assert_ne!(
            bytes, encoded,
            "duplicate fixture replacement failed: {name}"
        );
        assert_eq!(
            verify_conformance_result(
                bytes.as_bytes(),
                &expected_binding(),
                now(),
                &audit_trust_policy(),
            ),
            Err(ConformanceResultError::SchemaInvalid),
            "{name}"
        );
    }
}

#[test]
fn valid_audit_and_release_fixtures_round_trip_strictly() {
    let audit = audit_result();
    let audit_projection: ConformanceResult =
        serde_json::from_value(audit.clone()).expect("valid audit projection");
    assert_eq!(
        serde_json::to_value(audit_projection).expect("serialize audit projection"),
        audit
    );

    let mut release = release_result();
    sign_result(&mut release, KEY_ID, &signing_key(7));
    let release_projection: ConformanceResult =
        serde_json::from_value(release.clone()).expect("valid release projection");
    assert_eq!(
        serde_json::to_value(release_projection).expect("serialize release projection"),
        release
    );

    let mut unknown = audit_result();
    unknown["statement"]["environment"]["host"] = json!("secret-host");
    assert!(serde_json::from_value::<ConformanceResult>(unknown).is_err());

    for separator in ["t", " "] {
        let mut invalid_timestamp = audit_result();
        invalid_timestamp["statement"]["observedAt"] =
            json!(format!("2026-09-10{separator}12:55:00.000Z"));
        refresh_digest(&mut invalid_timestamp);
        assert!(
            serde_json::from_value::<ConformanceResult>(invalid_timestamp).is_err(),
            "timestamp separator {separator:?} must match the published schema"
        );
    }

    let long_scheme = "averylongcustomrepositoryscheme://host/repository";
    ExpectedSourceBinding::new(long_scheme, SOURCE_COMMIT)
        .expect("schema-valid repository schemes must not gain a Rust-only length limit");
    let mut long_scheme_result = audit_result();
    long_scheme_result["statement"]["source"]["repository"] = json!(long_scheme);
    assert!(serde_json::from_value::<ConformanceResult>(long_scheme_result).is_ok());
}

#[test]
fn file_count_accepts_json_schema_integer_forms_and_normalizes_serialization() {
    for (lexeme, expected) in [
        ("19.0", 19),
        ("1.9e1", 19),
        ("9007199254740991.0", MAX_SAFE_INTEGER),
    ] {
        let mut result = audit_result();
        result["statement"]["protocolArtifact"]["fileCount"] = json!(expected);
        refresh_digest(&mut result);
        let bytes = bytes_with_file_count_lexeme(&result, lexeme);
        let projection: ConformanceResult =
            serde_json::from_slice(&bytes).expect("mathematically integral fileCount");
        assert_eq!(
            serde_json::to_value(projection).expect("serialize conformance result")["statement"]
                ["protocolArtifact"]["fileCount"],
            json!(expected),
            "{lexeme}"
        );
        verify_conformance_result(
            &bytes,
            &expected_binding_with_file_count(expected),
            now(),
            &audit_trust_policy(),
        )
        .unwrap_or_else(|error| panic!("{lexeme} must verify: {error}"));
    }
}

#[test]
fn file_count_rejects_fractional_nonpositive_and_unsafe_values() {
    for lexeme in [
        "19.5",
        "19.0000000000000001",
        "1.90000000000000001e1",
        "0",
        "-1",
        "9007199254740991.0000000000000001",
        "9007199254740992",
    ] {
        let bytes = bytes_with_file_count_lexeme(&audit_result(), lexeme);
        assert!(
            serde_json::from_slice::<ConformanceResult>(&bytes).is_err(),
            "{lexeme}"
        );
        assert_eq!(
            verify_conformance_result(&bytes, &expected_binding(), now(), &audit_trust_policy(),)
                .expect_err("invalid fileCount must fail"),
            ConformanceResultError::SchemaInvalid,
            "{lexeme}"
        );
    }
}

#[test]
fn statement_digest_tampering_is_rejected() {
    let mut result = audit_result();
    result["statement"]["resultId"] = json!("tampered-result");
    assert_eq!(
        verify(&result, &audit_trust_policy()).expect_err("tampering must fail"),
        ConformanceResultError::StatementDigestMismatch
    );
}

#[test]
fn every_exact_source_artifact_and_runner_mismatch_is_rejected() {
    let cases: Vec<MutationCase> = vec![
        (
            "source repository",
            |value| {
                value["statement"]["source"]["repository"] =
                    json!("https://example.invalid/other/repository");
            },
            ConformanceResultError::SourceRepositoryMismatch,
        ),
        (
            "source commit",
            |value| {
                value["statement"]["source"]["commit"] = json!("f".repeat(40));
            },
            ConformanceResultError::SourceCommitMismatch,
        ),
        (
            "bundle schema version",
            |value| {
                value["statement"]["protocolArtifact"]["bundleSchemaVersion"] =
                    json!("coven.automations.bundle.v2");
            },
            ConformanceResultError::BundleSchemaVersionMismatch,
        ),
        (
            "bundle source commit",
            |value| {
                value["statement"]["protocolArtifact"]["sourceCommit"] = json!("e".repeat(40));
            },
            ConformanceResultError::BundleSourceCommitMismatch,
        ),
        (
            "bundle digest",
            |value| {
                value["statement"]["protocolArtifact"]["bundleSha256"] = json!("a".repeat(64));
            },
            ConformanceResultError::BundleDigestMismatch,
        ),
        (
            "content digest",
            |value| {
                value["statement"]["protocolArtifact"]["contractContentSha256"] =
                    json!("b".repeat(64));
            },
            ConformanceResultError::ContractContentDigestMismatch,
        ),
        (
            "file count",
            |value| {
                value["statement"]["protocolArtifact"]["fileCount"] = json!(20);
            },
            ConformanceResultError::FileCountMismatch,
        ),
        (
            "runner name",
            |value| {
                value["statement"]["runner"]["name"] = json!("other-runner");
            },
            ConformanceResultError::RunnerNameMismatch,
        ),
        (
            "runner version",
            |value| {
                value["statement"]["runner"]["version"] = json!("2.0.0");
            },
            ConformanceResultError::RunnerVersionMismatch,
        ),
        (
            "runner artifact digest",
            |value| {
                value["statement"]["runner"]["artifactSha256"] = json!("c".repeat(64));
            },
            ConformanceResultError::RunnerArtifactDigestMismatch,
        ),
        (
            "vector digest",
            |value| {
                value["statement"]["runner"]["vectorSetSha256"] = json!("d".repeat(64));
            },
            ConformanceResultError::VectorSetDigestMismatch,
        ),
        (
            "subject artifact digest",
            |value| {
                value["statement"]["subjectArtifact"]["sha256"] = json!("7".repeat(64));
            },
            ConformanceResultError::SubjectArtifactMismatch,
        ),
    ];

    for (name, mutate, expected) in cases {
        let mut result = audit_result();
        mutate(&mut result);
        refresh_digest(&mut result);
        assert_eq!(
            verify(&result, &audit_trust_policy()).expect_err("mismatch must fail"),
            expected,
            "{name}"
        );
    }
}

#[test]
fn unsigned_audit_is_accepted_only_as_audit_result() {
    let policy = release_trust_policy([ConformanceProfile::Full]);
    let verified = verify(&audit_result(), &policy).expect("unsigned audit is allowed");
    assert_eq!(
        verified.classification(),
        ConformanceVerificationClass::AuditOnly
    );
    assert!(!verified.authentication_verified());
}

#[test]
fn audit_authentication_is_fail_closed_when_present() {
    let mut result = audit_result();
    sign_result(&mut result, "untrusted-audit-key", &signing_key(7));
    assert_eq!(
        verify(&result, &release_trust_policy([ConformanceProfile::Full]))
            .expect_err("present untrusted audit signature must fail"),
        ConformanceResultError::AuthenticationKeyUntrusted
    );

    let mut invalid = audit_result();
    sign_result(&mut invalid, KEY_ID, &signing_key(8));
    assert_eq!(
        verify(&invalid, &release_trust_policy([ConformanceProfile::Full]))
            .expect_err("present invalid audit signature must fail"),
        ConformanceResultError::AuthenticationInvalid
    );
}

#[test]
fn release_requires_exact_policy_binding() {
    let mut missing = release_result();
    missing["statement"]["decisionScope"]
        .as_object_mut()
        .expect("decision scope")
        .remove("policyBinding");
    refresh_digest(&mut missing);
    assert_eq!(
        verify(&missing, &release_trust_policy([ConformanceProfile::Full]))
            .expect_err("missing policy binding"),
        ConformanceResultError::SchemaInvalid
    );

    let mut wrong = release_result();
    wrong["statement"]["decisionScope"]["policyBinding"]["digest"] = json!("9".repeat(64));
    refresh_digest(&mut wrong);
    assert_eq!(
        verify(&wrong, &release_trust_policy([ConformanceProfile::Full]))
            .expect_err("wrong policy binding"),
        ConformanceResultError::PolicyBindingMismatch
    );

    let mut signed = release_result();
    sign_result(&mut signed, KEY_ID, &signing_key(7));
    let policy = audit_trust_policy();
    assert_eq!(
        verify(&signed, &policy).expect_err("release policy must be caller supplied"),
        ConformanceResultError::ReleasePolicyMissing
    );
}

#[test]
fn release_authentication_fails_closed() {
    let trust = release_trust_policy([ConformanceProfile::Full]);

    assert_eq!(
        verify(&release_result(), &trust).expect_err("missing authentication"),
        ConformanceResultError::AuthenticationRequired
    );

    let mut unknown = release_result();
    sign_result(&mut unknown, "unknown-key", &signing_key(7));
    assert_eq!(
        verify(&unknown, &trust).expect_err("unknown key"),
        ConformanceResultError::AuthenticationKeyUntrusted
    );

    let mut invalid = release_result();
    sign_result(&mut invalid, KEY_ID, &signing_key(8));
    assert_eq!(
        verify(&invalid, &trust).expect_err("wrong signing key"),
        ConformanceResultError::AuthenticationInvalid
    );

    let mut non_der = release_result();
    sign_result(&mut non_der, KEY_ID, &signing_key(7));
    let canonical = canonicalize(&non_der["statement"]).expect("canonical statement");
    let raw_signature: Signature = signing_key(7).sign(&canonical);
    non_der["authentication"]["signature"] =
        json!(URL_SAFE_NO_PAD.encode(raw_signature.to_bytes()));
    assert_eq!(
        verify(&non_der, &trust).expect_err("raw signature is not canonical DER"),
        ConformanceResultError::AuthenticationEncodingInvalid
    );

    let mut padded = release_result();
    sign_result(&mut padded, KEY_ID, &signing_key(7));
    let signature = padded["authentication"]["signature"]
        .as_str()
        .expect("signature string");
    padded["authentication"]["signature"] = json!(format!("{signature}="));
    assert_eq!(
        verify(&padded, &trust).expect_err("base64url padding is not canonical"),
        ConformanceResultError::SchemaInvalid
    );
}

#[test]
fn release_timing_must_be_fresh_current_and_unexpired() {
    let trust = release_trust_policy([ConformanceProfile::Full]);
    let mut missing_expiry = release_result();
    missing_expiry["statement"]
        .as_object_mut()
        .expect("statement")
        .remove("expiresAt");
    sign_result(&mut missing_expiry, KEY_ID, &signing_key(7));
    assert_eq!(
        verify(&missing_expiry, &trust).expect_err("release expiry is required"),
        ConformanceResultError::EvidenceExpirationRequired
    );

    let cases = [
        (
            "not yet valid",
            "2026-09-10T13:01:00.000Z",
            "2026-09-10T14:00:00.000Z",
            ConformanceResultError::EvidenceNotYetValid,
        ),
        (
            "stale",
            "2026-09-10T10:00:00.000Z",
            "2026-09-10T14:00:00.000Z",
            ConformanceResultError::EvidenceStale,
        ),
        (
            "expired",
            "2026-09-10T12:55:00.000Z",
            "2026-09-10T12:59:59.000Z",
            ConformanceResultError::EvidenceExpired,
        ),
        (
            "invalid chronology",
            "2026-09-10T12:55:00.000Z",
            "2026-09-10T12:54:59.000Z",
            ConformanceResultError::EvidenceChronologyInvalid,
        ),
    ];

    for (name, observed_at, expires_at, expected) in cases {
        let mut result = release_result();
        result["statement"]["observedAt"] = json!(observed_at);
        result["statement"]["expiresAt"] = json!(expires_at);
        sign_result(&mut result, KEY_ID, &signing_key(7));
        assert_eq!(
            verify(&result, &trust).expect_err("invalid timing must fail"),
            expected,
            "{name}"
        );
    }
}

#[test]
fn release_requires_every_trust_policy_profile_to_pass() {
    let mut missing = release_result();
    remove_profile(&mut missing, "privacy");
    remove_profile(&mut missing, "full");
    sign_result(&mut missing, KEY_ID, &signing_key(7));
    assert_eq!(
        verify(
            &missing,
            &release_trust_policy([ConformanceProfile::Privacy])
        )
        .expect_err("required profile missing"),
        ConformanceResultError::RequiredProfileMissing
    );

    let mut incomplete = release_result();
    let privacy = profile_mut(&mut incomplete, "privacy");
    privacy["status"] = json!("incomplete");
    privacy["suiteResults"][0]["status"] = json!("incomplete");
    privacy["suiteResults"][0]
        .as_object_mut()
        .expect("suite result")
        .remove("evidenceDigest");
    remove_profile(&mut incomplete, "full");
    incomplete["statement"]["overallStatus"] = json!("incomplete");
    sign_result(&mut incomplete, KEY_ID, &signing_key(7));
    assert_eq!(
        verify(
            &incomplete,
            &release_trust_policy([ConformanceProfile::Privacy])
        )
        .expect_err("required profile incomplete"),
        ConformanceResultError::RequiredProfileNotPassed
    );
}

#[test]
fn release_requires_the_caller_pinned_suite_inventory() {
    let mut attack = release_result();
    let full = profile_mut(&mut attack, "full");
    full["requiredSuites"] = json!(["dummy-pass"]);
    full["suiteResults"] = json!([suite_result("dummy-pass", '7')]);
    sign_result(&mut attack, KEY_ID, &signing_key(7));

    assert_eq!(
        verify(&attack, &release_trust_policy([ConformanceProfile::Full]))
            .expect_err("producer-selected dummy suite must not satisfy release policy"),
        ConformanceResultError::RequiredSuiteSetMismatch
    );

    let mut component_attack = release_result();
    let structural = profile_mut(&mut component_attack, "structural");
    structural["requiredSuites"] = json!(["dummy-pass"]);
    structural["suiteResults"] = json!([suite_result("dummy-pass", '7')]);
    sign_result(&mut component_attack, KEY_ID, &signing_key(7));
    assert_eq!(
        verify(
            &component_attack,
            &release_trust_policy([ConformanceProfile::Full])
        )
        .expect_err("full policy must pin every component suite inventory"),
        ConformanceResultError::RequiredSuiteSetMismatch
    );
}

#[test]
fn release_policy_rejects_duplicate_profile_and_suite_requirements() {
    let structural = profile_requirement(ConformanceProfile::Structural);
    assert_eq!(
        ConformanceTrustPolicy::new(
            Some(expected_policy()),
            [structural.clone(), structural],
            [fixture_environment()],
            Duration::hours(1),
        )
        .expect_err("duplicate profile requirements must fail"),
        ConformanceResultError::TrustPolicyInvalid
    );

    assert_eq!(
        ConformanceProfileRequirement::new(
            ConformanceProfile::Structural,
            ["schema-validation", "schema-validation"],
        )
        .expect_err("duplicate suite requirements must fail"),
        ConformanceResultError::TrustPolicyInvalid
    );

    assert_eq!(
        ConformanceTrustPolicy::new(
            Some(expected_policy()),
            [profile_requirement(ConformanceProfile::Full)],
            [fixture_environment()],
            Duration::hours(1),
        )
        .expect_err("full policy must pin all component profile inventories"),
        ConformanceResultError::TrustPolicyInvalid
    );
}

#[test]
fn release_policy_requires_a_nonempty_exact_environment_allowlist() {
    assert_eq!(
        ConformanceTrustPolicy::new(
            Some(expected_policy()),
            [profile_requirement(ConformanceProfile::Full)],
            [],
            Duration::hours(1),
        )
        .expect_err("release environment allowlist must not be empty"),
        ConformanceResultError::TrustPolicyInvalid
    );

    let allowed = fixture_environment();
    let later_runtime = environment("linux", "x86_64", "rust-1.90");
    assert!(allowed < later_runtime);

    let error = ConformanceEnvironment::new("linux", "x86_64", "/private/SECRET")
        .expect_err("environment constructor must validate every fact");
    assert_eq!(error, ConformanceResultError::SchemaInvalid);
    assert!(!error.to_string().contains("SECRET"));
}

#[test]
fn release_environment_must_exactly_match_the_caller_allowlist() {
    let mut disallowed = release_result();
    disallowed["statement"]["environment"]["runtime"] = json!("rust-1.90");
    sign_result(&mut disallowed, KEY_ID, &signing_key(7));
    assert_eq!(
        verify(
            &disallowed,
            &release_trust_policy([ConformanceProfile::Full])
        )
        .expect_err("signed evidence from a disallowed environment must fail"),
        ConformanceResultError::EnvironmentNotAllowed
    );

    let mut allowed = release_result();
    sign_result(&mut allowed, KEY_ID, &signing_key(7));
    verify(&allowed, &release_trust_policy([ConformanceProfile::Full]))
        .expect("exactly allowed environment must verify");
}

#[test]
fn audit_only_environment_remains_informational() {
    let mut result = audit_result();
    result["statement"]["environment"] = json!({
        "os": "macos",
        "arch": "aarch64",
        "runtime": "rust-1.90"
    });
    refresh_digest(&mut result);

    verify(&result, &release_trust_policy([ConformanceProfile::Full]))
        .expect("release allowlist must not constrain audit-only evidence");
}

#[test]
fn profile_suite_semantics_fail_closed() {
    let cases: Vec<MutationCase> = vec![
        (
            "missing required suite result",
            |value| {
                profile_mut(value, "structural")["suiteResults"] = json!([]);
            },
            ConformanceResultError::SuiteSetMismatch,
        ),
        (
            "passed profile with failed suite",
            |value| {
                profile_mut(value, "structural")["suiteResults"][0]["status"] = json!("failed");
            },
            ConformanceResultError::ProfileStatusInconsistent,
        ),
        (
            "passed profile with incomplete suite",
            |value| {
                let suite = &mut profile_mut(value, "structural")["suiteResults"][0];
                suite["status"] = json!("incomplete");
                suite
                    .as_object_mut()
                    .expect("suite result")
                    .remove("evidenceDigest");
            },
            ConformanceResultError::ProfileStatusInconsistent,
        ),
        (
            "passed profile with not-applicable suite",
            |value| {
                let suite = &mut profile_mut(value, "structural")["suiteResults"][0];
                suite["status"] = json!("not_applicable");
                suite
                    .as_object_mut()
                    .expect("suite result")
                    .remove("evidenceDigest");
            },
            ConformanceResultError::ProfileStatusInconsistent,
        ),
        (
            "passed suite without evidence",
            |value| {
                profile_mut(value, "structural")["suiteResults"][0]
                    .as_object_mut()
                    .expect("suite result")
                    .remove("evidenceDigest");
            },
            ConformanceResultError::PassedSuiteEvidenceMissing,
        ),
        (
            "incomplete profile hiding failed suite",
            |value| {
                let profile = profile_mut(value, "structural");
                profile["status"] = json!("incomplete");
                profile["suiteResults"][0]["status"] = json!("failed");
                value["statement"]["overallStatus"] = json!("incomplete");
            },
            ConformanceResultError::ProfileStatusInconsistent,
        ),
        (
            "failed profile without failed suite",
            |value| {
                profile_mut(value, "structural")["status"] = json!("failed");
                value["statement"]["overallStatus"] = json!("failed");
            },
            ConformanceResultError::ProfileStatusInconsistent,
        ),
    ];

    for (name, mutate, expected) in cases {
        let mut result = release_result();
        mutate(&mut result);
        refresh_digest(&mut result);
        assert_eq!(
            verify(&result, &audit_trust_policy()).expect_err("invalid profile semantics"),
            expected,
            "{name}"
        );
    }
}

#[test]
fn duplicate_profiles_and_suites_are_rejected() {
    let mut duplicate_profile = release_result();
    let structural = duplicate_profile["statement"]["profileResults"][0].clone();
    duplicate_profile["statement"]["profileResults"][6] = structural;
    refresh_digest(&mut duplicate_profile);
    assert_eq!(
        verify(&duplicate_profile, &audit_trust_policy()).expect_err("duplicate profile"),
        ConformanceResultError::ProfileDuplicate
    );

    let mut duplicate_required = release_result();
    profile_mut(&mut duplicate_required, "structural")["requiredSuites"] =
        json!(["schema-validation", "schema-validation"]);
    refresh_digest(&mut duplicate_required);
    assert_eq!(
        verify(&duplicate_required, &audit_trust_policy()).expect_err("duplicate required suite"),
        ConformanceResultError::SuiteDuplicate
    );

    let mut duplicate_result = release_result();
    let profile = profile_mut(&mut duplicate_result, "structural");
    let suite = profile["suiteResults"][0].clone();
    profile["suiteResults"]
        .as_array_mut()
        .expect("suite results")
        .push(suite);
    refresh_digest(&mut duplicate_result);
    assert_eq!(
        verify(&duplicate_result, &audit_trust_policy()).expect_err("duplicate suite result"),
        ConformanceResultError::SuiteDuplicate
    );
}

#[test]
fn overall_and_full_profile_statuses_are_derived_not_claimed() {
    let mut overall = release_result();
    overall["statement"]["overallStatus"] = json!("failed");
    refresh_digest(&mut overall);
    assert_eq!(
        verify(&overall, &audit_trust_policy()).expect_err("inconsistent overall status"),
        ConformanceResultError::OverallStatusInconsistent
    );

    let mut incomplete_full = release_result();
    remove_profile(&mut incomplete_full, "privacy");
    refresh_digest(&mut incomplete_full);
    assert_eq!(
        verify(&incomplete_full, &audit_trust_policy())
            .expect_err("full pass requires every component"),
        ConformanceResultError::FullProfileIncomplete
    );
}

#[test]
fn exact_signed_release_fixture_verifies_as_release_eligibility() {
    let mut result = release_result();
    sign_result(&mut result, KEY_ID, &signing_key(7));
    let verified = verify(
        &result,
        &release_trust_policy([
            ConformanceProfile::Structural,
            ConformanceProfile::SchedulerReliability,
            ConformanceProfile::RuntimeAuthority,
            ConformanceProfile::Continuity,
            ConformanceProfile::Privacy,
            ConformanceProfile::Interoperability,
            ConformanceProfile::Full,
        ]),
    )
    .expect("exact signed fixture must verify");

    assert_eq!(
        verified.classification(),
        ConformanceVerificationClass::ReleaseEligibility
    );
    assert!(verified.authentication_verified());
    let ConformanceDecisionScope::ReleaseEligibility { policy_binding } =
        &verified.result().statement.decision_scope
    else {
        panic!("verified release result must expose its policy binding");
    };
    assert_eq!(policy_binding.policy_id.as_str(), POLICY_ID);
    assert_eq!(policy_binding.policy_version.as_str(), POLICY_VERSION);
    assert_eq!(policy_binding.digest.as_str(), POLICY_DIGEST);
}

#[test]
fn verifier_errors_are_static_and_privacy_safe() {
    let sensitive_marker = "PRIVATE_SECRET_VALUE";
    let mut invalid_environment = audit_result();
    invalid_environment["statement"]["environment"]["runtime"] = json!(format!(
        "/private/{sensitive_marker}\nHOST={sensitive_marker}"
    ));
    refresh_digest(&mut invalid_environment);
    let error = verify(&invalid_environment, &audit_trust_policy())
        .expect_err("unbounded environment values must fail");
    assert_eq!(error, ConformanceResultError::SchemaInvalid);
    assert!(!error.to_string().contains(sensitive_marker));
    assert!(!error.to_string().contains("/private/"));

    let mut injected_environment = release_result();
    injected_environment["statement"]["environment"]["runtime"] =
        json!(format!("runtime-{sensitive_marker}"));
    sign_result(&mut injected_environment, KEY_ID, &signing_key(7));
    let error = verify(
        &injected_environment,
        &release_trust_policy([ConformanceProfile::Full]),
    )
    .expect_err("untrusted injected environment must fail");
    assert_eq!(error, ConformanceResultError::EnvironmentNotAllowed);
    assert!(!error.to_string().contains(sensitive_marker));
    assert!(!error.to_string().contains("runtime-"));

    let mut injected_suite = release_result();
    let profile = profile_mut(&mut injected_suite, "full");
    profile["requiredSuites"] = json!([format!("suite-{sensitive_marker}")]);
    profile["suiteResults"][0]["suiteId"] = json!(format!("suite-{sensitive_marker}"));
    sign_result(&mut injected_suite, KEY_ID, &signing_key(7));
    let error = verify(
        &injected_suite,
        &release_trust_policy([ConformanceProfile::Full]),
    )
    .expect_err("untrusted injected suite must fail");
    assert_eq!(error, ConformanceResultError::RequiredSuiteSetMismatch);
    assert!(!error.to_string().contains(sensitive_marker));
    assert!(!error.to_string().contains("suite-"));

    let mut injected_key = release_result();
    injected_key["statement"]["resultId"] = json!(format!("result-{sensitive_marker}"));
    sign_result(
        &mut injected_key,
        &format!("key-{sensitive_marker}"),
        &signing_key(7),
    );
    let error = verify(
        &injected_key,
        &release_trust_policy([ConformanceProfile::Full]),
    )
    .expect_err("untrusted injected key must fail");
    assert_eq!(error, ConformanceResultError::AuthenticationKeyUntrusted);
    assert!(!error.to_string().contains(sensitive_marker));
    assert!(!error.to_string().contains("key-"));
}
