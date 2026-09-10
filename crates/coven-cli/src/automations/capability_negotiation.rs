use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

const CAPABILITIES_JSON: &str =
    include_str!("../../../../spec/coven-automations/v1/capabilities.json");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityProfile {
    pub version: u32,
    pub contract_profile: String,
    pub description: String,
    pub supported: SupportedVariants,
    pub experimental: Vec<VariantCapability>,
    pub refused: Vec<RefusedVariant>,
    pub negotiation_rules: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupportedVariants {
    pub triggers: Vec<VariantCapability>,
    pub conditions: Vec<VariantCapability>,
    pub actions: Vec<VariantCapability>,
    pub trigger_policies: Vec<VariantCapability>,
    pub delivery_policies: Vec<VariantCapability>,
    pub retention_policies: Vec<VariantCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VariantCapability {
    pub variant: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefusedVariant {
    pub variant: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedVariant {
    pub variant: String,
    pub reason: String,
}

pub fn capability_profile() -> &'static CapabilityProfile {
    static PROFILE: OnceLock<CapabilityProfile> = OnceLock::new();
    PROFILE.get_or_init(|| {
        serde_json::from_str(CAPABILITIES_JSON)
            .expect("packaged coven.automations.v1 capabilities must be valid")
    })
}

#[must_use]
pub fn preflight_definition(definition: &Value) -> Option<UnsupportedVariant> {
    let definition = definition.as_object()?;

    if let Some(trigger) = definition.get("trigger").and_then(Value::as_object) {
        if let Some(variant) = trigger.get("variant").and_then(Value::as_str) {
            if variant != "schedule" {
                return unsupported_from_component("trigger", variant, false);
            }
            if let Some(schedule) = trigger.get("schedule").and_then(Value::as_object) {
                if schedule.get("timezone").and_then(Value::as_str) == Some("local") {
                    return Some(unsupported("timezone.local.durable".to_owned()));
                }
                if let Some(rrule) = schedule.get("rrule").and_then(Value::as_str) {
                    if let Some(unsupported) = unsupported_rrule_frequency(rrule) {
                        return Some(unsupported);
                    }
                }
            }
        }
    }

    if let Some(conditions) = definition.get("conditions").and_then(Value::as_array) {
        for condition in conditions {
            if let Some(variant) = condition
                .as_object()
                .and_then(|condition| condition.get("variant"))
                .and_then(Value::as_str)
            {
                if let Some(unsupported) = unsupported_from_component("condition", variant, false) {
                    return Some(unsupported);
                }
            }
        }
    }

    if let Some(action) = definition.get("action").and_then(Value::as_object) {
        if let Some(variant) = action.get("variant").and_then(Value::as_str) {
            if variant != "familiarInvocation" {
                return unsupported_from_component("action", variant, false);
            }
        }
    }

    if let Some(policies) = definition.get("policies").and_then(Value::as_object) {
        if let Some(disposition) = policies
            .get("misfire")
            .and_then(Value::as_object)
            .and_then(|misfire| misfire.get("disposition"))
            .and_then(Value::as_str)
        {
            if disposition != "latest" {
                return unsupported_from_component("misfire", disposition, false);
            }
        }
        if let Some(overlap) = policies
            .get("concurrency")
            .and_then(Value::as_object)
            .and_then(|concurrency| concurrency.get("overlap"))
            .and_then(Value::as_str)
        {
            if overlap != "forbid" {
                return unsupported_from_component("overlap", overlap, false);
            }
        }
        if let Some(backoff) = policies
            .get("retry")
            .and_then(Value::as_object)
            .and_then(|retry| retry.get("backoffPolicy"))
            .and_then(Value::as_str)
        {
            if !matches!(backoff, "none" | "fixed" | "exponential") {
                return unsupported_from_component("retry.backoff", backoff, false);
            }
        }
        if let Some(delivery) = policies.get("delivery").and_then(Value::as_object) {
            if delivery
                .get("outputTarget")
                .and_then(Value::as_str)
                .is_some()
            {
                if let Some(mode) = delivery.get("mode").and_then(Value::as_str) {
                    return unsupported_from_component("outputTarget", mode, false);
                }
            }
        }
    }

    if let Some(misfire) = definition.get("misfire").and_then(Value::as_str) {
        if misfire != "latest" {
            return unsupported_from_component("misfire", misfire, false);
        }
    }
    if let Some(overlap) = definition.get("overlap").and_then(Value::as_str) {
        if overlap != "forbid" {
            return unsupported_from_component("overlap", overlap, false);
        }
    }
    if let Some(backoff) = definition
        .get("retry")
        .and_then(Value::as_object)
        .and_then(|retry| retry.get("backoffPolicy"))
        .and_then(Value::as_str)
    {
        if !matches!(backoff, "none" | "fixed" | "exponential") {
            return unsupported_from_component("retry.backoff", backoff, false);
        }
    }
    if definition
        .get("outputTarget")
        .and_then(Value::as_str)
        .is_some()
    {
        return Some(unsupported("outputTarget.atomic".to_owned()));
    }
    if let Some(rrule) = definition.get("rrule").and_then(Value::as_str) {
        if let Some(unsupported) = unsupported_rrule_frequency(rrule) {
            return Some(unsupported);
        }
    }

    None
}

fn unsupported_rrule_frequency(rrule: &str) -> Option<UnsupportedVariant> {
    let mut frequency = None;
    for part in rrule
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let (key, value) = part.split_once('=')?;
        if key.trim().eq_ignore_ascii_case("FREQ") {
            if frequency.is_some() {
                return None;
            }
            let value = value.trim();
            if value.is_empty() {
                return None;
            }
            frequency = Some(value);
        }
    }
    let frequency = frequency?;
    if frequency.eq_ignore_ascii_case("DAILY") || frequency.eq_ignore_ascii_case("WEEKLY") {
        return None;
    }
    unsupported_from_component("trigger.schedule.frequency", frequency, true)
}

fn unsupported_from_component(
    prefix: &str,
    value: &str,
    lowercase: bool,
) -> Option<UnsupportedVariant> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let component = if value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        if lowercase {
            value.to_ascii_lowercase()
        } else {
            value.to_owned()
        }
    } else {
        "unknown".to_owned()
    };
    Some(unsupported(format!("{prefix}.{component}")))
}

