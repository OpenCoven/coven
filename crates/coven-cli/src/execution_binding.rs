//! Psyche `psyche.execution_binding.v1` closed wire contract.
//!
//! Coven treats the execution binding as an opaque, Psyche-defined tuple: it
//! validates syntax, expiry, and exact-match comparison, but never
//! interprets field meaning. See `specs/psyche/O2_CONTRACT_DESIGN.md`.

use std::collections::BTreeSet;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CONTRACT: &str = "psyche.execution_binding.v1";

/// The exact member set required at the root `executionBinding` object,
/// independent of Serde field ordering, so unknown/missing keys are caught
/// before Serde would silently coerce omitted nullable fields into `None`.
const BINDING_FIELDS: [&str; 13] = [
    "attemptId",
    "contract",
    "delegationDigest",
    "expiresAt",
    "familiarId",
    "familiarSnapshotDigest",
    "graphId",
    "nodeId",
    "parent",
    "policyRevision",
    "principalRef",
    "projectDigest",
    "requestDigest",
];

/// The exact member set required at a non-null `executionBinding.parent`.
const PARENT_FIELDS: [&str; 4] = ["attemptId", "graphId", "nodeId", "sessionId"];

/// Immutable identity of the parent attempt that delegated this session, when
/// present. Every field is an opaque, Psyche-defined value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionBindingParent {
    pub session_id: String,
    pub graph_id: String,
    pub node_id: String,
    pub attempt_id: String,
}

/// Typed `psyche.execution_binding.v1` tuple. Coven never interprets these
/// values beyond syntax, version, and expiry; they are opaque to Psyche.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionBinding {
    pub contract: String,
    pub principal_ref: String,
    pub familiar_id: String,
    pub familiar_snapshot_digest: String,
    pub project_digest: String,
    pub graph_id: String,
    pub node_id: String,
    pub attempt_id: String,
    pub request_digest: String,
    pub policy_revision: String,
    pub expires_at: String,
    pub parent: Option<ExecutionBindingParent>,
    pub delegation_digest: Option<String>,
}

/// A shape, syntax, version, or expiry violation. Carries only a static
/// field path — never the offending value — so it can propagate through
/// error responses without leaking request contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValidationError {
    Missing { path: &'static str },
    Invalid { path: &'static str },
    Unsupported { path: &'static str },
    Expired { path: &'static str },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::Missing { path } => write!(f, "missing field at {path}"),
            ValidationError::Invalid { path } => write!(f, "invalid field at {path}"),
            ValidationError::Unsupported { path } => write!(f, "unsupported value at {path}"),
            ValidationError::Expired { path } => write!(f, "expired at {path}"),
        }
    }
}

impl std::error::Error for ValidationError {}

/// Validates that `value` is a JSON object whose key set is exactly
/// `expected`, reporting unknown keys as `Invalid` and any absent key as
/// `Missing`, both at `path`.
fn require_exact_fields(
    value: &Value,
    expected: &[&str],
    path: &'static str,
) -> Result<(), ValidationError> {
    let object = value.as_object().ok_or(ValidationError::Invalid { path })?;
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    if actual.difference(&expected).next().is_some() {
        return Err(ValidationError::Invalid { path });
    }
    if expected.difference(&actual).next().is_some() {
        return Err(ValidationError::Missing { path });
    }
    Ok(())
}

/// Parses and fully validates a `psyche.execution_binding.v1` value from
/// untyped JSON. The exact key-set check runs before Serde deserialization
/// so omitted nullable members (`parent`, `delegationDigest`) are rejected
/// as `Missing` rather than silently collapsing into `None`.
pub fn parse(value: &Value) -> Result<ExecutionBinding, ValidationError> {
    require_exact_fields(value, &BINDING_FIELDS, "executionBinding")?;
    if let Some(parent) = value.get("parent").filter(|parent| !parent.is_null()) {
        require_exact_fields(parent, &PARENT_FIELDS, "executionBinding.parent")?;
    }
    let binding: ExecutionBinding =
        serde_json::from_value(value.clone()).map_err(|_| ValidationError::Invalid {
            path: "executionBinding",
        })?;
    binding.validate_shape()?;
    Ok(binding)
}

/// True when `value` is 1..=255 ASCII bytes drawn only from
/// `[A-Za-z0-9._:/-]`.
fn valid_opaque(value: &str) -> bool {
    (1..=255).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-')
        })
}

