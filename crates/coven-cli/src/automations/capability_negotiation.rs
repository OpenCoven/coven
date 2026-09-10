use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::definition::RoutineDefinition;

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

#[derive(Debug, Clone, PartialEq)]
pub enum DefinitionNegotiation {
    Supported(Box<RoutineDefinition>),
    Unsupported(UnsupportedVariant),
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
        if let Some(retryable_classes) = policies
            .get("retry")
            .and_then(Value::as_object)
            .and_then(|retry| retry.get("retryableClasses"))
            .and_then(Value::as_array)
            .filter(|classes| classes.iter().all(Value::is_string))
        {
            for retryable_class in retryable_classes.iter().filter_map(Value::as_str) {
                if !matches!(
                    retryable_class,
                    "transient_dispatch" | "lease_expired" | "runtime_unavailable"
                ) {
                    return unsupported_from_component(
                        "retry.safe-classes",
                        retryable_class,
                        false,
                    );
                }
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
        if let Some(retention) = policies.get("retention").and_then(Value::as_object) {
            for field in ["occurrenceHistory", "runLogs", "receipts"] {
                if let Some(classification) = retention
                    .get(field)
                    .and_then(Value::as_object)
                    .and_then(|retention_class| retention_class.get("classification"))
                    .and_then(Value::as_str)
                {
                    if classification != "standard" {
                        return unsupported_from_component("retention", classification, false);
                    }
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

pub fn negotiate_definition(definition: &Value) -> Result<DefinitionNegotiation, String> {
    let Some(unsupported) = preflight_definition(definition) else {
        return RoutineDefinition::from_json(definition)
            .and_then(RoutineDefinition::resolve_timezone_for_persistence)
            .map(Box::new)
            .map(DefinitionNegotiation::Supported);
    };

    let projection = validation_projection(definition);
    RoutineDefinition::from_json(&projection)
        .and_then(RoutineDefinition::resolve_timezone_for_persistence)?;
    Ok(DefinitionNegotiation::Unsupported(unsupported))
}

fn validation_projection(definition: &Value) -> Value {
    let Value::Object(mut projection) = definition.clone() else {
        return definition.clone();
    };

    neutralize_flat_unsupported_values(&mut projection);
    for key in ["trigger", "conditions", "action", "policies"] {
        if projection
            .get(key)
            .is_some_and(|value| rich_hint_is_well_formed(key, value))
        {
            projection.remove(key);
        }
    }

    Value::Object(projection)
}

fn neutralize_flat_unsupported_values(definition: &mut Map<String, Value>) {
    if definition
        .get("misfire")
        .and_then(Value::as_str)
        .is_some_and(|misfire| misfire != "latest")
    {
        definition.insert("misfire".to_owned(), Value::String("latest".to_owned()));
    }
    if definition
        .get("overlap")
        .and_then(Value::as_str)
        .is_some_and(|overlap| overlap != "forbid")
    {
        definition.insert("overlap".to_owned(), Value::String("forbid".to_owned()));
    }
    if let Some(retry) = definition.get_mut("retry").and_then(Value::as_object_mut) {
        if retry
            .get("backoffPolicy")
            .and_then(Value::as_str)
            .is_some_and(|backoff| !matches!(backoff, "none" | "fixed" | "exponential"))
        {
            retry.insert("backoffPolicy".to_owned(), Value::String("none".to_owned()));
        }
    }
    if definition.get("outputTarget").is_some_and(Value::is_string) {
        definition.remove("outputTarget");
    }
    if let Some(rrule) = definition.get("rrule").and_then(Value::as_str) {
        if unsupported_rrule_frequency(rrule).is_some() {
            let frequency = if rrule.split(';').any(|part| {
                part.split_once('=')
                    .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case("BYDAY"))
            }) {
                "WEEKLY"
            } else {
                "DAILY"
            };
            definition.insert(
                "rrule".to_owned(),
                Value::String(neutralize_rrule_frequency(rrule, frequency)),
            );
        }
    }
}

fn neutralize_rrule_frequency(rrule: &str, supported_frequency: &str) -> String {
    rrule
        .split(';')
        .map(|part| match part.split_once('=') {
            Some((key, _)) if key.trim().eq_ignore_ascii_case("FREQ") => {
                format!("{key}={supported_frequency}")
            }
            _ => part.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn rich_hint_is_well_formed(key: &str, value: &Value) -> bool {
    match key {
        "trigger" => trigger_hint_is_well_formed(value),
        "conditions" => conditions_hint_is_well_formed(value),
        "action" => action_hint_is_well_formed(value),
        "policies" => policies_hint_is_well_formed(value),
        _ => false,
    }
}

fn trigger_hint_is_well_formed(value: &Value) -> bool {
    let Some(trigger) = value.as_object() else {
        return false;
    };
    if !has_only_fields(trigger, &["variant", "version", "schedule"]) {
        return false;
    }
    let Some(variant) = trigger.get("variant").and_then(Value::as_str) else {
        return false;
    };
    if variant.trim().is_empty()
        || !optional_version_is_well_formed(trigger.get("version"))
        || !optional_schedule_is_well_formed(trigger.get("schedule"))
    {
        return false;
    }
    true
}

fn optional_schedule_is_well_formed(schedule: Option<&Value>) -> bool {
    schedule.is_none_or(schedule_is_well_formed)
}

fn schedule_is_well_formed(value: &Value) -> bool {
    let Some(schedule) = value.as_object() else {
        return false;
    };
    has_only_fields(schedule, &["rrule", "timezone"])
        && schedule.get("rrule").is_some_and(Value::is_string)
        && schedule.get("timezone").is_some_and(Value::is_string)
}

fn conditions_hint_is_well_formed(value: &Value) -> bool {
    value.as_array().is_some_and(|conditions| {
        conditions.iter().all(|condition| {
            let Some(condition) = condition.as_object() else {
                return false;
            };
            has_only_fields(condition, &["variant", "version"])
                && condition
                    .get("variant")
                    .and_then(Value::as_str)
                    .is_some_and(|variant| !variant.trim().is_empty())
                && optional_version_is_well_formed(condition.get("version"))
        })
    })
}

fn action_hint_is_well_formed(value: &Value) -> bool {
    let Some(action) = value.as_object() else {
        return false;
    };
    if !has_only_fields(action, &["variant", "version", "prompt", "cwd"]) {
        return false;
    }
    let Some(variant) = action.get("variant").and_then(Value::as_str) else {
        return false;
    };
    if variant.trim().is_empty()
        || !optional_version_is_well_formed(action.get("version"))
        || !optional_string_is_well_formed(action.get("prompt"))
        || !optional_string_is_well_formed(action.get("cwd"))
    {
        return false;
    }
    true
}

fn policies_hint_is_well_formed(value: &Value) -> bool {
    let Some(policies) = value.as_object() else {
        return false;
    };
    !policies.is_empty()
        && has_only_fields(
            policies,
            &[
                "timeout",
                "retry",
                "concurrency",
                "misfire",
                "delivery",
                "retention",
            ],
        )
        && policies.iter().all(|(key, value)| match key.as_str() {
            "timeout" => timeout_hint_is_well_formed(value),
            "retry" => retry_hint_is_well_formed(value),
            "concurrency" => single_string_field_is_well_formed(value, "overlap"),
            "misfire" => single_string_field_is_well_formed(value, "disposition"),
            "delivery" => delivery_hint_is_well_formed(value),
            "retention" => retention_hint_is_well_formed(value),
            _ => false,
        })
}

fn timeout_hint_is_well_formed(value: &Value) -> bool {
    let Some(timeout) = value.as_object() else {
        return false;
    };
    has_only_fields(timeout, &["perRunMinutes"])
        && timeout
            .get("perRunMinutes")
            .and_then(Value::as_u64)
            .is_some_and(|minutes| (1..=44_640).contains(&minutes))
}

fn retry_hint_is_well_formed(value: &Value) -> bool {
    let Some(retry) = value.as_object() else {
        return false;
    };
    if retry.is_empty()
        || !has_only_fields(
            retry,
            &[
                "maxAttempts",
                "backoffPolicy",
                "backoffSeconds",
                "retryableClasses",
            ],
        )
        || !retry.get("maxAttempts").is_none_or(|value| {
            value
                .as_u64()
                .is_some_and(|attempts| (1..=10).contains(&attempts))
        })
        || !optional_string_is_well_formed(retry.get("backoffPolicy"))
        || !retry.get("backoffSeconds").is_none_or(|value| {
            value
                .as_u64()
                .is_some_and(|seconds| (1..=86_400).contains(&seconds))
        })
    {
        return false;
    }
    let Some(retryable_classes) = retry.get("retryableClasses") else {
        return true;
    };
    let Some(retryable_classes) = retryable_classes.as_array() else {
        return false;
    };
    let mut unique = BTreeSet::new();
    retryable_classes.iter().all(|value| {
        value
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .is_some_and(|value| unique.insert(value))
    })
}

fn delivery_hint_is_well_formed(value: &Value) -> bool {
    let Some(delivery) = value.as_object() else {
        return false;
    };
    has_only_fields(delivery, &["outputTarget", "mode"])
        && delivery.get("outputTarget").is_some_and(Value::is_string)
        && delivery.get("mode").is_some_and(Value::is_string)
}

fn retention_hint_is_well_formed(value: &Value) -> bool {
    let Some(retention) = value.as_object() else {
        return false;
    };
    !retention.is_empty()
        && has_only_fields(retention, &["occurrenceHistory", "runLogs", "receipts"])
        && retention.values().all(retention_class_is_well_formed)
}

fn retention_class_is_well_formed(value: &Value) -> bool {
    let Some(retention_class) = value.as_object() else {
        return false;
    };
    has_only_fields(retention_class, &["classification", "deleteAfter"])
        && retention_class
            .get("classification")
            .and_then(Value::as_str)
            .is_some_and(|classification| !classification.trim().is_empty())
        && optional_string_is_well_formed(retention_class.get("deleteAfter"))
}

fn single_string_field_is_well_formed(value: &Value, field: &str) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    has_only_fields(object, &[field]) && object.get(field).is_some_and(Value::is_string)
}

fn has_only_fields(object: &Map<String, Value>, allowed: &[&str]) -> bool {
    object.keys().all(|key| allowed.contains(&key.as_str()))
}

fn optional_version_is_well_formed(version: Option<&Value>) -> bool {
    version.is_none_or(is_version_one)
}

fn is_version_one(value: &Value) -> bool {
    value.as_u64() == Some(1)
}

fn optional_string_is_well_formed(value: Option<&Value>) -> bool {
    value.is_none_or(Value::is_string)
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
    fn capability_negotiation_classifies_unknown_retryable_classes() {
        for (retryable_class, expected) in [
            ("ambiguous", "retry.safe-classes.ambiguous"),
            ("operator_required", "retry.safe-classes.operator_required"),
        ] {
            assert_variant(
                json!({"policies": {"retry": {
                    "retryableClasses": ["transient_dispatch", retryable_class]
                }}}),
                expected,
            );
        }
    }

    #[test]
    fn capability_negotiation_classifies_unsupported_retention_classes() {
        for (field, classification, expected) in [
            ("occurrenceHistory", "ephemeral", "retention.ephemeral"),
            ("runLogs", "extended", "retention.extended"),
            ("receipts", "forever", "retention.forever"),
        ] {
            assert_variant(
                json!({"policies": {"retention": {
                    field: {"classification": classification}
                }}}),
                expected,
            );
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
            json!({"policies": {"retry": {"retryableClasses": "runtime_unavailable"}}}),
            json!({"policies": {"retry": {"retryableClasses": [1]}}}),
            json!({"policies": {"retention": {"occurrenceHistory": "standard"}}}),
            json!({"policies": {"retention": {
                "occurrenceHistory": {"classification": 1}
            }}}),
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
                "retry": {
                    "backoffPolicy": "fixed",
                    "retryableClasses": [
                        "transient_dispatch",
                        "lease_expired",
                        "runtime_unavailable"
                    ]
                },
                "retention": {
                    "occurrenceHistory": {"classification": "standard"},
                    "runLogs": {"classification": "standard"},
                    "receipts": {"classification": "standard"}
                }
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

        let private_retry_class = format!("secret-{}", "x".repeat(500));
        let unsupported = preflight_definition(&json!({"policies": {"retry": {
            "retryableClasses": [private_retry_class]
        }}}))
        .unwrap();

        assert_eq!(unsupported.variant, "retry.safe-classes.unknown");
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

    #[test]
    fn capability_negotiation_does_not_refuse_advertised_supported_variants() {
        let profile = capability_profile();

        for supported in &profile.supported.triggers {
            assert_eq!(
                preflight_definition(&json!({"trigger": {"variant": supported.variant}})),
                None,
                "advertised trigger `{}` was refused",
                supported.variant
            );
        }
        for supported in &profile.supported.conditions {
            assert_eq!(
                preflight_definition(&json!({"conditions": [{"variant": supported.variant}]})),
                None,
                "advertised condition `{}` was refused",
                supported.variant
            );
        }
        for supported in &profile.supported.actions {
            assert_eq!(
                preflight_definition(&json!({"action": {"variant": supported.variant}})),
                None,
                "advertised action `{}` was refused",
                supported.variant
            );
        }
        for supported in &profile.supported.trigger_policies {
            let definition = match supported.variant.as_str() {
                "misfire.latest" => json!({"misfire": "latest"}),
                "overlap.forbid" => json!({"overlap": "forbid"}),
                variant if variant.starts_with("retry.backoff.") => {
                    let value = variant.trim_start_matches("retry.backoff.");
                    json!({"retry": {"backoffPolicy": value}})
                }
                "timezone.utc" => json!({"trigger": {
                    "variant": "schedule",
                    "schedule": {"timezone": "utc"}
                }}),
                "timezone.iana" => json!({"trigger": {
                    "variant": "schedule",
                    "schedule": {"timezone": "America/Chicago"}
                }}),
                "timeout.required"
                | "dst.gap.skip"
                | "dst.fold.first"
                | "retry.attempts"
                | "retry.safe-classes"
                | "retry.exhaustion-quarantine" => continue,
                variant => panic!("missing preflight fixture for supported policy `{variant}`"),
            };
            assert_eq!(
                preflight_definition(&definition),
                None,
                "advertised policy `{}` was refused",
                supported.variant
            );
        }
        for supported in &profile.supported.delivery_policies {
            let mode = supported
                .variant
                .strip_prefix("outputTarget.")
                .unwrap_or_else(|| {
                    panic!(
                        "missing preflight fixture for supported delivery policy `{}`",
                        supported.variant
                    )
                });
            assert_eq!(
                preflight_definition(&json!({"policies": {"delivery": {
                    "outputTarget": "result.md",
                    "mode": mode
                }}})),
                None,
                "advertised delivery policy `{}` was refused",
                supported.variant
            );
        }
    }
}
