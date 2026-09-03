//! Serde projections of every object published in Coven Automations v1.

use std::collections::BTreeMap;
use std::fmt;

use serde::ser::{SerializeMap, SerializeStruct, Serializer};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::error::ErrorEnvelope;

pub type JsonObject = BTreeMap<String, Value>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringConstraintError(&'static str);

impl fmt::Display for StringConstraintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for StringConstraintError {}

macro_rules! validated_string {
    ($name:ident, $validator:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: String) -> Result<Self, StringConstraintError> {
                if $validator(&value) {
                    Ok(Self(value))
                } else {
                    Err(StringConstraintError(stringify!($name)))
                }
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

fn matches_identifier(value: &str, maximum: usize) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= maximum
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn matches_adoption_key(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 8
        && bytes.len() <= 200
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn matches_correlation_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 200
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn matches_principal_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'@' | b'-')
        })
}

fn matches_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    let separators = match bytes.len() {
        20 if matches!(
            bytes,
            [
                _,
                _,
                _,
                _,
                b'-',
                _,
                _,
                b'-',
                _,
                _,
                b'T',
                _,
                _,
                b':',
                _,
                _,
                b':',
                _,
                _,
                b'Z'
            ]
        ) =>
        {
            [4, 7, 10, 13, 16, 19].as_slice()
        }
        24 if matches!(
            bytes,
            [
                _,
                _,
                _,
                _,
                b'-',
                _,
                _,
                b'-',
                _,
                _,
                b'T',
                _,
                _,
                b':',
                _,
                _,
                b':',
                _,
                _,
                b'.',
                _,
                _,
                _,
                b'Z'
            ]
        ) =>
        {
            [4, 7, 10, 13, 16, 19, 23].as_slice()
        }
        _ => return false,
    };
    bytes
        .iter()
        .enumerate()
        .filter(|(index, _)| !separators.contains(index))
        .all(|(_, byte)| byte.is_ascii_digit())
}

fn matches_automation_id(value: &str) -> bool {
    matches_identifier(value, 96)
}

fn matches_entity_id(value: &str) -> bool {
    matches_identifier(value, 160)
}

fn matches_familiar_id(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 64
}

fn matches_runtime_id(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 64
}

fn matches_approval_policy_ref(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 200
}

fn matches_capability(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 96
}

fn matches_approval_record_ref(value: &str) -> bool {
    value.chars().count() <= 200
}

fn matches_runtime_model(value: &str) -> bool {
    value.chars().count() <= 128
}

