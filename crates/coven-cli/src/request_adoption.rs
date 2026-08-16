use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CONTRACT: &str = "psyche.request_adoption.v1";
const FIELDS: [&str; 3] = ["contract", "key", "requestDigest"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestAdoption {
    pub contract: String,
    pub key: String,
    pub request_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    Missing { path: &'static str },
    Invalid { path: &'static str },
    Unsupported { path: &'static str },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { path } => write!(f, "missing field at {path}"),
            Self::Invalid { path } => write!(f, "invalid field at {path}"),
            Self::Unsupported { path } => write!(f, "unsupported value at {path}"),
        }
    }
}

impl std::error::Error for ValidationError {}

pub fn parse(value: &Value) -> Result<RequestAdoption, ValidationError> {
    let object = value.as_object().ok_or(ValidationError::Invalid {
        path: "requestAdoption",
    })?;
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = FIELDS.into_iter().collect::<BTreeSet<_>>();
    if actual.difference(&expected).next().is_some() {
        return Err(ValidationError::Invalid {
            path: "requestAdoption",
        });
    }
    if expected.difference(&actual).next().is_some() {
        return Err(ValidationError::Missing {
            path: "requestAdoption",
        });
    }
    let string = |field: &str, invalid_path: &'static str| {
        object
            .get(field)
            .ok_or(ValidationError::Missing {
                path: "requestAdoption",
            })?
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or(ValidationError::Invalid { path: invalid_path })
    };
    let adoption = RequestAdoption {
        contract: string("contract", "requestAdoption.contract")?,
        key: string("key", "requestAdoption.key")?,
        request_digest: string("requestDigest", "requestAdoption.requestDigest")?,
    };
    adoption.validate()?;
    Ok(adoption)
}

impl RequestAdoption {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.contract != CONTRACT {
            return Err(ValidationError::Unsupported {
                path: "requestAdoption.contract",
            });
        }
        if !valid_key(&self.key) {
            return Err(ValidationError::Invalid {
                path: "requestAdoption.key",
            });
        }
        if !valid_digest(&self.request_digest) {
            return Err(ValidationError::Invalid {
                path: "requestAdoption.requestDigest",
            });
        }
        Ok(())
    }

    pub fn deterministic_json(&self) -> String {
        serde_json::to_string(self).expect("validated request adoption serializes")
    }
}

fn valid_key(value: &str) -> bool {
    (1..=255).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-')
        })
}