/// True when `value` is exactly `sha256:` followed by 64 lowercase hex
/// digits.
fn valid_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Parses a canonical RFC3339 UTC whole-second timestamp
/// (`YYYY-MM-DDTHH:MM:SSZ`): fractional seconds and non-`Z` offsets are
/// rejected by round-tripping the parsed instant back through the same
/// canonical formatting and comparing byte-for-byte.
fn parse_expiry(value: &str) -> Option<DateTime<Utc>> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .ok()?
        .with_timezone(&Utc);
    (parsed.to_rfc3339_opts(SecondsFormat::Secs, true) == value).then_some(parsed)
}

impl ExecutionBinding {
    /// Validates contract version and per-field syntax. Values are never
    /// normalized: a rejected value is reported as-is via the field path.
    pub fn validate_shape(&self) -> Result<(), ValidationError> {
        if self.contract != CONTRACT {
            return Err(ValidationError::Unsupported {
                path: "executionBinding.contract",
            });
        }
        for (path, value) in [
            ("executionBinding.principalRef", self.principal_ref.as_str()),
            ("executionBinding.familiarId", self.familiar_id.as_str()),
            ("executionBinding.graphId", self.graph_id.as_str()),
            ("executionBinding.nodeId", self.node_id.as_str()),
            ("executionBinding.attemptId", self.attempt_id.as_str()),
            (
                "executionBinding.policyRevision",
                self.policy_revision.as_str(),
            ),
        ] {
            if !valid_opaque(value) {
                return Err(ValidationError::Invalid { path });
            }
        }
        for (path, value) in [
            (
                "executionBinding.familiarSnapshotDigest",
                self.familiar_snapshot_digest.as_str(),
            ),
            (
                "executionBinding.projectDigest",
                self.project_digest.as_str(),
            ),
            (
                "executionBinding.requestDigest",
                self.request_digest.as_str(),
            ),
        ] {
            if !valid_digest(value) {
                return Err(ValidationError::Invalid { path });
            }
        }
        if parse_expiry(&self.expires_at).is_none() {
            return Err(ValidationError::Invalid {
                path: "executionBinding.expiresAt",
            });
        }
        if let Some(parent) = &self.parent {
            for (path, value) in [
                (
                    "executionBinding.parent.sessionId",
                    parent.session_id.as_str(),
                ),
                ("executionBinding.parent.graphId", parent.graph_id.as_str()),
                ("executionBinding.parent.nodeId", parent.node_id.as_str()),
                (
                    "executionBinding.parent.attemptId",
                    parent.attempt_id.as_str(),
                ),
            ] {
                if !valid_opaque(value) {
                    return Err(ValidationError::Invalid { path });
                }
            }
        }
        if self
            .delegation_digest
            .as_deref()
            .is_some_and(|value| !valid_digest(value))
        {
            return Err(ValidationError::Invalid {
                path: "executionBinding.delegationDigest",
            });
        }
        Ok(())
    }

    /// Errors with `Expired` when `expiresAt` is less than or equal to
    /// `now` (elapsed is inclusive of the boundary instant). Called by the
    /// launch route (issue #728 Task 3) and the bound input route (Task 4).
    pub fn validate_not_expired(&self, now: DateTime<Utc>) -> Result<(), ValidationError> {
        let expires_at = parse_expiry(&self.expires_at).ok_or(ValidationError::Invalid {
            path: "executionBinding.expiresAt",
        })?;
        if expires_at <= now {
            return Err(ValidationError::Expired {
                path: "executionBinding.expiresAt",
            });
        }
        Ok(())
    }