fn matches_sha256_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn matches_event_id(value: &str) -> bool {
    (20..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn matches_event_stream_id(value: &str) -> bool {
    value.chars().count() <= 320
}

fn matches_command_stream_id(value: &str) -> bool {
    (1..=320).contains(&value.chars().count())
}

fn matches_event_summary(value: &str) -> bool {
    (1..=300).contains(&value.chars().count())
}

fn matches_display_name(value: &str) -> bool {
    (1..=160).contains(&value.chars().count())
}

fn matches_display_description(value: &str) -> bool {
    value.chars().count() <= 2_000
}

fn matches_display_tag(value: &str) -> bool {
    (1..=64).contains(&value.chars().count())
}

fn matches_cause_event_id(value: &str) -> bool {
    value.chars().count() <= 64
}

fn matches_component_name(value: &str) -> bool {
    (1..=96).contains(&value.chars().count())
}

fn matches_instance_id(value: &str) -> bool {
    (1..=128).contains(&value.chars().count())
}

fn matches_implementation_version(value: &str) -> bool {
    value.chars().count() <= 64
}

fn matches_event_ref_stream(value: &str) -> bool {
    (1..=400).contains(&value.chars().count())
}

validated_string!(Timestamp, matches_timestamp);
validated_string!(AutomationId, matches_automation_id);
validated_string!(OccurrenceId, matches_entity_id);
validated_string!(RunId, matches_entity_id);
validated_string!(AttemptId, matches_entity_id);
validated_string!(ReceiptId, matches_entity_id);
validated_string!(AdoptionKey, matches_adoption_key);
validated_string!(CorrelationId, matches_correlation_id);
validated_string!(PrincipalId, matches_principal_id);
validated_string!(FamiliarId, matches_familiar_id);
validated_string!(RuntimeId, matches_runtime_id);
validated_string!(ApprovalPolicyRef, matches_approval_policy_ref);
validated_string!(Capability, matches_capability);
validated_string!(Sha256Digest, matches_sha256_digest);
validated_string!(ApprovalRecordRef, matches_approval_record_ref);
validated_string!(RuntimeModel, matches_runtime_model);
validated_string!(EventId, matches_event_id);
validated_string!(EventStreamId, matches_event_stream_id);
validated_string!(CommandStreamId, matches_command_stream_id);
validated_string!(EventSummary, matches_event_summary);
validated_string!(DisplayName, matches_display_name);
validated_string!(DisplayDescription, matches_display_description);
validated_string!(DisplayTag, matches_display_tag);
validated_string!(CauseEventId, matches_cause_event_id);
validated_string!(ComponentName, matches_component_name);
validated_string!(InstanceId, matches_instance_id);
validated_string!(ImplementationVersion, matches_implementation_version);
validated_string!(EventRefStream, matches_event_ref_stream);

macro_rules! validated_integer {
    ($name:ident, $minimum:expr, $maximum:expr) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name(u64);

        impl $name {
            pub fn new(value: u64) -> Result<Self, StringConstraintError> {
                if ($minimum..=$maximum).contains(&value) {
                    Ok(Self(value))
                } else {
                    Err(StringConstraintError(stringify!($name)))
                }
            }

            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_u64(self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::new(u64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

validated_integer!(PositiveInteger, 1, 9_007_199_254_740_991);
validated_integer!(TimeoutMinutes, 1, 44_640);
validated_integer!(MaximumAttempts, 1, 10);
validated_integer!(BackoffSeconds, 1, 86_400);
validated_integer!(LeaseMinutes, 1, 1_440);
validated_integer!(RunAttemptCount, 1, 100);
validated_integer!(CommandListLimit, 1, 100);
validated_integer!(EventReadLimit, 1, 1_000);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionBag(BTreeMap<String, Value>);

impl ExtensionBag {
    pub fn new(values: BTreeMap<String, Value>) -> Result<Self, StringConstraintError> {
        if values.keys().all(|key| matches_extension_key(key)) {
            Ok(Self(values))
        } else {
            Err(StringConstraintError("ExtensionBag"))
        }
    }
}

impl Serialize for ExtensionBag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ExtensionBag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(BTreeMap::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

fn matches_extension_key(key: &str) -> bool {
    if let Some(suffix) = key.strip_prefix("x-") {
        return !suffix.is_empty()
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    }
    let mut parts = key.rsplitn(3, '.');
    let Some(last) = parts.next() else {
        return false;
    };
    let Some(middle) = parts.next() else {
        return false;
    };
    let Some(first) = parts.next() else {
        return false;
    };
    !first.is_empty()
        && !middle.is_empty()
        && !last.is_empty()
        && first
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.')
        && middle
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && last
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCapabilities(Vec<Capability>);

impl RuntimeCapabilities {
    fn new(values: Vec<Capability>) -> Result<Self, StringConstraintError> {
        let mut seen = std::collections::BTreeSet::new();
        if values.iter().all(|value| seen.insert(value.as_str())) {
            Ok(Self(values))
        } else {
            Err(StringConstraintError("RuntimeCapabilities must be unique"))
        }
    }
}

impl Serialize for RuntimeCapabilities {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RuntimeCapabilities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Vec::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExercisedCapabilities(Vec<Capability>);

impl ExercisedCapabilities {
    fn new(values: Vec<Capability>) -> Result<Self, StringConstraintError> {
        if values.len() > 128 {
            return Err(StringConstraintError(
                "ExercisedCapabilities must contain at most 128 entries",
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        if values.iter().all(|value| seen.insert(value.as_str())) {
            Ok(Self(values))
        } else {
            Err(StringConstraintError(
                "ExercisedCapabilities must be unique",
            ))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayTags(Vec<DisplayTag>);

impl DisplayTags {
    fn new(values: Vec<DisplayTag>) -> Result<Self, StringConstraintError> {
        if values.len() > 64 {
            return Err(StringConstraintError(
                "DisplayTags must contain at most 64 entries",
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        if values.iter().all(|value| seen.insert(value.as_str())) {
            Ok(Self(values))
        } else {
            Err(StringConstraintError("DisplayTags must be unique"))
        }
    }
}

impl Serialize for DisplayTags {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DisplayTags {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Vec::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl Serialize for ExercisedCapabilities {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ExercisedCapabilities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Vec::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaVersion {
    #[serde(rename = "coven.automations.v1")]
    V1,
}

/// A wire-number that accepts and emits exactly `1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VersionOne;

impl Serialize for VersionOne {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(1)
    }
}

impl<'de> Deserialize<'de> for VersionOne {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match u8::deserialize(deserializer)? {
            1 => Ok(Self),
            value => Err(serde::de::Error::custom(format!(
                "expected version 1, got {value}"
            ))),
        }
    }
}

/// The v1 conditions union has no variants, so only an empty array is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EmptyConditions;

impl Serialize for EmptyConditions {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Vec::<Value>::new().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for EmptyConditions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = Vec::<Value>::deserialize(deserializer)?;
        if values.is_empty() {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(
                "coven.automations.v1 defines no condition variants",
            ))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DigestValue {
    pub algorithm: DigestAlgorithm,
    pub canonicalization: Canonicalization,
    pub value: Sha256Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DigestAlgorithm {
    #[serde(rename = "sha256")]
    Sha256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Canonicalization {
    #[serde(rename = "jcs-rfc8785")]
    JcsRfc8785,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrincipalRef {
    pub principal_id: PrincipalId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FamiliarRef {
    pub familiar_id: FamiliarId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeDescriptor {
    pub runtime_id: RuntimeId,
    pub capabilities: RuntimeCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<RuntimeModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalRef {
    pub approval_policy_ref: ApprovalPolicyRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_record_ref: Option<ApprovalRecordRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyClassification {
    Public,
    Operational,
    Sensitive,
    Restricted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetentionClass {
    pub classification: RetentionClassification,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete_after: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClassification {
    Ephemeral,
    Standard,
    Extended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProducerIdentity {
    pub component: ComponentName,
    pub instance_id: InstanceId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub implementation_version: Option<ImplementationVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provenance {
    pub created_by: PrincipalRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<PrincipalRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivationWindow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_from: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_until: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefinitionLifecycleState {
    Draft,
    Paused,
    Active,
    Disabled,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationDefinition {
    pub schema_version: SchemaVersion,
    pub automation_id: AutomationId,
    pub revision: PositiveInteger,
    pub integrity: DigestValue,
    lifecycle_state: DefinitionLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletion: Option<DefinitionDeletion>,
    pub display: DefinitionDisplay,
    pub trigger: ScheduleTrigger,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<EmptyConditions>,
    pub action: FamiliarInvocationAction,
    binding: DefinitionBinding,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_requirements: Option<RuntimeDescriptor>,
    policies: DefinitionPolicies,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activation: Option<ActivationWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionBag>,
}

impl AutomationDefinition {
    #[must_use]
    pub const fn lifecycle_state(&self) -> DefinitionLifecycleState {
        self.lifecycle_state
    }

    #[must_use]
    pub fn binding(&self) -> &DefinitionBinding {
        &self.binding
    }

    #[must_use]
    pub fn runtime_requirements(&self) -> Option<&RuntimeDescriptor> {
        self.runtime_requirements.as_ref()
    }

    #[must_use]
    pub fn policies(&self) -> &DefinitionPolicies {
        &self.policies
    }

    fn validate(&self) -> Result<(), StringConstraintError> {
        if matches!(
            self.lifecycle_state,
            DefinitionLifecycleState::Active
                | DefinitionLifecycleState::Paused
                | DefinitionLifecycleState::Disabled
        ) && (self.runtime_requirements.is_none() || self.binding.familiar_id.is_none())
        {
            return Err(StringConstraintError(
                "active, paused, and disabled definitions require runtimeRequirements and binding.familiarId",
            ));
        }
        if self.policies.retry.backoff_policy == BackoffPolicy::Fixed
            && self.policies.retry.backoff_seconds.is_none()
        {
            return Err(StringConstraintError(
                "fixed retry backoff requires backoffSeconds",
            ));
        }
        if self
            .policies
            .delivery
            .as_ref()
            .is_some_and(|delivery| delivery.output_target.is_some() && delivery.mode.is_none())
        {
            return Err(StringConstraintError("delivery outputTarget requires mode"));
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for AutomationDefinition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            schema_version: SchemaVersion,
            automation_id: AutomationId,
            revision: PositiveInteger,
            integrity: DigestValue,
            lifecycle_state: DefinitionLifecycleState,
            deletion: Option<DefinitionDeletion>,
            display: DefinitionDisplay,
            trigger: ScheduleTrigger,
            conditions: Option<EmptyConditions>,
            action: FamiliarInvocationAction,
            binding: DefinitionBinding,
            runtime_requirements: Option<RuntimeDescriptor>,
            policies: DefinitionPolicies,
            provenance: Option<Provenance>,
            activation: Option<ActivationWindow>,
            extensions: Option<ExtensionBag>,
        }

        let raw = Raw::deserialize(deserializer)?;
        let definition = Self {
            schema_version: raw.schema_version,
            automation_id: raw.automation_id,
            revision: raw.revision,
            integrity: raw.integrity,
            lifecycle_state: raw.lifecycle_state,
            deletion: raw.deletion,
            display: raw.display,
            trigger: raw.trigger,
            conditions: raw.conditions,
            action: raw.action,
            binding: raw.binding,
            runtime_requirements: raw.runtime_requirements,
            policies: raw.policies,
            provenance: raw.provenance,
            activation: raw.activation,
            extensions: raw.extensions,
        };
        definition.validate().map_err(serde::de::Error::custom)?;
        Ok(definition)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionDeletion {
    pub tombstoned: True,
    pub requested_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<PrincipalRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct True;

impl Serialize for True {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for True {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("expected true"))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionDisplay {
    pub name: DisplayName,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<DisplayDescription>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<DisplayTags>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleTrigger {
    pub variant: ScheduleVariant,
    pub version: VersionOne,
    pub schedule: Schedule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduleVariant {
    #[serde(rename = "schedule")]
    Schedule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Schedule {
    pub rrule: String,
    pub timezone: Timezone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Timezone {
    Local,
    Utc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FamiliarInvocationAction {
    pub variant: FamiliarInvocationVariant,
    pub version: VersionOne,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FamiliarInvocationVariant {
    #[serde(rename = "familiarInvocation")]
    FamiliarInvocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionBinding {
    pub familiar_binding_policy: FamiliarBindingPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub familiar_id: Option<FamiliarId>,
    pub authority: ApprovalRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FamiliarBindingPolicy {
    Exact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionPolicies {
    pub timeout: TimeoutPolicy,
    pub retry: RetryPolicy,
    pub concurrency: ConcurrencyPolicy,
    pub misfire: MisfirePolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<DeliveryPolicy>,
    pub retention: RetentionPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeoutPolicy {
    pub per_run_minutes: TimeoutMinutes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetryPolicy {
    pub max_attempts: MaximumAttempts,
    pub backoff_policy: BackoffPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backoff_seconds: Option<BackoffSeconds>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable_classes: Option<Vec<RetryableClass>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackoffPolicy {
    None,
    Fixed,
    Exponential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryableClass {
    TransientDispatch,
    LeaseExpired,
    RuntimeUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConcurrencyPolicy {
    pub overlap: OverlapPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlapPolicy {
    Forbid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MisfirePolicy {
    pub disposition: MisfirePolicyDisposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MisfirePolicyDisposition {
    Latest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<DeliveryMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    Atomic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetentionPolicy {
    pub occurrence_history: RetentionClass,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_logs: Option<RetentionClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipts: Option<RetentionClass>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceState {
    Planned,
    Eligible,
    Claimed,
    Dispatching,
    Running,
    Recovering,
    RecoveryRequired,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Skipped,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MisfireDisposition {
    None,
    CollapsedToLatest,
    SkippedOverlap,
    SkippedPaused,
    SkippedInvalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationOccurrence {
    pub schema_version: SchemaVersion,
    pub occurrence_id: OccurrenceId,
    pub automation_id: AutomationId,
    pub automation_revision: PositiveInteger,
    pub trigger_identity: TriggerIdentity,
    pub occurrence_key: String,
    pub scheduled_for: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eligible_at: Option<Timestamp>,
    pub state: OccurrenceState,
    pub state_reason: String,
    pub fence: OccurrenceFence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub misfire_disposition: Option<MisfireDisposition>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim_metadata: Option<ClaimMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_run_ref: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancellation: Option<Cancellation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<Recovery>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_window: Option<EventWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionBag>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TriggerIdentity {
    pub kind: TriggerIdentityKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rrule_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<PrincipalRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerIdentityKind {
    #[serde(rename = "schedule.slot")]
    ScheduleSlot,
    #[serde(rename = "manual.request")]
    ManualRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OccurrenceFence {
    pub generation: PositiveInteger,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaimMetadata {
    pub claimed_at: Timestamp,
    pub lease_minutes: LeaseMinutes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cancellation {
    pub requested_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<PrincipalRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciled_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Recovery {
    pub entered_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<RecoveryEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_disposition: Option<RecoveryDisposition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryEvidence {
    LeaseExpired,
    DispatchUnconfirmed,
    RuntimeLost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryDisposition {
    FailedDeterministic,
    FailedAmbiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventWindow {
    pub first_sequence: u64,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Accepted,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Ambiguous,
}

pub type RunOutcome = RunState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationRun {
    pub schema_version: SchemaVersion,
    pub run_id: RunId,
    pub occurrence_id: OccurrenceId,
    pub automation_id: AutomationId,
    pub automation_revision: PositiveInteger,
    pub binding: RunBinding,
    state: RunState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_reason: Option<String>,
    pub attempt_count: RunAttemptCount,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_attempt_id: Option<AttemptId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_disposition: Option<TerminalDisposition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<RunDelivery>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_digest: Option<DigestValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_ref: Option<ReceiptId>,
    pub started_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionBag>,
}

impl AutomationRun {
    #[must_use]
    pub const fn state(&self) -> RunState {
        self.state
    }

    #[must_use]
    pub fn terminal_disposition(&self) -> Option<&TerminalDisposition> {
        self.terminal_disposition.as_ref()
    }

    #[must_use]
    pub fn finished_at(&self) -> Option<&Timestamp> {
        self.finished_at.as_ref()
    }

    fn validate(&self) -> Result<(), StringConstraintError> {
        if matches!(
            self.state,
            RunState::Succeeded
                | RunState::Failed
                | RunState::Cancelled
                | RunState::TimedOut
                | RunState::Ambiguous
        ) && (self.finished_at.is_none() || self.terminal_disposition.is_none())
        {
            return Err(StringConstraintError(
                "terminal runs require finishedAt and terminalDisposition",
            ));
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for AutomationRun {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            schema_version: SchemaVersion,
            run_id: RunId,
            occurrence_id: OccurrenceId,
            automation_id: AutomationId,
            automation_revision: PositiveInteger,
            binding: RunBinding,
            state: RunState,
            state_reason: Option<String>,
            attempt_count: RunAttemptCount,
            current_attempt_id: Option<AttemptId>,
            terminal_disposition: Option<TerminalDisposition>,
            delivery: Option<RunDelivery>,
            result_digest: Option<DigestValue>,
            receipt_ref: Option<ReceiptId>,
            started_at: Timestamp,
            finished_at: Option<Timestamp>,
            extensions: Option<ExtensionBag>,
        }

        let raw = Raw::deserialize(deserializer)?;
        let run = Self {
            schema_version: raw.schema_version,
            run_id: raw.run_id,
            occurrence_id: raw.occurrence_id,
            automation_id: raw.automation_id,
            automation_revision: raw.automation_revision,
            binding: raw.binding,
            state: raw.state,
            state_reason: raw.state_reason,
            attempt_count: raw.attempt_count,
            current_attempt_id: raw.current_attempt_id,
            terminal_disposition: raw.terminal_disposition,
            delivery: raw.delivery,
            result_digest: raw.result_digest,
            receipt_ref: raw.receipt_ref,
            started_at: raw.started_at,
            finished_at: raw.finished_at,
            extensions: raw.extensions,
        };
        run.validate().map_err(serde::de::Error::custom)?;
        Ok(run)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunBinding {
    pub familiar: FamiliarRef,
    pub authority: AuthorityBinding,
    pub runtime: RuntimeDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityBinding {
    pub principal: PrincipalRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication_class: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalDisposition {
    pub outcome: TerminalOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<TerminalFailureClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutcome {
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalFailureClass {
    LaunchRefused,
    RuntimeError,
    Timeout,
    CancelledByRequest,
    LeaseExpired,
    AmbiguousEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunDelivery {
    pub status: RunDeliveryStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_refs: Option<Vec<ArtifactRef>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunDeliveryStatus {
    None,
    Pending,
    Committed,
    Refused,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactRef {
    pub r#ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<DigestValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    Adopted,
    Dispatching,
    Started,
    Observing,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationAttempt {
    pub schema_version: SchemaVersion,
    pub attempt_id: AttemptId,
    pub run_id: RunId,
    pub occurrence_id: OccurrenceId,
    pub attempt_number: PositiveInteger,
    pub adoption_key: AdoptionKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prior_disposition: Option<PriorDisposition>,
    pub dispatch_fence: DispatchFence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_correlation: Option<WorkerCorrelation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_classification: Option<RetryClassification>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_observations: Option<Vec<LeaseObservation>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_cursors: Option<OutputCursors>,
    pub state: AttemptState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opened_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionBag>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PriorDisposition {
    pub attempt_number: PositiveInteger,
    pub outcome: PriorAttemptOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorAttemptOutcome {
    Failed,
    TimedOut,
    Ambiguous,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DispatchFence {
    pub occurrence_fence_generation: PositiveInteger,
    pub dispatch_generation: PositiveInteger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerCorrelation {
    pub worker_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adopted_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetryClassification {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification: Option<RetryClassificationKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eligible_classes: Option<Vec<RetryableClass>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryClassificationKind {
    Initial,
    AutomaticRetry,
    OperatorRetry,
    OperatorRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LeaseObservation {
    pub observed_at: Timestamp,
    pub heartbeat_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputCursors {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_cursor: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_cursor: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectClass {
    None,
    LocalRead,
    LocalWrite,
    ExternalRead,
    ExternalMutation,
    IrreversibleExternalMutation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationReceipt {
    pub schema_version: SchemaVersion,
    pub receipt_id: ReceiptId,
    pub automation_id: AutomationId,
    pub automation_revision: PositiveInteger,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition_digest: Option<DigestValue>,
    pub occurrence_id: OccurrenceId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurrence_fence_generation: Option<PositiveInteger>,
    pub run_id: RunId,
    pub attempt_id: AttemptId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_number: Option<PositiveInteger>,
    pub identity: FamiliarRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<ReceiptAuthority>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_digest: Option<DigestValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_digest: Option<DigestValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exercised_capabilities: Option<ExercisedCapabilities>,
    pub side_effect_class: SideEffectClass,
    pub outcome: ReceiptOutcome,
    pub produced_at: Timestamp,
    pub producer: ProducerIdentity,
    pub integrity: ReceiptIntegrity,
    pub privacy: ReceiptPrivacy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptAuthority {
    pub principal: PrincipalRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptOutcome {
    pub disposition: TerminalOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_failures: Option<Vec<PartialFailure>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_disposition: Option<ReceiptRecoveryDisposition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartialFailure {
    pub step: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovered: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptRecoveryDisposition {
    NotRequired,
    RecoveredInline,
    DeferredToOperator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptIntegrity {
    pub algorithm: DigestAlgorithm,
    pub canonicalization: Canonicalization,
    pub value: Sha256Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<ReceiptAuthentication>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReceiptAuthentication {
    None,
    ProducerHmac,
    Cosign,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptPrivacy {
    pub classification: PrivacyClassification,
    pub retention: RetentionClass,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandName {
    #[serde(rename = "definition.create.v1")]
    DefinitionCreate,
    #[serde(rename = "definition.revise.v1")]
    DefinitionRevise,
    #[serde(rename = "definition.activate.v1")]
    DefinitionActivate,
    #[serde(rename = "definition.pause.v1")]
    DefinitionPause,
    #[serde(rename = "definition.disable.v1")]
    DefinitionDisable,
    #[serde(rename = "definition.tombstone.v1")]
    DefinitionTombstone,
    #[serde(rename = "occurrence.runNow.v1")]
    OccurrenceRunNow,
    #[serde(rename = "occurrence.cancel.v1")]
    OccurrenceCancel,
    #[serde(rename = "run.cancel.v1")]
    RunCancel,
    #[serde(rename = "attempt.cancel.v1")]
    AttemptCancel,
    #[serde(rename = "attempt.retry.v1")]
    AttemptRetry,
    #[serde(rename = "occurrence.recover.v1")]
    OccurrenceRecover,
    #[serde(rename = "definition.list.v1")]
    DefinitionList,
    #[serde(rename = "definition.get.v1")]
    DefinitionGet,
    #[serde(rename = "run.history.v1")]
    RunHistory,
    #[serde(rename = "definition.health.v1")]
    DefinitionHealth,
    #[serde(rename = "events.read.v1")]
    EventsRead,
    #[serde(rename = "events.subscribe.v1")]
    EventsSubscribe,
    #[serde(rename = "legacy.import.v1")]
    LegacyImport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequest {
    schema_version: SchemaVersion,
    adoption_key: AdoptionKey,
    expected_revision: Option<PositiveInteger>,
    origin: CommandOrigin,
    intent: CommandIntent,
    payload: CommandPayload,
}

impl CommandRequest {
    pub fn new(
        schema_version: SchemaVersion,
        adoption_key: AdoptionKey,
        expected_revision: Option<PositiveInteger>,
        origin: CommandOrigin,
        intent: CommandIntent,
        payload: CommandPayload,
    ) -> Result<Self, StringConstraintError> {
        let revision_is_required = payload.requires_expected_revision();
        if revision_is_required != expected_revision.is_some() {
            return Err(StringConstraintError(
                "expectedRevision is required only for definition revise/activate/pause/disable/tombstone commands",
            ));
        }
        Ok(Self {
            schema_version,
            adoption_key,
            expected_revision,
            origin,
            intent,
            payload,
        })
    }

    #[must_use]
    pub const fn command(&self) -> CommandName {
        self.payload.command()
    }

    #[must_use]
    pub const fn expected_revision(&self) -> Option<PositiveInteger> {
        self.expected_revision
    }

    #[must_use]
    pub fn payload(&self) -> &CommandPayload {
        &self.payload
    }
}

impl Serialize for CommandRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count = if self.expected_revision.is_some() {
            7
        } else {
            6
        };
        let mut state = serializer.serialize_struct("CommandRequest", field_count)?;
        state.serialize_field("schemaVersion", &self.schema_version)?;
        state.serialize_field("command", &self.command())?;
        state.serialize_field("adoptionKey", &self.adoption_key)?;
        if let Some(expected_revision) = self.expected_revision {
            state.serialize_field("expectedRevision", &expected_revision)?;
        }
        state.serialize_field("origin", &self.origin)?;
        state.serialize_field("intent", &self.intent)?;
        state.serialize_field("payload", &self.payload)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for CommandRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            schema_version: SchemaVersion,
            command: CommandName,
            adoption_key: AdoptionKey,
            expected_revision: Option<PositiveInteger>,
            origin: CommandOrigin,
            intent: CommandIntent,
            payload: Value,
        }

        let raw = Raw::deserialize(deserializer)?;
        let payload = CommandPayload::from_value(raw.command, raw.payload)
            .map_err(serde::de::Error::custom)?;
        Self::new(
            raw.schema_version,
            raw.adoption_key,
            raw.expected_revision,
            raw.origin,
            raw.intent,
            payload,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum CommandPayload {
    DefinitionCreate(DefinitionMutationPayload),
    DefinitionRevise(DefinitionMutationPayload),
    DefinitionActivate(DefinitionTargetPayload),
    DefinitionPause(DefinitionTargetPayload),
    DefinitionDisable(DefinitionTargetPayload),
    DefinitionTombstone(DefinitionTargetPayload),
    OccurrenceRunNow(RunNowPayload),
    OccurrenceCancel(OccurrenceCancelPayload),
    RunCancel(RunCancelPayload),
    AttemptCancel(AttemptCancelPayload),
    AttemptRetry(AttemptRetryPayload),
    OccurrenceRecover(OccurrenceRecoverPayload),
    DefinitionList(DefinitionListPayload),
    DefinitionGet(DefinitionGetPayload),
    RunHistory(RunHistoryPayload),
    DefinitionHealth(DefinitionHealthPayload),
    EventsRead(EventsReadPayload),
    EventsSubscribe(EventsSubscribePayload),
    LegacyImport(LegacyImportPayload),
}

impl CommandPayload {
    fn from_value(command: CommandName, value: Value) -> Result<Self, serde_json::Error> {
        match command {
            CommandName::DefinitionCreate => {
                Ok(Self::DefinitionCreate(serde_json::from_value(value)?))
            }
            CommandName::DefinitionRevise => {
                Ok(Self::DefinitionRevise(serde_json::from_value(value)?))
            }
            CommandName::DefinitionActivate => {
                Ok(Self::DefinitionActivate(serde_json::from_value(value)?))
            }
            CommandName::DefinitionPause => {
                Ok(Self::DefinitionPause(serde_json::from_value(value)?))
            }
            CommandName::DefinitionDisable => {
                Ok(Self::DefinitionDisable(serde_json::from_value(value)?))
            }
            CommandName::DefinitionTombstone => {
                Ok(Self::DefinitionTombstone(serde_json::from_value(value)?))
            }
            CommandName::OccurrenceRunNow => {
                Ok(Self::OccurrenceRunNow(serde_json::from_value(value)?))
            }
            CommandName::OccurrenceCancel => {
                Ok(Self::OccurrenceCancel(serde_json::from_value(value)?))
            }
            CommandName::RunCancel => Ok(Self::RunCancel(serde_json::from_value(value)?)),
            CommandName::AttemptCancel => Ok(Self::AttemptCancel(serde_json::from_value(value)?)),
            CommandName::AttemptRetry => Ok(Self::AttemptRetry(serde_json::from_value(value)?)),
            CommandName::OccurrenceRecover => {
                Ok(Self::OccurrenceRecover(serde_json::from_value(value)?))
            }
            CommandName::DefinitionList => Ok(Self::DefinitionList(serde_json::from_value(value)?)),
            CommandName::DefinitionGet => Ok(Self::DefinitionGet(serde_json::from_value(value)?)),
            CommandName::RunHistory => Ok(Self::RunHistory(serde_json::from_value(value)?)),
            CommandName::DefinitionHealth => {
                Ok(Self::DefinitionHealth(serde_json::from_value(value)?))
            }
            CommandName::EventsRead => Ok(Self::EventsRead(serde_json::from_value(value)?)),
            CommandName::EventsSubscribe => {
                Ok(Self::EventsSubscribe(serde_json::from_value(value)?))
            }
            CommandName::LegacyImport => Ok(Self::LegacyImport(serde_json::from_value(value)?)),
        }
    }

    #[must_use]
    pub const fn command(&self) -> CommandName {
        match self {
            Self::DefinitionCreate(_) => CommandName::DefinitionCreate,
            Self::DefinitionRevise(_) => CommandName::DefinitionRevise,
            Self::DefinitionActivate(_) => CommandName::DefinitionActivate,
            Self::DefinitionPause(_) => CommandName::DefinitionPause,
            Self::DefinitionDisable(_) => CommandName::DefinitionDisable,
            Self::DefinitionTombstone(_) => CommandName::DefinitionTombstone,
            Self::OccurrenceRunNow(_) => CommandName::OccurrenceRunNow,
            Self::OccurrenceCancel(_) => CommandName::OccurrenceCancel,
            Self::RunCancel(_) => CommandName::RunCancel,
            Self::AttemptCancel(_) => CommandName::AttemptCancel,
            Self::AttemptRetry(_) => CommandName::AttemptRetry,
            Self::OccurrenceRecover(_) => CommandName::OccurrenceRecover,
            Self::DefinitionList(_) => CommandName::DefinitionList,
            Self::DefinitionGet(_) => CommandName::DefinitionGet,
            Self::RunHistory(_) => CommandName::RunHistory,
            Self::DefinitionHealth(_) => CommandName::DefinitionHealth,
            Self::EventsRead(_) => CommandName::EventsRead,
            Self::EventsSubscribe(_) => CommandName::EventsSubscribe,
            Self::LegacyImport(_) => CommandName::LegacyImport,
        }
    }

    const fn requires_expected_revision(&self) -> bool {
        matches!(
            self,
            Self::DefinitionRevise(_)
                | Self::DefinitionActivate(_)
                | Self::DefinitionPause(_)
                | Self::DefinitionDisable(_)
                | Self::DefinitionTombstone(_)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionMutationPayload {
    pub definition: AutomationDefinition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionTargetPayload {
    pub automation_id: AutomationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunNowPayload {
    pub automation_id: AutomationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bypass_eligibility: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OccurrenceCancelPayload {
    pub occurrence_id: OccurrenceId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunCancelPayload {
    pub run_id: RunId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttemptCancelPayload {
    pub attempt_id: AttemptId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttemptRetryPayload {
    pub run_id: RunId,
    pub prior_attempt_number: PositiveInteger,
    pub prior_disposition: RetryPriorDisposition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryPriorDisposition {
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OccurrenceRecoverPayload {
    pub occurrence_id: OccurrenceId,
    pub evidence_determination: EvidenceDetermination,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statement: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceDetermination {
    FailedDeterministic,
    RetryWithNewAttempt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionListPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle_state: Option<ListLifecycleState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<CommandListLimit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListLifecycleState {
    Draft,
    Paused,
    Active,
    Disabled,
    Invalid,
    Tombstoned,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionGetPayload {
    pub automation_id: AutomationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<PositiveInteger>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunHistoryPayload {
    pub automation_id: AutomationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurrence_id: Option<OccurrenceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<CommandListLimit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionHealthPayload {
    pub automation_id: AutomationId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventsReadPayload {
    pub stream: CommandStreamRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<EventReadLimit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventsSubscribePayload {
    pub stream: CommandStreamRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyImportPayload {
    pub source: LegacyImportSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LegacyImportSource {
    #[serde(rename = "codex-automation-toml")]
    CodexAutomationToml,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandOrigin {
    pub principal: PrincipalRef,
    pub channel: CommandChannel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandChannel {
    #[serde(rename = "daemon-ipc")]
    DaemonIpc,
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "control-action")]
    ControlAction,
    #[serde(rename = "cli")]
    Cli,
    #[serde(rename = "sdk")]
    Sdk,
    #[serde(rename = "cave")]
    Cave,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandIntent {
    pub statement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandOutcome {
    Committed,
    Replayed,
    Rejected,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandResponse {
    schema_version: SchemaVersion,
    command: CommandName,
    adoption_key: AdoptionKey,
    body: CommandResponseBody,
}

impl CommandResponse {
    #[must_use]
    pub fn new(
        schema_version: SchemaVersion,
        command: CommandName,
        adoption_key: AdoptionKey,
        body: CommandResponseBody,
    ) -> Self {
        Self {
            schema_version,
            command,
            adoption_key,
            body,
        }
    }

    #[must_use]
    pub const fn outcome(&self) -> CommandOutcome {
        self.body.outcome()
    }

    #[must_use]
    pub fn body(&self) -> &CommandResponseBody {
        &self.body
    }
}

impl Serialize for CommandResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("schemaVersion", &self.schema_version)?;
        map.serialize_entry("command", &self.command)?;
        map.serialize_entry("adoptionKey", &self.adoption_key)?;
        map.serialize_entry("outcome", &self.outcome())?;
        match &self.body {
            CommandResponseBody::Committed {
                revision,
                result,
                receipt_ref,
                event_ref,
            } => {
                if let Some(revision) = revision {
                    map.serialize_entry("revision", revision)?;
                }
                map.serialize_entry("result", result)?;
                if let Some(receipt_ref) = receipt_ref {
                    map.serialize_entry("receiptRef", receipt_ref)?;
                }
                if let Some(event_ref) = event_ref {
                    map.serialize_entry("eventRef", event_ref)?;
                }
            }
            CommandResponseBody::Replayed {
                replay,
                revision,
                result,
                receipt_ref,
                event_ref,
            } => {
                map.serialize_entry("replay", replay)?;
                if let Some(revision) = revision {
                    map.serialize_entry("revision", revision)?;
                }
                map.serialize_entry("result", result)?;
                if let Some(receipt_ref) = receipt_ref {
                    map.serialize_entry("receiptRef", receipt_ref)?;
                }
                if let Some(event_ref) = event_ref {
                    map.serialize_entry("eventRef", event_ref)?;
                }
            }
            CommandResponseBody::Rejected {
                revision,
                error,
                receipt_ref,
                event_ref,
            } => {
                if let Some(revision) = revision {
                    map.serialize_entry("revision", revision)?;
                }
                map.serialize_entry("error", error)?;
                if let Some(receipt_ref) = receipt_ref {
                    map.serialize_entry("receiptRef", receipt_ref)?;
                }
                if let Some(event_ref) = event_ref {
                    map.serialize_entry("eventRef", event_ref)?;
                }
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for CommandResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            schema_version: SchemaVersion,
            command: CommandName,
            adoption_key: AdoptionKey,
            outcome: CommandOutcome,
            replay: Option<ReplayMetadata>,
            revision: Option<PositiveInteger>,
            result: Option<JsonObject>,
            error: Option<ErrorEnvelope>,
            receipt_ref: Option<ReceiptId>,
            event_ref: Option<EventRef>,
        }

        let raw = Raw::deserialize(deserializer)?;
        let body = match raw.outcome {
            CommandOutcome::Committed => match (raw.replay, raw.result, raw.error) {
                (None, Some(result), None) => CommandResponseBody::Committed {
                    revision: raw.revision,
                    result,
                    receipt_ref: raw.receipt_ref,
                    event_ref: raw.event_ref,
                },
                _ => {
                    return Err(serde::de::Error::custom(
                        "committed responses require result and forbid error and replay",
                    ));
                }
            },
            CommandOutcome::Replayed => match (raw.replay, raw.result, raw.error) {
                (Some(replay), Some(result), None) => CommandResponseBody::Replayed {
                    replay,
                    revision: raw.revision,
                    result,
                    receipt_ref: raw.receipt_ref,
                    event_ref: raw.event_ref,
                },
                _ => {
                    return Err(serde::de::Error::custom(
                        "replayed responses require replay and result and forbid error",
                    ));
                }
            },
            CommandOutcome::Rejected => match (raw.replay, raw.result, raw.error) {
                (None, None, Some(error)) => CommandResponseBody::Rejected {
                    revision: raw.revision,
                    error,
                    receipt_ref: raw.receipt_ref,
                    event_ref: raw.event_ref,
                },
                _ => {
                    return Err(serde::de::Error::custom(
                        "rejected responses require error and forbid result and replay",
                    ));
                }
            },
        };
        Ok(Self::new(
            raw.schema_version,
            raw.command,
            raw.adoption_key,
            body,
        ))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandResponseBody {
    Committed {
        revision: Option<PositiveInteger>,
        result: JsonObject,
        receipt_ref: Option<ReceiptId>,
        event_ref: Option<EventRef>,
    },
    Replayed {
        replay: ReplayMetadata,
        revision: Option<PositiveInteger>,
        result: JsonObject,
        receipt_ref: Option<ReceiptId>,
        event_ref: Option<EventRef>,
    },
    Rejected {
        revision: Option<PositiveInteger>,
        error: ErrorEnvelope,
        receipt_ref: Option<ReceiptId>,
        event_ref: Option<EventRef>,
    },
}

impl CommandResponseBody {
    #[must_use]
    pub const fn outcome(&self) -> CommandOutcome {
        match self {
            Self::Committed { .. } => CommandOutcome::Committed,
            Self::Replayed { .. } => CommandOutcome::Replayed,
            Self::Rejected { .. } => CommandOutcome::Rejected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplayMetadata {
    pub first_committed_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventRef {
    pub stream: EventRefStream,
    pub sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    #[serde(rename = "definition.created")]
    DefinitionCreated,
    #[serde(rename = "definition.revised")]
    DefinitionRevised,
    #[serde(rename = "definition.activated")]
    DefinitionActivated,
    #[serde(rename = "definition.paused")]
    DefinitionPaused,
    #[serde(rename = "definition.disabled")]
    DefinitionDisabled,
    #[serde(rename = "definition.invalidated")]
    DefinitionInvalidated,
    #[serde(rename = "definition.tombstoned")]
    DefinitionTombstoned,
    #[serde(rename = "definition.imported")]
    DefinitionImported,
    #[serde(rename = "occurrence.transitioned")]
    OccurrenceTransitioned,
    #[serde(rename = "occurrence.misfire_recorded")]
    OccurrenceMisfireRecorded,
    #[serde(rename = "run.transitioned")]
    RunTransitioned,
    #[serde(rename = "attempt.transitioned")]
    AttemptTransitioned,
    #[serde(rename = "receipt.recorded")]
    ReceiptRecorded,
    #[serde(rename = "feed.snapshot")]
    FeedSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventEnvelope {
    pub schema_version: SchemaVersion,
    pub event_id: EventId,
    pub stream: StreamRef,
    pub sequence: u64,
    pub recorded_at: Timestamp,
    pub observed_at: Timestamp,
    pub producer: ProducerIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub causation: Option<EventCausation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_id: Option<AutomationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurrence_id: Option<OccurrenceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<AttemptId>,
    pub kind: EventKind,
    pub summary: EventSummary,
    pub payload: EventPayload,
    pub privacy: EventPrivacy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<DigestValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamRef {
    pub kind: StreamKind,
    pub id: EventStreamId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandStreamRef {
    pub kind: StreamKind,
    pub id: CommandStreamId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Automation,
    Occurrence,
    Run,
    Feed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventCausation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adoption_key: Option<AdoptionKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause_event_id: Option<CauseEventId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventPayload {
    DefinitionLifecycle(DefinitionLifecyclePayload),
    Transition(TransitionPayload),
    Misfire(MisfirePayload),
    Receipt(ReceiptPayload),
    Snapshot(SnapshotPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionLifecyclePayload {
    pub revision: PositiveInteger,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition_digest: Option<DigestValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle_state: Option<EventLifecycleState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventLifecycleState {
    Draft,
    Paused,
    Active,
    Disabled,
    Invalid,
    Tombstoned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransitionPayload {
    pub entity: TransitionEntity,
    pub from: String,
    pub to: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fence_generation: Option<PositiveInteger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_number: Option<PositiveInteger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_adoption_key: Option<AdoptionKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionEntity {
    Occurrence,
    Run,
    Attempt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MisfirePayload {
    pub disposition: MisfireDisposition,
    pub collapsed_slots: Vec<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptPayload {
    pub receipt_ref: ReceiptId,
    pub outcome: TerminalOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side_effect_class: Option<SideEffectClass>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotPayload {
    pub through_sequence: u64,
    /// Compacted state is explicitly an open, schema-defined JSON object.
    pub state: JsonObject,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<SnapshotReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotReason {
    RetentionCompaction,
    ManualSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventPrivacy {
    pub classification: PrivacyClassification,
    pub retention: RetentionClass,
}

impl fmt::Display for SchemaVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("coven.automations.v1")
    }
}