fn unsupported(variant: String) -> UnsupportedVariant {
    let profile = capability_profile();
    let reason = profile
        .refused
        .iter()
        .find(|refused| refused.variant == variant)
        .map(|refused| refused.reason.clone())
        .unwrap_or_else(|| {
            format!(
                "This variant is not supported by the {} capability profile.",
                profile.contract_profile
            )
        });
    UnsupportedVariant { variant, reason }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn assert_variant(definition: Value, expected: &str) {
        let unsupported = preflight_definition(&definition)
            .unwrap_or_else(|| panic!("expected unsupported variant `{expected}`"));
        assert_eq!(unsupported.variant, expected);
        assert!(!unsupported.reason.is_empty());
        assert!(unsupported.reason.len() <= 500);
    }

    #[test]
    fn capability_negotiation_classifies_flat_compatibility_variants() {
        for (definition, expected) in [
            (json!({"outputTarget": "result.md"}), "outputTarget.atomic"),
            (json!({"misfire": "backfill"}), "misfire.backfill"),
            (json!({"overlap": "allow"}), "overlap.allow"),
            (
                json!({"retry": {"backoffPolicy": "linear"}}),
                "retry.backoff.linear",
            ),
            (
                json!({"rrule": "FREQ=MONTHLY;BYHOUR=9"}),
                "trigger.schedule.frequency.monthly",
            ),
        ] {
            assert_variant(definition, expected);
        }
    }

    #[test]
    fn capability_negotiation_classifies_nested_variant_hints() {
        for (definition, expected) in [
            (
                json!({"trigger": {"variant": "webhook"}}),
                "trigger.webhook",
            ),
            (
                json!({"conditions": [{"variant": "branch"}]}),
                "condition.branch",
            ),
            (
                json!({"action": {"variant": "pipeline"}}),
                "action.pipeline",
            ),
            (
                json!({"trigger": {
                    "variant": "schedule",
                    "schedule": {"rrule": "FREQ=YEARLY", "timezone": "utc"}
                }}),
                "trigger.schedule.frequency.yearly",
            ),
            (
                json!({"trigger": {
                    "variant": "schedule",
                    "schedule": {"rrule": "FREQ=DAILY", "timezone": "local"}
                }}),
                "timezone.local.durable",
            ),
            (
                json!({"policies": {"misfire": {"disposition": "backfill"}}}),
                "misfire.backfill",
            ),
            (
                json!({"policies": {"concurrency": {"overlap": "parallel"}}}),
                "overlap.parallel",
            ),
            (
                json!({"policies": {"retry": {"backoffPolicy": "linear"}}}),
                "retry.backoff.linear",
            ),
            (
                json!({"policies": {
                    "delivery": {"outputTarget": "result.md", "mode": "atomic"}
                }}),
                "outputTarget.atomic",
            ),
            (
                json!({"policies": {
                    "delivery": {"outputTarget": "result.md", "mode": "stream"}
                }}),
                "outputTarget.stream",
            ),
        ] {
            assert_variant(definition, expected);
        }
    }

    #[test]
    fn capability_negotiation_leaves_malformed_types_for_validation() {
        for definition in [
            json!({"outputTarget": {"private": "value"}}),
            json!({"misfire": 1}),
            json!({"overlap": false}),
            json!({"retry": {"backoffPolicy": ["linear"]}}),
            json!({"rrule": 1}),
            json!({"rrule": "FREQ=DAILY;BYHOUR=not-a-number"}),
            json!({"trigger": {"variant": 1}}),
            json!({"conditions": {"variant": "branch"}}),
            json!({"conditions": [{"variant": 1}]}),
            json!({"action": {"variant": []}}),
            json!({"policies": {"misfire": "backfill"}}),
            json!({"policies": {"delivery": {"mode": "atomic"}}}),
        ] {
            assert_eq!(preflight_definition(&definition), None, "{definition}");
        }
    }

    #[test]
    fn capability_negotiation_does_not_refuse_supported_variant_hints() {
        let definition = json!({
            "rrule": "FREQ=WEEKLY;BYDAY=MO;BYHOUR=9",
            "misfire": "latest",
            "overlap": "forbid",
            "retry": {"backoffPolicy": "exponential"},
            "trigger": {
                "variant": "schedule",
                "schedule": {"rrule": "FREQ=DAILY", "timezone": "utc"}
            },
            "conditions": [],
            "action": {"variant": "familiarInvocation"},
            "policies": {
                "misfire": {"disposition": "latest"},
                "concurrency": {"overlap": "forbid"},
                "retry": {"backoffPolicy": "fixed"}
            }
        });

        assert_eq!(preflight_definition(&definition), None);
    }

    #[test]
    fn capability_negotiation_bounds_untrusted_variant_identifiers() {
        let private_value = format!("secret-{}", "x".repeat(500));
        let unsupported =
            preflight_definition(&json!({"trigger": {"variant": private_value}})).unwrap();

        assert_eq!(unsupported.variant, "trigger.unknown");
        assert!(!unsupported.reason.contains("secret"));
    }

    #[test]
    fn capability_negotiation_recognizes_every_expressible_refused_variant() {
        for refused in &capability_profile().refused {
            let definition = match refused.variant.as_str() {
                "trigger.webhook" => json!({"trigger": {"variant": "webhook"}}),
                "action.pipeline" => json!({"action": {"variant": "pipeline"}}),
                "misfire.backfill" => json!({"misfire": "backfill"}),
                "timezone.local.durable" => json!({"trigger": {
                    "variant": "schedule",
                    "schedule": {"rrule": "FREQ=DAILY", "timezone": "local"}
                }}),
                "outputTarget.atomic" => json!({"outputTarget": "result.md"}),
                variant => panic!("no preflight fixture for refused variant `{variant}`"),
            };

            let unsupported = preflight_definition(&definition)
                .unwrap_or_else(|| panic!("preflight missed `{}`", refused.variant));
            assert_eq!(unsupported.variant, refused.variant);
            assert_eq!(unsupported.reason, refused.reason);
        }
    }
}