fn valid_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn digest(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    fn valid_value() -> Value {
        json!({
            "contract": CONTRACT,
            "key": "psyche:graph/node_attempt-1",
            "requestDigest": digest('a'),
        })
    }

    #[test]
    fn contract_constant_matches_o3_contract() {
        assert_eq!(CONTRACT, "psyche.request_adoption.v1");
    }

    #[test]
    fn parses_exact_three_member_object() {
        let parsed = parse(&valid_value()).expect("valid adoption");
        assert_eq!(
            parsed,
            RequestAdoption {
                contract: CONTRACT.to_owned(),
                key: "psyche:graph/node_attempt-1".to_owned(),
                request_digest: digest('a'),
            }
        );
    }

    #[test]
    fn rejects_missing_root_member_for_every_field() {
        for field in FIELDS {
            let mut value = valid_value();
            value.as_object_mut().unwrap().remove(field);
            assert_eq!(
                parse(&value),
                Err(ValidationError::Missing {
                    path: "requestAdoption",
                }),
                "missing {field} must fail",
            );
        }
    }

    #[test]
    fn rejects_unknown_root_members() {
        let mut value = valid_value();
        value["extra"] = json!(true);
        assert_eq!(
            parse(&value),
            Err(ValidationError::Invalid {
                path: "requestAdoption",
            })
        );
    }

    #[test]
    fn rejects_non_object_roots() {
        for value in [json!(null), json!(true), json!("nope"), json!([])] {
            assert_eq!(
                parse(&value),
                Err(ValidationError::Invalid {
                    path: "requestAdoption",
                })
            );
        }
    }

    #[test]
    fn key_length_validation_is_exact() {
        let cases = [
            ("", false),
            ("a", true),
            (&"a".repeat(255), true),
            (&"a".repeat(256), false),
        ];

        for (key, expected_ok) in cases {
            let mut value = valid_value();
            value["key"] = json!(key);
            let result = parse(&value);
            assert_eq!(
                result.is_ok(),
                expected_ok,
                "expected key length {} success to be {expected_ok}",
                key.len()
            );
            if !expected_ok {
                assert_eq!(
                    result,
                    Err(ValidationError::Invalid {
                        path: "requestAdoption.key",
                    })
                );
            }
        }
    }

    #[test]
    fn key_allows_every_supported_punctuation_character() {
        for punctuation in [".", "_", ":", "/", "-"] {
            let key = format!("Az09{punctuation}tail");
            let mut value = valid_value();
            value["key"] = json!(key.clone());
            let parsed = parse(&value).expect("allowed punctuation should parse");
            assert_eq!(parsed.key, key);
        }
    }

    #[test]
    fn key_rejects_whitespace_unicode_and_question_mark() {
        for key in ["has space", "tab\tkey", "snowman☃", "key?bad"] {
            let mut value = valid_value();
            value["key"] = json!(key);
            assert_eq!(
                parse(&value),
                Err(ValidationError::Invalid {
                    path: "requestAdoption.key",
                }),
                "expected key {key:?} to fail"
            );
        }
    }

    #[test]
    fn request_digest_rejects_uppercase_wrong_prefix_and_short_values() {
        let cases = [
            format!("sha256:{}", "A".repeat(64)),
            format!("sha512:{}", "a".repeat(64)),
            format!("sha256:{}", "a".repeat(63)),
        ];

        for digest in cases {
            let mut value = valid_value();
            value["requestDigest"] = json!(digest);
            assert_eq!(
                parse(&value),
                Err(ValidationError::Invalid {
                    path: "requestAdoption.requestDigest",
                })
            );
        }
    }

    #[test]
    fn rejects_unknown_contract() {
        let mut value = valid_value();
        value["contract"] = json!("psyche.request_adoption.v2");
        assert_eq!(
            parse(&value),
            Err(ValidationError::Unsupported {
                path: "requestAdoption.contract",
            })
        );
    }

    #[test]
    fn mixed_case_key_round_trips_byte_exact() {
        let key = "Alpha.beta_GAMMA:delta/Route-99";
        let mut value = valid_value();
        value["key"] = json!(key);
        let parsed = parse(&value).expect("mixed-case key should parse");
        assert_eq!(parsed.key, key);
        assert_eq!(parsed.deterministic_json(), value.to_string());
    }

    #[test]
    fn deterministic_serialization_is_byte_exact() {
        let adoption = parse(&valid_value()).expect("valid adoption");
        assert_eq!(
            adoption.deterministic_json(),
            format!(
                "{{\"contract\":\"{contract}\",\"key\":\"psyche:graph/node_attempt-1\",\"requestDigest\":\"{digest}\"}}",
                contract = CONTRACT,
                digest = digest('a'),
            )
        );
    }

    #[test]
    fn invalid_member_types_report_static_error_paths() {
        let cases = [
            (
                "contract",
                json!(7),
                ValidationError::Invalid {
                    path: "requestAdoption.contract",
                },
            ),
            (
                "key",
                json!(7),
                ValidationError::Invalid {
                    path: "requestAdoption.key",
                },
            ),
            (
                "requestDigest",
                json!(7),
                ValidationError::Invalid {
                    path: "requestAdoption.requestDigest",
                },
            ),
        ];

        for (field, replacement, expected) in cases {
            let mut value = valid_value();
            value[field] = replacement;
            assert_eq!(parse(&value), Err(expected), "expected {field} path");
        }
    }
}
