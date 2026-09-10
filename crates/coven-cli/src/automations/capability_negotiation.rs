use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::contract::types::{
    FamiliarInvocationAction, RetryableClass, TimeoutPolicy, Timestamp, WorkingDirectory,
};
use super::definition::{RoutineDefinition, RoutineTimezone};
use super::rrule::parse_rrule;

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
    preflight_definition_with_profile(definition, capability_profile())
}

fn preflight_definition_with_profile(
    definition: &Value,
    profile: &CapabilityProfile,
) -> Option<UnsupportedVariant> {
    let definition = definition.as_object()?;

    if let Some(trigger) = definition.get("trigger").and_then(Value::as_object) {
        if let Some(variant) = trigger.get("variant").and_then(Value::as_str) {
            if !supports(&profile.supported.triggers, variant) {
                return unsupported_from_component(profile, "trigger", variant, false);
            }
            if variant == "schedule" {
                if let Some(schedule) = trigger.get("schedule").and_then(Value::as_object) {
                    if let Some(timezone) =
                        schedule.get("timezone").and_then(schedule_timezone_variant)
                    {
                        if !supports(&profile.supported.trigger_policies, timezone) {
                            return Some(unsupported(profile, timezone.to_owned()));
                        }
                    }
                    if let Some(rrule) = schedule.get("rrule").and_then(Value::as_str) {
                        if let Some(unsupported) = unsupported_rrule_frequency(profile, rrule) {
                            return Some(unsupported);
                        }
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
                if !supports(&profile.supported.conditions, variant) {
                    if let Some(unsupported) =
                        unsupported_from_component(profile, "condition", variant, false)
                    {
                        return Some(unsupported);
                    }
                }
            }
        }
    }

    if let Some(action) = definition.get("action").and_then(Value::as_object) {
        if let Some(variant) = action.get("variant").and_then(Value::as_str) {
            if !supports(&profile.supported.actions, variant) {
                return unsupported_from_component(profile, "action", variant, false);
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
            if !supports_exact_value(&profile.supported.trigger_policies, "misfire", disposition) {
                return unsupported_from_component(profile, "misfire", disposition, false);
            }
        }
        if let Some(overlap) = policies
            .get("concurrency")
            .and_then(Value::as_object)
            .and_then(|concurrency| concurrency.get("overlap"))
            .and_then(Value::as_str)
        {
            if !supports_exact_value(&profile.supported.trigger_policies, "overlap", overlap) {
                return unsupported_from_component(profile, "overlap", overlap, false);
            }
        }
        if let Some(backoff) = policies
            .get("retry")
            .and_then(Value::as_object)
            .and_then(|retry| retry.get("backoffPolicy"))
            .and_then(Value::as_str)
        {
            if !supports_exact_value(
                &profile.supported.trigger_policies,
                "retry.backoff",
                backoff,
            ) {
                return unsupported_from_component(profile, "retry.backoff", backoff, false);
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
                if !retryable_class_is_supported(retryable_class) {
                    return unsupported_from_component(
                        profile,
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
                    if !supports_exact_value(
                        &profile.supported.delivery_policies,
                        "outputTarget",
                        mode,
                    ) {
                        return unsupported_from_component(profile, "outputTarget", mode, false);
                    }
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
                    if !supports_exact_value(
                        &profile.supported.retention_policies,
                        "retention",
                        classification,
                    ) {
                        return unsupported_from_component(
                            profile,
                            "retention",
                            classification,
                            false,
                        );
                    }
                }
            }
        }
    }

    if let Some(misfire) = definition.get("misfire").and_then(Value::as_str) {
        if !supports_exact_value(&profile.supported.trigger_policies, "misfire", misfire) {
            return unsupported_from_component(profile, "misfire", misfire, false);
        }
    }
    if let Some(overlap) = definition.get("overlap").and_then(Value::as_str) {
        if !supports_exact_value(&profile.supported.trigger_policies, "overlap", overlap) {
            return unsupported_from_component(profile, "overlap", overlap, false);
        }
    }
    if let Some(backoff) = definition
        .get("retry")
        .and_then(Value::as_object)
        .and_then(|retry| retry.get("backoffPolicy"))
        .and_then(Value::as_str)
    {
        if !supports_exact_value(
            &profile.supported.trigger_policies,
            "retry.backoff",
            backoff,
        ) {
            return unsupported_from_component(profile, "retry.backoff", backoff, false);
        }
    }
    if let Some(retryable_classes) = definition
        .get("retry")
        .and_then(Value::as_object)
        .and_then(|retry| retry.get("retryableClasses"))
        .and_then(Value::as_array)
        .filter(|classes| classes.iter().all(Value::is_string))
    {
        for retryable_class in retryable_classes.iter().filter_map(Value::as_str) {
            if !retryable_class_is_supported(retryable_class) {
                return unsupported_from_component(
                    profile,
                    "retry.safe-classes",
                    retryable_class,
                    false,
                );
            }
        }
    }
    if definition
        .get("outputTarget")
        .and_then(Value::as_str)
        .is_some()
        && !supports(&profile.supported.delivery_policies, "outputTarget.atomic")
    {
        return Some(unsupported(profile, "outputTarget.atomic".to_owned()));
    }
    if let Some(timezone) = definition.get("timezone") {
        if let Some(timezone_variant @ ("timezone.utc" | "timezone.iana")) =
            schedule_timezone_variant(timezone)
        {
            if !supports(&profile.supported.trigger_policies, timezone_variant) {
                return Some(unsupported(profile, timezone_variant.to_owned()));
            }
        }
    }
    if let Some(rrule) = definition.get("rrule").and_then(Value::as_str) {
        if let Some(unsupported) = unsupported_rrule_frequency(profile, rrule) {
            return Some(unsupported);
        }
    }

    None
}

pub fn negotiate_definition(definition: &Value) -> Result<DefinitionNegotiation, String> {
    let Some(unsupported) = preflight_definition(definition) else {
        return RoutineDefinition::from_json(definition)
            .map(Box::new)
            .map(DefinitionNegotiation::Supported);
    };

    let projection = validation_projection(definition, capability_profile());
    RoutineDefinition::from_json(&projection)?;
    Ok(DefinitionNegotiation::Unsupported(unsupported))
}

fn validation_projection(definition: &Value, profile: &CapabilityProfile) -> Value {
    let Value::Object(mut projection) = definition.clone() else {
        return definition.clone();
    };

    neutralize_flat_unsupported_values(&mut projection, profile);
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

fn neutralize_flat_unsupported_values(
    definition: &mut Map<String, Value>,
    profile: &CapabilityProfile,
) {
    if definition
        .get("misfire")
        .and_then(Value::as_str)
        .is_some_and(|misfire| {
            !supports_exact_value(&profile.supported.trigger_policies, "misfire", misfire)
                && unsupported_component(misfire, false).is_some()
        })
    {
        definition.insert("misfire".to_owned(), Value::String("latest".to_owned()));
    }
    if definition
        .get("overlap")
        .and_then(Value::as_str)
        .is_some_and(|overlap| {
            !supports_exact_value(&profile.supported.trigger_policies, "overlap", overlap)
                && unsupported_component(overlap, false).is_some()
        })
    {
        definition.insert("overlap".to_owned(), Value::String("forbid".to_owned()));
    }
    if let Some(retry) = definition.get_mut("retry").and_then(Value::as_object_mut) {
        if retry
            .get("backoffPolicy")
            .and_then(Value::as_str)
            .is_some_and(|backoff| {
                !supports_exact_value(
                    &profile.supported.trigger_policies,
                    "retry.backoff",
                    backoff,
                ) && unsupported_component(backoff, false).is_some()
            })
        {
            retry.insert("backoffPolicy".to_owned(), Value::String("none".to_owned()));
        }
        if let Some(retryable_classes) = retry
            .get_mut("retryableClasses")
            .and_then(Value::as_array_mut)
            .filter(|classes| classes.iter().all(Value::is_string))
        {
            for retryable_class in retryable_classes {
                let should_neutralize = retryable_class.as_str().is_some_and(|retryable_class| {
                    !retryable_class.trim().is_empty()
                        && !retryable_class_is_supported(retryable_class)
                });
                if should_neutralize {
                    *retryable_class = Value::String("transient_dispatch".to_owned());
                }
            }
        }
    }
    if definition.get("outputTarget").is_some_and(Value::is_string)
        && !supports(&profile.supported.delivery_policies, "outputTarget.atomic")
    {
        definition.remove("outputTarget");
    }
    if let Some(rrule) = definition.get("rrule").and_then(Value::as_str) {
        if unsupported_rrule_frequency_value(rrule).is_some() {
            definition.insert(
                "rrule".to_owned(),
                Value::String(supported_rrule_projection(rrule)),
            );
        }
    }
}

fn supported_rrule_projection(rrule: &str) -> String {
    let frequency = if rrule.split(';').any(|part| {
        part.split_once('=')
            .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case("BYDAY"))
    }) {
        "WEEKLY"
    } else {
        "DAILY"
    };
    neutralize_rrule_frequency(rrule, frequency)
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
    let Some(variant) = trigger.get("variant").and_then(Value::as_str) else {
        return false;
    };
    if variant != "schedule" {
        return unsupported_union_hint_is_well_formed(trigger);
    }
    has_only_fields(trigger, &["variant", "version", "schedule"])
        && trigger.get("version").is_some_and(is_version_one)
        && trigger.get("schedule").is_some_and(schedule_is_well_formed)
}

fn schedule_is_well_formed(value: &Value) -> bool {
    let Some(schedule) = value.as_object() else {
        return false;
    };
    has_only_fields(schedule, &["rrule", "timezone"])
        && schedule
            .get("rrule")
            .and_then(Value::as_str)
            .is_some_and(schedule_rrule_is_well_formed)
        && schedule
            .get("timezone")
            .is_some_and(schedule_timezone_is_well_formed)
}

fn schedule_rrule_is_well_formed(rrule: &str) -> bool {
    if !(1..=512).contains(&rrule.chars().count()) {
        return false;
    }
    let validation_rrule = if unsupported_rrule_frequency_value(rrule).is_some() {
        supported_rrule_projection(rrule)
    } else {
        rrule.to_owned()
    };
    parse_rrule(&validation_rrule).is_ok()
}

fn schedule_timezone_is_well_formed(timezone: &Value) -> bool {
    serde_json::from_value::<RoutineTimezone>(timezone.clone()).is_ok()
}

fn conditions_hint_is_well_formed(value: &Value) -> bool {
    value.as_array().is_some_and(|conditions| {
        conditions.len() <= 16
            && conditions.iter().all(|condition| {
                let Some(condition) = condition.as_object() else {
                    return false;
                };
                unsupported_union_hint_is_well_formed(condition)
            })
    })
}

fn action_hint_is_well_formed(value: &Value) -> bool {
    let Some(action) = value.as_object() else {
        return false;
    };
    let Some(variant) = action.get("variant").and_then(Value::as_str) else {
        return false;
    };
    if variant != "familiarInvocation" {
        return unsupported_union_hint_is_well_formed(action);
    }
    serde_json::from_value::<FamiliarInvocationAction>(value.clone()).is_ok()
}

fn unsupported_union_hint_is_well_formed(union: &Map<String, Value>) -> bool {
    union
        .get("variant")
        .and_then(Value::as_str)
        .is_some_and(|variant| !variant.trim().is_empty())
        && union.get("version").is_some_and(is_version_one)
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
    serde_json::from_value::<TimeoutPolicy>(value.clone()).is_ok()
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
        || !retry
            .get("maxAttempts")
            .and_then(Value::as_u64)
            .is_some_and(|attempts| (1..=10).contains(&attempts))
        || retry
            .get("backoffPolicy")
            .and_then(Value::as_str)
            .is_none_or(|backoff| backoff.trim().is_empty())
        || !retry.get("backoffSeconds").is_none_or(|value| {
            value
                .as_u64()
                .is_some_and(|seconds| (1..=86_400).contains(&seconds))
        })
    {
        return false;
    }
    if retry.get("backoffPolicy").and_then(Value::as_str) == Some("fixed")
        && retry.get("backoffSeconds").is_none()
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
    if !has_only_fields(delivery, &["outputTarget", "mode"])
        || !delivery.get("outputTarget").is_none_or(|output_target| {
            serde_json::from_value::<WorkingDirectory>(output_target.clone()).is_ok()
        })
        || delivery
            .get("mode")
            .is_some_and(|mode| mode.as_str().is_none_or(|mode| mode.trim().is_empty()))
    {
        return false;
    }
    delivery.get("outputTarget").is_none() || delivery.get("mode").is_some()
}

fn retention_hint_is_well_formed(value: &Value) -> bool {
    let Some(retention) = value.as_object() else {
        return false;
    };
    has_only_fields(retention, &["occurrenceHistory", "runLogs", "receipts"])
        && retention.get("occurrenceHistory").is_some()
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
        && retention_class
            .get("deleteAfter")
            .is_none_or(|delete_after| {
                serde_json::from_value::<Timestamp>(delete_after.clone()).is_ok()
            })
}

fn single_string_field_is_well_formed(value: &Value, field: &str) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    has_only_fields(object, &[field])
        && object
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
}

fn has_only_fields(object: &Map<String, Value>, allowed: &[&str]) -> bool {
    object.keys().all(|key| allowed.contains(&key.as_str()))
}

fn is_version_one(value: &Value) -> bool {
    value.as_u64() == Some(1)
}

fn retryable_class_is_supported(value: &str) -> bool {
    serde_json::from_value::<RetryableClass>(Value::String(value.to_owned())).is_ok()
}

fn schedule_timezone_variant(value: &Value) -> Option<&'static str> {
    match serde_json::from_value::<RoutineTimezone>(value.clone()).ok()? {
        RoutineTimezone::Local => Some("timezone.local.durable"),
        RoutineTimezone::Utc => Some("timezone.utc"),
        RoutineTimezone::Iana(_) => Some("timezone.iana"),
    }
}

fn supports(variants: &[VariantCapability], variant: &str) -> bool {
    variants
        .iter()
        .any(|supported| supported.variant == variant)
}

fn supports_exact_value(variants: &[VariantCapability], prefix: &str, value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && supports(variants, &format!("{prefix}.{value}"))
}

fn unsupported_rrule_frequency_value(rrule: &str) -> Option<&str> {
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
    Some(frequency)
}

fn unsupported_rrule_frequency(
    profile: &CapabilityProfile,
    rrule: &str,
) -> Option<UnsupportedVariant> {
    unsupported_from_component(
        profile,
        "trigger.schedule.frequency",
        unsupported_rrule_frequency_value(rrule)?,
        true,
    )
}

fn unsupported_from_component(
    profile: &CapabilityProfile,
    prefix: &str,
    value: &str,
    lowercase: bool,
) -> Option<UnsupportedVariant> {
    let component = unsupported_component(value, lowercase)?;
    Some(unsupported(profile, format!("{prefix}.{component}")))
}

fn unsupported_component(value: &str, lowercase: bool) -> Option<String> {
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
    Some(component)
}

fn unsupported(profile: &CapabilityProfile, variant: String) -> UnsupportedVariant {
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

    #[derive(Clone, Copy)]
    enum SupportedCollection {
        Trigger,
        Condition,
        Action,
        TriggerPolicy,
        DeliveryPolicy,
        RetentionPolicy,
    }

    fn supported_variants_mut(
        profile: &mut CapabilityProfile,
        collection: SupportedCollection,
    ) -> &mut Vec<VariantCapability> {
        match collection {
            SupportedCollection::Trigger => &mut profile.supported.triggers,
            SupportedCollection::Condition => &mut profile.supported.conditions,
            SupportedCollection::Action => &mut profile.supported.actions,
            SupportedCollection::TriggerPolicy => &mut profile.supported.trigger_policies,
            SupportedCollection::DeliveryPolicy => &mut profile.supported.delivery_policies,
            SupportedCollection::RetentionPolicy => &mut profile.supported.retention_policies,
        }
    }

    fn advertise(profile: &mut CapabilityProfile, collection: SupportedCollection, variant: &str) {
        supported_variants_mut(profile, collection).push(VariantCapability {
            variant: variant.to_owned(),
            profile: None,
            notes: None,
        });
    }

    fn assert_variant(definition: Value, expected: &str) {
        let unsupported = preflight_definition(&definition)
            .unwrap_or_else(|| panic!("expected unsupported variant `{expected}`"));
        assert_eq!(unsupported.variant, expected);
        assert!(!unsupported.reason.is_empty());
        assert!(unsupported.reason.len() <= 500);
    }

    fn complete_definition() -> Value {
        json!({
            "schemaVersion": 1,
            "id": "negotiation-test",
            "name": "Negotiation test",
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "local",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "prompt": "Exercise capability negotiation."
        })
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
    fn unsupported_union_payloads_do_not_require_supported_variant_fields() {
        for (key, definition, expected) in [
            (
                "trigger",
                json!({"trigger": {
                    "variant": "webhook",
                    "version": 1,
                    "webhook": {"url": "https://example.invalid/hook"}
                }}),
                "trigger.webhook",
            ),
            (
                "conditions",
                json!({"conditions": [{
                    "variant": "branch",
                    "version": 1,
                    "branch": {"expression": "result.ok"}
                }]}),
                "condition.branch",
            ),
            (
                "action",
                json!({"action": {
                    "variant": "pipeline",
                    "version": 1,
                    "steps": [{"prompt": "first"}]
                }}),
                "action.pipeline",
            ),
        ] {
            assert_variant(definition.clone(), expected);
            assert!(
                rich_hint_is_well_formed(key, definition.get(key).unwrap()),
                "{key}"
            );
        }
    }

    #[test]
    fn unsupported_union_hints_with_invalid_versions_remain_validation_errors() {
        for (key, value) in [
            (
                "trigger",
                json!({"variant": "webhook", "version": 2, "webhook": {}}),
            ),
            (
                "conditions",
                json!([{"variant": "branch", "version": "one", "branch": {}}]),
            ),
            (
                "action",
                json!({"variant": "pipeline", "version": false, "steps": []}),
            ),
        ] {
            assert!(!rich_hint_is_well_formed(key, &value));
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
                    "maxAttempts": 2,
                    "backoffPolicy": "none",
                    "retryableClasses": ["transient_dispatch", retryable_class]
                }}}),
                expected,
            );
        }
    }

    #[test]
    fn flat_retryable_classes_are_negotiated_after_structural_validation() {
        let mut definition = complete_definition();
        definition["retry"] = json!({
            "maxAttempts": 2,
            "backoffPolicy": "none",
            "retryableClasses": ["transient_dispatch", "ambiguous"]
        });

        let negotiation = negotiate_definition(&definition).unwrap();
        let DefinitionNegotiation::Unsupported(unsupported) = negotiation else {
            panic!("unknown flat retryable class must be refused");
        };
        assert_eq!(unsupported.variant, "retry.safe-classes.ambiguous");
    }

    #[test]
    fn rich_schedule_validation_preserves_malformed_nested_values() {
        for (case, trigger) in [
            (
                "unsupported-frequency-with-malformed-hour",
                json!({
                    "variant": "schedule",
                    "version": 1,
                    "schedule": {
                        "rrule": "FREQ=YEARLY;BYHOUR=not-a-number",
                        "timezone": "utc"
                    }
                }),
            ),
            (
                "local-timezone-with-malformed-hour",
                json!({
                    "variant": "schedule",
                    "version": 1,
                    "schedule": {
                        "rrule": "FREQ=DAILY;BYHOUR=not-a-number",
                        "timezone": "local"
                    }
                }),
            ),
            (
                "malformed-timezone",
                json!({
                    "variant": "schedule",
                    "version": 1,
                    "schedule": {
                        "rrule": "FREQ=DAILY",
                        "timezone": "Mars/Olympus"
                    }
                }),
            ),
            (
                "missing-version",
                json!({
                    "variant": "schedule",
                    "schedule": {
                        "rrule": "FREQ=DAILY",
                        "timezone": "utc"
                    }
                }),
            ),
            (
                "missing-schedule",
                json!({
                    "variant": "schedule",
                    "version": 1
                }),
            ),
        ] {
            let mut definition = complete_definition();
            definition["outputTarget"] = json!("result.md");
            definition["trigger"] = trigger;

            assert!(
                negotiate_definition(&definition).is_err(),
                "{case} must remain a validation failure"
            );
        }
    }

    #[test]
    fn rich_schedule_allows_valid_unsupported_frequency_negotiation() {
        let mut definition = complete_definition();
        definition["trigger"] = json!({
            "variant": "schedule",
            "version": 1,
            "schedule": {
                "rrule": "FREQ=YEARLY;BYHOUR=9",
                "timezone": "utc"
            }
        });

        let negotiation = negotiate_definition(&definition).unwrap();
        let DefinitionNegotiation::Unsupported(unsupported) = negotiation else {
            panic!("unsupported frequency must be refused");
        };
        assert_eq!(unsupported.variant, "trigger.schedule.frequency.yearly");
    }

    #[test]
    fn rich_union_hints_require_version_one_and_supported_action_fields() {
        for (case, key, hint) in [
            (
                "unsupported-trigger-without-version",
                "trigger",
                json!({"variant": "webhook", "webhook": {}}),
            ),
            (
                "unsupported-condition-without-version",
                "conditions",
                json!([{"variant": "branch", "branch": {}}]),
            ),
            (
                "unsupported-action-without-version",
                "action",
                json!({"variant": "pipeline", "steps": []}),
            ),
            (
                "supported-action-without-version",
                "action",
                json!({"variant": "familiarInvocation", "prompt": "Run it."}),
            ),
            (
                "supported-action-with-empty-prompt",
                "action",
                json!({"variant": "familiarInvocation", "version": 1, "prompt": ""}),
            ),
        ] {
            let mut definition = complete_definition();
            definition["outputTarget"] = json!("result.md");
            definition[key] = hint;

            assert!(
                negotiate_definition(&definition).is_err(),
                "{case} must remain a validation failure"
            );
        }
    }

    #[test]
    fn each_present_rich_policy_subsection_must_satisfy_its_schema_shape() {
        for (case, policies) in [
            (
                "retry-missing-max-attempts",
                json!({"retry": {"backoffPolicy": "none"}}),
            ),
            (
                "retry-missing-backoff-policy",
                json!({"retry": {"maxAttempts": 2}}),
            ),
            (
                "fixed-retry-missing-backoff-seconds",
                json!({"retry": {"maxAttempts": 2, "backoffPolicy": "fixed"}}),
            ),
            (
                "non-fixed-retry-invalid-backoff-seconds",
                json!({
                    "retry": {
                        "maxAttempts": 2,
                        "backoffPolicy": "none",
                        "backoffSeconds": "soon"
                    }
                }),
            ),
            (
                "retention-missing-occurrence-history",
                json!({
                    "retention": {
                        "receipts": {"classification": "standard"}
                    }
                }),
            ),
            (
                "delivery-target-missing-mode",
                json!({"delivery": {"outputTarget": "result.md"}}),
            ),
        ] {
            let mut definition = complete_definition();
            definition["outputTarget"] = json!("result.md");
            definition["policies"] = policies;

            assert!(
                negotiate_definition(&definition).is_err(),
                "{case} must remain a validation failure"
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
            json!({"policies": {"retry": {
                "maxAttempts": 2,
                "backoffPolicy": "none",
                "retryableClasses": "runtime_unavailable"
            }}}),
            json!({"policies": {"retry": {
                "maxAttempts": 2,
                "backoffPolicy": "none",
                "retryableClasses": [1]
            }}}),
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
                    "maxAttempts": 2,
                    "backoffPolicy": "fixed",
                    "backoffSeconds": 5,
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
            "maxAttempts": 2,
            "backoffPolicy": "none",
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
        for supported in &profile.supported.retention_policies {
            let classification =
                supported
                    .variant
                    .strip_prefix("retention.")
                    .unwrap_or_else(|| {
                        panic!(
                            "missing preflight fixture for supported retention policy `{}`",
                            supported.variant
                        )
                    });
            assert_eq!(
                preflight_definition(&json!({"policies": {"retention": {
                    "occurrenceHistory": {"classification": classification}
                }}})),
                None,
                "advertised retention policy `{}` was refused",
                supported.variant
            );
        }
    }

    #[test]
    fn removing_an_implemented_exact_variant_from_the_profile_refuses_it() {
        for (collection, supported_variant, expected_variant, definition) in [
            (
                SupportedCollection::Trigger,
                "schedule",
                "trigger.schedule",
                json!({"trigger": {
                    "variant": "schedule",
                    "version": 1,
                    "schedule": {"rrule": "FREQ=DAILY", "timezone": "utc"}
                }}),
            ),
            (
                SupportedCollection::Action,
                "familiarInvocation",
                "action.familiarInvocation",
                json!({"action": {
                    "variant": "familiarInvocation",
                    "version": 1,
                    "prompt": "Run it."
                }}),
            ),
            (
                SupportedCollection::TriggerPolicy,
                "misfire.latest",
                "misfire.latest",
                json!({"misfire": "latest"}),
            ),
            (
                SupportedCollection::TriggerPolicy,
                "overlap.forbid",
                "overlap.forbid",
                json!({"overlap": "forbid"}),
            ),
            (
                SupportedCollection::TriggerPolicy,
                "retry.backoff.none",
                "retry.backoff.none",
                json!({"retry": {"backoffPolicy": "none"}}),
            ),
            (
                SupportedCollection::TriggerPolicy,
                "retry.backoff.fixed",
                "retry.backoff.fixed",
                json!({"retry": {"backoffPolicy": "fixed"}}),
            ),
            (
                SupportedCollection::TriggerPolicy,
                "retry.backoff.exponential",
                "retry.backoff.exponential",
                json!({"retry": {"backoffPolicy": "exponential"}}),
            ),
            (
                SupportedCollection::TriggerPolicy,
                "timezone.utc",
                "timezone.utc",
                json!({"trigger": {
                    "variant": "schedule",
                    "version": 1,
                    "schedule": {"rrule": "FREQ=DAILY", "timezone": "utc"}
                }}),
            ),
            (
                SupportedCollection::TriggerPolicy,
                "timezone.iana",
                "timezone.iana",
                json!({"trigger": {
                    "variant": "schedule",
                    "version": 1,
                    "schedule": {
                        "rrule": "FREQ=DAILY",
                        "timezone": "America/Chicago"
                    }
                }}),
            ),
            (
                SupportedCollection::RetentionPolicy,
                "retention.standard",
                "retention.standard",
                json!({"policies": {"retention": {
                    "occurrenceHistory": {"classification": "standard"}
                }}}),
            ),
        ] {
            let mut profile = capability_profile().clone();
            supported_variants_mut(&mut profile, collection)
                .retain(|supported| supported.variant != supported_variant);

            let unsupported = preflight_definition_with_profile(&definition, &profile)
                .unwrap_or_else(|| {
                    panic!("removed variant `{supported_variant}` was still accepted")
                });
            assert_eq!(unsupported.variant, expected_variant);
        }
    }

    #[test]
    fn advertising_an_exact_variant_removes_its_capability_refusal() {
        for (collection, variant, definition) in [
            (
                SupportedCollection::Trigger,
                "webhook",
                json!({"trigger": {
                    "variant": "webhook",
                    "version": 1,
                    "webhook": {"url": "https://example.invalid/hook"}
                }}),
            ),
            (
                SupportedCollection::Condition,
                "branch",
                json!({"conditions": [{
                    "variant": "branch",
                    "version": 1,
                    "branch": {"expression": "result.ok"}
                }]}),
            ),
            (
                SupportedCollection::Action,
                "pipeline",
                json!({"action": {
                    "variant": "pipeline",
                    "version": 1,
                    "steps": [{"prompt": "first"}]
                }}),
            ),
            (
                SupportedCollection::TriggerPolicy,
                "misfire.backfill",
                json!({"misfire": "backfill"}),
            ),
            (
                SupportedCollection::TriggerPolicy,
                "overlap.parallel",
                json!({"overlap": "parallel"}),
            ),
            (
                SupportedCollection::TriggerPolicy,
                "retry.backoff.linear",
                json!({"retry": {"backoffPolicy": "linear"}}),
            ),
            (
                SupportedCollection::TriggerPolicy,
                "timezone.local.durable",
                json!({"trigger": {
                    "variant": "schedule",
                    "version": 1,
                    "schedule": {"rrule": "FREQ=DAILY", "timezone": "local"}
                }}),
            ),
            (
                SupportedCollection::DeliveryPolicy,
                "outputTarget.atomic",
                json!({"outputTarget": "result.md"}),
            ),
            (
                SupportedCollection::RetentionPolicy,
                "retention.extended",
                json!({"policies": {"retention": {
                    "occurrenceHistory": {"classification": "extended"}
                }}}),
            ),
        ] {
            let mut profile = capability_profile().clone();
            advertise(&mut profile, collection, variant);

            assert_eq!(
                preflight_definition_with_profile(&definition, &profile),
                None,
                "advertised variant `{variant}` was still refused"
            );
        }
    }
}