    /// Returns the field path of the first mismatch between `self`
    /// (expected/stored) and `supplied` (proof), in normative object order:
    /// top-level fields, then nested `parent` fields, then
    /// `delegationDigest` last. `None` when every field matches exactly.
    /// Per §7, `parent` and its members are always named bare (`parent`,
    /// `parent.sessionId`, ...) — never `executionBinding.parent[...]` —
    /// matching the paths the launch-correlation route already reports.
    ///
    /// Consumed by the O2 bound input/kill exact-proof enforcement (issue
    /// #728 Task 4), not by launch-correlation (Task 3): launch correlation
    /// compares individual stored parent fields against individual
    /// submitted `parent` fields, not two complete `ExecutionBinding`
    /// values, so it does not call this helper.
    pub fn first_mismatch_path(&self, supplied: &Self) -> Option<&'static str> {
        let top_level = [
            (
                self.contract != supplied.contract,
                "executionBinding.contract",
            ),
            (
                self.principal_ref != supplied.principal_ref,
                "executionBinding.principalRef",
            ),
            (
                self.familiar_id != supplied.familiar_id,
                "executionBinding.familiarId",
            ),
            (
                self.familiar_snapshot_digest != supplied.familiar_snapshot_digest,
                "executionBinding.familiarSnapshotDigest",
            ),
            (
                self.project_digest != supplied.project_digest,
                "executionBinding.projectDigest",
            ),
            (
                self.graph_id != supplied.graph_id,
                "executionBinding.graphId",
            ),
            (self.node_id != supplied.node_id, "executionBinding.nodeId"),
            (
                self.attempt_id != supplied.attempt_id,
                "executionBinding.attemptId",
            ),
            (
                self.request_digest != supplied.request_digest,
                "executionBinding.requestDigest",
            ),
            (
                self.policy_revision != supplied.policy_revision,
                "executionBinding.policyRevision",
            ),
            (
                self.expires_at != supplied.expires_at,
                "executionBinding.expiresAt",
            ),
        ]
        .into_iter()
        .find_map(|(different, path)| different.then_some(path));
        if top_level.is_some() {
            return top_level;
        }

        match (&self.parent, &supplied.parent) {
            (None, None) => {}
            (Some(expected), Some(actual)) => {
                // Bare `parent.*` — not `executionBinding.parent.*` — per the
                // normative field-path convention (§7): `parent` is named
                // bare in every error path, matching the launch-correlation
                // paths the Task 3 route already reports.
                let parent_mismatch = [
                    (expected.session_id != actual.session_id, "parent.sessionId"),
                    (expected.graph_id != actual.graph_id, "parent.graphId"),
                    (expected.node_id != actual.node_id, "parent.nodeId"),
                    (expected.attempt_id != actual.attempt_id, "parent.attemptId"),
                ]
                .into_iter()
                .find_map(|(different, path)| different.then_some(path));
                if parent_mismatch.is_some() {
                    return parent_mismatch;
                }
            }
            _ => return Some("parent"),
        }

