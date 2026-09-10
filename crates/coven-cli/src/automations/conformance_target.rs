use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::capability_negotiation::{negotiate_definition, DefinitionNegotiation};

const TARGET_CAPABILITY_SCHEMA_VERSION: &str = "coven.automations.conformance-target-capability.v1";
const SUITE_REQUEST_SCHEMA_VERSION: &str = "coven.automations.conformance-suite-request.v1";
const SUITE_RESULT_SCHEMA_VERSION: &str = "coven.automations.conformance-suite-result.v1";
const CAPABILITY_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.capability-negotiation-vectors.v1";
const STRUCTURAL_PROFILE: &str = "structural";
const MAX_CASES: usize = 128;

pub const CAPABILITY_NEGOTIATION_SUITE: &str = "capability-negotiation";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetCapability {
    schema_version: &'static str,
    profiles: Vec<TargetProfileCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TargetProfileCapability {
    profile: &'static str,
    suites: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSuiteResult {
    schema_version: &'static str,
    suite_id: String,
    pub status: TargetSuiteStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetSuiteStatus {
    Passed,
    Failed,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetSuiteRequest {
    schema_version: String,
    profile: String,
    suite_id: String,
    protocol_artifact: Value,
    subject_artifact: Value,
    vector: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilityVectorSet {
    schema_version: String,
    cases: Vec<CapabilityVectorCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilityVectorCase {
    case_id: String,
    definition: Value,
    expected: ExpectedNegotiation,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
enum ExpectedNegotiation {
    Supported,
    Unsupported { variant: String },
}

#[must_use]
pub fn capability() -> TargetCapability {
    TargetCapability {
        schema_version: TARGET_CAPABILITY_SCHEMA_VERSION,
        profiles: vec![TargetProfileCapability {
            profile: STRUCTURAL_PROFILE,
            suites: vec![CAPABILITY_NEGOTIATION_SUITE],
        }],
    }
}

pub fn evaluate(request: &Value) -> Result<TargetSuiteResult, &'static str> {
    let request: TargetSuiteRequest =
        serde_json::from_value(request.clone()).map_err(|_| "conformance request is invalid")?;
    if request.schema_version != SUITE_REQUEST_SCHEMA_VERSION
        || request.profile != STRUCTURAL_PROFILE
        || request.suite_id != CAPABILITY_NEGOTIATION_SUITE
    {
        return Err("conformance suite is unsupported");
    }
    if !request.protocol_artifact.is_object() || !request.subject_artifact.is_object() {
        return Err("conformance request is invalid");
    }

    let vectors: CapabilityVectorSet = serde_json::from_value(request.vector.clone())
        .map_err(|_| "conformance vector is invalid")?;
    if vectors.schema_version != CAPABILITY_VECTOR_SCHEMA_VERSION
        || vectors.cases.is_empty()
        || vectors.cases.len() > MAX_CASES
    {
        return Err("conformance vector is invalid");
    }

    let mut case_ids = BTreeSet::new();
    for case in &vectors.cases {
        if !valid_case_id(&case.case_id)
            || !case_ids.insert(&case.case_id)
            || !case.definition.is_object()
        {
            return Err("conformance vector is invalid");
        }
    }

    let all_passed = vectors.cases.iter().all(case_matches);
    let evidence = if all_passed {
        let canonical =
            serde_jcs::to_vec(&request.vector).map_err(|_| "conformance vector is invalid")?;
        let vector_digest: String = Sha256::digest(canonical)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Some(json!({
            "executedCases": vectors.cases.len(),
            "passedCases": vectors.cases.len(),
            "vectorDigest": format!("sha256:{vector_digest}")
        }))
    } else {
        None
    };

    Ok(TargetSuiteResult {
        schema_version: SUITE_RESULT_SCHEMA_VERSION,
        suite_id: request.suite_id,
        status: if all_passed {
            TargetSuiteStatus::Passed
        } else {
            TargetSuiteStatus::Failed
        },
        evidence,
    })
}

fn case_matches(case: &CapabilityVectorCase) -> bool {
    match (negotiate_definition(&case.definition), &case.expected) {
        (Ok(DefinitionNegotiation::Supported(_)), ExpectedNegotiation::Supported) => true,
        (
            Ok(DefinitionNegotiation::Unsupported(observed)),
            ExpectedNegotiation::Unsupported { variant },
        ) => observed.variant == *variant,
        _ => false,
    }
}

fn valid_case_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric()
                || (index > 0 && matches!(byte, b'.' | b'_' | b':' | b'@' | b'-'))
        })
}