        (self.delegation_digest != supplied.delegation_digest)
            .then_some("executionBinding.delegationDigest")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn digest(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    fn root_value() -> serde_json::Value {
        json!({
            "contract": CONTRACT,
            "principalRef": "principal:operator",
            "familiarId": "sage",
            "familiarSnapshotDigest": digest('a'),
            "projectDigest": digest('b'),
            "graphId": "graph-1",
            "nodeId": "node-1",
            "attemptId": "attempt-1",
            "requestDigest": digest('c'),
            "policyRevision": "policy:7",
            "expiresAt": "2099-01-01T00:00:00Z",
            "parent": null,
            "delegationDigest": null
        })
    }

    fn parent_value() -> serde_json::Value {
        json!({
            "sessionId": "parent-session",
            "graphId": "graph-parent",
            "nodeId": "node-parent",
            "attemptId": "attempt-parent"
        })
    }

    fn child_value() -> serde_json::Value {
        let mut value = root_value();
        value["parent"] = parent_value();
        value["delegationDigest"] = json!(digest('d'));
        value
    }

    // --- Positive controls -------------------------------------------------

    #[test]
    fn parses_the_exact_root_shape() {
        let binding = parse(&root_value()).expect("root binding should parse");
        assert_eq!(binding.contract, CONTRACT);
        assert_eq!(binding.principal_ref, "principal:operator");
        assert_eq!(binding.familiar_id, "sage");
        assert_eq!(binding.familiar_snapshot_digest, digest('a'));
        assert_eq!(binding.project_digest, digest('b'));
        assert_eq!(binding.graph_id, "graph-1");
        assert_eq!(binding.node_id, "node-1");
        assert_eq!(binding.attempt_id, "attempt-1");
        assert_eq!(binding.request_digest, digest('c'));
        assert_eq!(binding.policy_revision, "policy:7");
        assert_eq!(binding.expires_at, "2099-01-01T00:00:00Z");
        assert_eq!(binding.parent, None);
        assert_eq!(binding.delegation_digest, None);
    }

    #[test]
    fn parses_the_exact_child_shape() {
        let binding = parse(&child_value()).expect("child binding should parse");
        let parent = binding.parent.expect("parent should be present");
        assert_eq!(parent.session_id, "parent-session");
        assert_eq!(parent.graph_id, "graph-parent");
        assert_eq!(parent.node_id, "node-parent");
        assert_eq!(parent.attempt_id, "attempt-parent");
        assert_eq!(binding.delegation_digest, Some(digest('d')));
    }

    #[test]
    fn serialized_field_order_is_stable() {
        let binding = parse(&root_value()).expect("root binding should parse");
        let expected = format!(
            "{{\"contract\":\"{contract}\",\"principalRef\":\"principal:operator\",\
\"familiarId\":\"sage\",\"familiarSnapshotDigest\":\"{a}\",\"projectDigest\":\"{b}\",\
\"graphId\":\"graph-1\",\"nodeId\":\"node-1\",\"attemptId\":\"attempt-1\",\
\"requestDigest\":\"{c}\",\"policyRevision\":\"policy:7\",\
\"expiresAt\":\"2099-01-01T00:00:00Z\",\"parent\":null,\"delegationDigest\":null}}",
            contract = CONTRACT,
            a = digest('a'),
            b = digest('b'),
            c = digest('c'),
        );
        assert_eq!(serde_json::to_string(&binding).unwrap(), expected);
    }

    // --- Unknown/missing members --------------------------------------------

    #[test]
    fn rejects_unknown_field_at_root() {
        let mut value = root_value();
        value["extra"] = json!(true);
        assert_eq!(
            parse(&value),
            Err(ValidationError::Invalid {
                path: "executionBinding"
            })
        );
    }

    #[test]
    fn rejects_missing_field_at_root_for_every_field() {
        for field in BINDING_FIELDS {
            let mut value = root_value();
            value.as_object_mut().unwrap().remove(field);
            assert_eq!(
                parse(&value),
                Err(ValidationError::Missing {
                    path: "executionBinding"
                }),
                "expected missing-field rejection when {field} is absent"
            );
        }
    }

    #[test]
    fn rejects_unknown_field_at_parent() {
        let mut value = root_value();
        let mut parent = parent_value();
        parent["extra"] = json!(true);
        value["parent"] = parent;
        assert_eq!(
            parse(&value),
            Err(ValidationError::Invalid {
                path: "executionBinding.parent"
            })
        );
    }

    #[test]
    fn rejects_missing_field_at_parent_for_every_field() {
        for field in PARENT_FIELDS {
            let mut value = root_value();
            let mut parent = parent_value();
            parent.as_object_mut().unwrap().remove(field);
            value["parent"] = parent;
            assert_eq!(
                parse(&value),
                Err(ValidationError::Missing {
                    path: "executionBinding.parent"
                }),
                "expected missing-field rejection when parent.{field} is absent"
            );
        }
    }

    #[test]
    fn rejects_non_object_root_and_parent() {
        assert_eq!(
            parse(&json!([1, 2, 3])),
            Err(ValidationError::Invalid {
                path: "executionBinding"
            })
        );

        let mut value = root_value();
        value["parent"] = json!("not-an-object");
        assert_eq!(
            parse(&value),
            Err(ValidationError::Invalid {
                path: "executionBinding.parent"
            })
        );
    }

    // --- Unsupported contract ------------------------------------------------

    #[test]
    fn rejects_unsupported_contract() {
        let mut value = root_value();
        value["contract"] = json!("psyche.execution_binding.v2");
        assert_eq!(
            parse(&value),
            Err(ValidationError::Unsupported {
                path: "executionBinding.contract"
            })
        );
    }

    // --- Opaque syntax class and boundaries ---------------------------------

    #[test]
    fn accepts_opaque_boundary_lengths() {
        for field in [
            "principalRef",
            "familiarId",
            "graphId",
            "nodeId",
            "attemptId",
            "policyRevision",
        ] {
            let mut value = root_value();
            value[field] = json!("a");
            assert!(parse(&value).is_ok(), "1-byte {field} should be accepted");

            let mut value = root_value();
            value[field] = json!("x".repeat(255));
            assert!(parse(&value).is_ok(), "255-byte {field} should be accepted");
        }
    }

    #[test]
    fn rejects_opaque_out_of_bounds_lengths() {
        for (field, path) in [
            ("principalRef", "executionBinding.principalRef"),
            ("familiarId", "executionBinding.familiarId"),
            ("graphId", "executionBinding.graphId"),
            ("nodeId", "executionBinding.nodeId"),
            ("attemptId", "executionBinding.attemptId"),
            ("policyRevision", "executionBinding.policyRevision"),
        ] {
            let mut value = root_value();
            value[field] = json!("");
            assert_eq!(
                parse(&value),
                Err(ValidationError::Invalid { path }),
                "empty {field} should be rejected at {path}"
            );

            let mut value = root_value();
            value[field] = json!("x".repeat(256));
            assert_eq!(
                parse(&value),
                Err(ValidationError::Invalid { path }),
                "256-byte {field} should be rejected at {path}"
            );
        }
    }

    #[test]
    fn rejects_opaque_values_with_disallowed_characters() {
        for (field, path) in [
            ("principalRef", "executionBinding.principalRef"),
            ("familiarId", "executionBinding.familiarId"),
            ("graphId", "executionBinding.graphId"),
            ("nodeId", "executionBinding.nodeId"),
            ("attemptId", "executionBinding.attemptId"),
            ("policyRevision", "executionBinding.policyRevision"),
        ] {
            for invalid in [
                " leading",
                "trailing ",
                "snowman-\u{2603}",
                "has space",
                "new\nline",
            ] {
                let mut value = root_value();
                value[field] = json!(invalid);
                assert_eq!(
                    parse(&value),
                    Err(ValidationError::Invalid { path }),
                    "{field}={invalid:?} should be rejected at {path}"
                );
            }
        }
    }

    #[test]
    fn accepts_every_allowed_opaque_character() {
        let mut value = root_value();
        value["graphId"] = json!("Az09._:/-");
        assert!(parse(&value).is_ok());
    }

    #[test]
    fn rejects_opaque_parent_fields_out_of_bounds_and_disallowed() {
        for (field, path) in [
            ("sessionId", "executionBinding.parent.sessionId"),
            ("graphId", "executionBinding.parent.graphId"),
            ("nodeId", "executionBinding.parent.nodeId"),
            ("attemptId", "executionBinding.parent.attemptId"),
        ] {
            let mut value = root_value();
            let mut parent = parent_value();
            parent[field] = json!("");
            value["parent"] = parent;
            assert_eq!(
                parse(&value),
                Err(ValidationError::Invalid { path }),
                "empty parent.{field} should be rejected at {path}"
            );

            let mut value = root_value();
            let mut parent = parent_value();
            parent[field] = json!("bad space");
            value["parent"] = parent;
            assert_eq!(
                parse(&value),
                Err(ValidationError::Invalid { path }),
                "parent.{field} with disallowed character should be rejected at {path}"
            );
        }
    }

    // --- Digest syntax class -------------------------------------------------

    #[test]
    fn accepts_valid_digests_on_every_digest_field() {
        for field in ["familiarSnapshotDigest", "projectDigest", "requestDigest"] {
            let mut value = root_value();
            value[field] = json!(digest('f'));
            assert!(
                parse(&value).is_ok(),
                "{field} should accept a valid digest"
            );
        }

        let mut value = child_value();
        value["delegationDigest"] = json!(digest('0'));
        assert!(parse(&value).is_ok());
    }

    #[test]
    fn rejects_malformed_digests_on_every_digest_field() {
        let malformed = [
            format!("sha256:{}", "A".repeat(64)), // uppercase hex
            "sha256:1234".to_string(),            // too short
            format!("sha512:{}", "a".repeat(64)), // wrong prefix
            format!("sha256:{}", "g".repeat(64)), // non-hex character
            format!("sha256:{}", "a".repeat(65)), // too long
            "a".repeat(71),                       // missing prefix entirely
        ];

        for (field, path) in [
            (
                "familiarSnapshotDigest",
                "executionBinding.familiarSnapshotDigest",
            ),
            ("projectDigest", "executionBinding.projectDigest"),
            ("requestDigest", "executionBinding.requestDigest"),
        ] {
            for invalid in &malformed {
                let mut value = root_value();
                value[field] = json!(invalid);
                assert_eq!(
                    parse(&value),
                    Err(ValidationError::Invalid { path }),
                    "{field}={invalid:?} should be rejected at {path}"
                );
            }
        }

        for invalid in &malformed {
            let mut value = child_value();
            value["delegationDigest"] = json!(invalid);
            assert_eq!(
                parse(&value),
                Err(ValidationError::Invalid {
                    path: "executionBinding.delegationDigest"
                }),
                "delegationDigest={invalid:?} should be rejected"
            );
        }
    }

    #[test]
    fn accepts_null_delegation_digest_but_rejects_null_required_digest() {
        // delegationDigest is nullable at the value level (null is valid)...
        assert!(parse(&root_value()).is_ok());

        // ...but the required digests must be strings, not null. Type
        // coercion fails during Serde deserialization itself (before
        // per-field syntax checks run), so the reported path is the whole
        // `executionBinding` object rather than the specific field.
        let mut value = root_value();
        value["familiarSnapshotDigest"] = json!(null);
        assert_eq!(
            parse(&value),
            Err(ValidationError::Invalid {
                path: "executionBinding"
            })
        );
    }

    // --- Expiry syntax and elapsed boundary ---------------------------------

    #[test]
    fn rejects_malformed_and_noncanonical_expiry() {
        for invalid in [
            "2099-01-01T00:00:00.000Z",
            "2099-01-01T00:00:00+00:00",
            "2099-01-01T00:00:00+01:00",
            "2099-01-01 00:00:00Z",
            "2099-01-01T00:00:00",
            "not-a-timestamp",
            "2099-01-01T00:00:00z",
        ] {
            let mut value = root_value();
            value["expiresAt"] = json!(invalid);
            assert_eq!(
                parse(&value),
                Err(ValidationError::Invalid {
                    path: "executionBinding.expiresAt"
                }),
                "expiresAt={invalid:?} should be rejected"
            );
        }
    }

    #[test]
    fn accepts_canonical_expiry() {
        let mut value = root_value();
        value["expiresAt"] = json!("2030-06-15T12:34:56Z");
        assert!(parse(&value).is_ok());
    }

    #[test]
    fn accepts_leap_second_expiry_but_rejects_seconds_above_sixty() {
        let mut value = root_value();
        value["expiresAt"] = json!("2030-06-15T12:34:60Z");
        assert!(parse(&value).is_ok());

        value["expiresAt"] = json!("2030-06-15T12:34:61Z");
        assert_eq!(
            parse(&value),
            Err(ValidationError::Invalid {
                path: "executionBinding.expiresAt"
            })
        );
    }

    #[test]
    fn expiry_is_elapsed_at_and_before_the_boundary_instant() {
        let mut value = root_value();
        value["expiresAt"] = json!("2030-06-15T12:00:00Z");
        let binding = parse(&value).expect("should parse");
        let boundary = Utc.with_ymd_and_hms(2030, 6, 15, 12, 0, 0).unwrap();

        assert_eq!(
            binding.validate_not_expired(boundary),
            Err(ValidationError::Expired {
                path: "executionBinding.expiresAt"
            }),
            "exact boundary instant should be elapsed"
        );
        assert_eq!(
            binding.validate_not_expired(boundary + chrono::Duration::seconds(1)),
            Err(ValidationError::Expired {
                path: "executionBinding.expiresAt"
            }),
            "an instant after expiry should be elapsed"
        );
        assert!(
            binding
                .validate_not_expired(boundary - chrono::Duration::seconds(1))
                .is_ok(),
            "an instant before expiry should not be elapsed"
        );
    }

    // --- No normalization ----------------------------------------------------

    #[test]
    fn preserves_mixed_case_values_byte_exact_without_normalization() {
        let mut value = root_value();
        value["principalRef"] = json!("Principal:MixedCase-ID_9");
        value["familiarSnapshotDigest"] = json!(format!("sha256:{}", "a".repeat(64)));
        let binding = parse(&value).expect("should parse");
        assert_eq!(binding.principal_ref, "Principal:MixedCase-ID_9");
        assert_eq!(
            serde_json::to_value(&binding).unwrap()["principalRef"],
            json!("Principal:MixedCase-ID_9")
        );
    }

    // --- Deterministic mismatch path -----------------------------------------

    #[test]
    fn first_mismatch_path_is_none_for_identical_bindings() {
        let binding = parse(&child_value()).expect("should parse");
        assert_eq!(binding.first_mismatch_path(&binding.clone()), None);
    }

    #[test]
    fn first_mismatch_path_reports_every_top_level_field_in_order() {
        type Mutator = Box<dyn Fn(&mut ExecutionBinding)>;
        let expected = parse(&root_value()).expect("should parse");
        // Mutate the parsed struct directly (rather than re-parsing mutated
        // JSON) so a deliberately mismatched `contract` can be compared
        // without tripping the separate `Unsupported` parse-time check.
        let cases: [(&str, Mutator); 11] = [
            (
                "contract",
                Box::new(|b| b.contract = "psyche.execution_binding.v2".to_string()),
            ),
            (
                "principalRef",
                Box::new(|b| b.principal_ref = "principal:other".to_string()),
            ),
            (
                "familiarId",
                Box::new(|b| b.familiar_id = "other-familiar".to_string()),
            ),
            (
                "familiarSnapshotDigest",
                Box::new(|b| b.familiar_snapshot_digest = digest('e')),
            ),
            (
                "projectDigest",
                Box::new(|b| b.project_digest = digest('e')),
            ),
            ("graphId", Box::new(|b| b.graph_id = "graph-2".to_string())),
            ("nodeId", Box::new(|b| b.node_id = "node-2".to_string())),
            (
                "attemptId",
                Box::new(|b| b.attempt_id = "attempt-2".to_string()),
            ),
            (
                "requestDigest",
                Box::new(|b| b.request_digest = digest('e')),
            ),
            (
                "policyRevision",
                Box::new(|b| b.policy_revision = "policy:8".to_string()),
            ),
            (
                "expiresAt",
                Box::new(|b| b.expires_at = "2100-01-01T00:00:00Z".to_string()),
            ),
        ];
        for (field, mutate) in cases {
            let mut supplied = expected.clone();
            mutate(&mut supplied);
            let expected_path = format!("executionBinding.{field}");
            assert_eq!(
                expected.first_mismatch_path(&supplied),
                Some(expected_path.as_str()),
                "expected mismatch reported at {field}"
            );
        }
    }

    #[test]
    fn first_mismatch_path_reports_parent_fields_after_top_level_fields() {
        let expected = parse(&child_value()).expect("should parse");
        let cases = [
            ("sessionId", "parent.sessionId"),
            ("graphId", "parent.graphId"),
            ("nodeId", "parent.nodeId"),
            ("attemptId", "parent.attemptId"),
        ];
        for (field, expected_path) in cases {
            let mut mutated_value = child_value();
            mutated_value["parent"][field] = json!("mismatched-value");
            let supplied = parse(&mutated_value).expect("mutated value should still parse");
            assert_eq!(
                expected.first_mismatch_path(&supplied),
                Some(expected_path),
                "expected mismatch reported at {expected_path}"
            );
        }
    }

    #[test]
    fn first_mismatch_path_reports_presence_mismatch_of_parent() {
        let with_parent = parse(&child_value()).expect("should parse");
        let mut without_parent_value = child_value();
        without_parent_value["parent"] = json!(null);
        without_parent_value["delegationDigest"] = json!(null);
        let without_parent = parse(&without_parent_value).expect("should parse");

        assert_eq!(
            with_parent.first_mismatch_path(&without_parent),
            Some("parent")
        );
        assert_eq!(
            without_parent.first_mismatch_path(&with_parent),
            Some("parent")
        );
    }

    #[test]
    fn first_mismatch_path_reports_delegation_digest_last() {
        let expected = parse(&child_value()).expect("should parse");
        let mut mutated_value = child_value();
        mutated_value["delegationDigest"] = json!(digest('9'));
        let supplied = parse(&mutated_value).expect("should parse");
        assert_eq!(
            expected.first_mismatch_path(&supplied),
            Some("executionBinding.delegationDigest")
        );
    }

    #[test]
    fn first_mismatch_path_prefers_earliest_field_when_several_differ() {
        let expected = parse(&child_value()).expect("should parse");
        let mut mutated_value = child_value();
        mutated_value["familiarId"] = json!("other-familiar");
        mutated_value["parent"]["nodeId"] = json!("other-node");
        mutated_value["delegationDigest"] = json!(digest('9'));
        let supplied = parse(&mutated_value).expect("should parse");
        assert_eq!(
            expected.first_mismatch_path(&supplied),
            Some("executionBinding.familiarId")
        );
    }
}
