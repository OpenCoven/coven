use std::{fmt, str::FromStr};

use serde::{de, Deserialize, Deserializer, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::AgentId;

const MAX_AGENT_REF_COMPONENT_BYTES: usize = 127;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AgentRefError {
    #[error("agent {field} must not be empty")]
    Empty { field: &'static str },
    #[error("agent {field} exceeds {MAX_AGENT_REF_COMPONENT_BYTES} bytes")]
    TooLong { field: &'static str },
    #[error("agent {field} must not contain whitespace or control characters")]
    InvalidCharacter { field: &'static str },
}

fn validate_agent_ref_component(value: &str, field: &'static str) -> Result<(), AgentRefError> {
    if value.is_empty() {
        return Err(AgentRefError::Empty { field });
    }
    if value.len() > MAX_AGENT_REF_COMPONENT_BYTES {
        return Err(AgentRefError::TooLong { field });
    }
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(AgentRefError::InvalidCharacter { field });
    }
    Ok(())
}

/// Immutable revision identifier for a registered agent implementation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct AgentRevision(String);

impl AgentRevision {
    pub fn new(value: impl Into<String>) -> Result<Self, AgentRefError> {
        let value = value.into();
        validate_agent_ref_component(&value, "revision")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for AgentRevision {
    type Err = AgentRefError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for AgentRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Validated logical identity and optional immutable revision of an agent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct AgentRef {
    id: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revision: Option<AgentRevision>,
}

impl AgentRef {
    pub fn new(id: impl Into<String>) -> Result<Self, AgentRefError> {
        Self::from_parts(id.into(), None)
    }

    pub fn with_revision(
        id: impl Into<String>,
        revision: impl Into<String>,
    ) -> Result<Self, AgentRefError> {
        Self::from_parts(id.into(), Some(AgentRevision::new(revision)?))
    }

    fn from_parts(id: String, revision: Option<AgentRevision>) -> Result<Self, AgentRefError> {
        validate_agent_ref_component(&id, "id")?;
        Ok(Self {
            id: AgentId::new(id),
            revision,
        })
    }

    pub fn id(&self) -> &AgentId {
        &self.id
    }

    pub fn revision(&self) -> Option<&AgentRevision> {
        self.revision.as_ref()
    }
}

impl fmt::Display for AgentRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.id.fmt(formatter)?;
        if let Some(revision) = &self.revision {
            write!(formatter, "@{}", revision.as_str())?;
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for AgentRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawAgentRef {
            id: String,
            #[serde(default)]
            revision: Option<AgentRevision>,
        }

        let raw = RawAgentRef::deserialize(deserializer)?;
        Self::from_parts(raw.id, raw.revision).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InvocationEventVersion {
    #[serde(rename = "coven.agent-invocation-event.v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InvocationSource {
    Caller,
    Agent { agent: AgentRef },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InvocationEventKind {
    Started {},
    ControlTransferred {
        from: AgentRef,
        to: AgentRef,
        name: String,
    },
    Completed {
        final_agent: AgentRef,
        turns: usize,
        control_transfers: usize,
    },
    Failed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failed_at: Option<AgentRef>,
        kind: InvocationFailureKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum InvocationFailureKind {
    Configuration,
    Session,
    InputGuardrail,
    OutputGuardrail,
    Model,
    Tool,
    ControlTransfer,
    InvalidResponse,
    Limit,
}

/// Versioned metadata-only event for one provider-neutral agent invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationEvent {
    contract: InvocationEventVersion,
    pub invocation: InvocationContext,
    pub source: InvocationSource,
    pub requested_target: AgentRef,
    pub event: InvocationEventKind,
}

impl InvocationEvent {
    pub const fn new(
        invocation: InvocationContext,
        source: InvocationSource,
        requested_target: AgentRef,
        event: InvocationEventKind,
    ) -> Self {
        Self {
            contract: InvocationEventVersion::V1,
            invocation,
            source,
            requested_target,
            event,
        }
    }

    pub const fn contract(&self) -> InvocationEventVersion {
        self.contract
    }
}

/// Validated request metadata for the canonical invocation event stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationRequest {
    pub invocation: InvocationContext,
    pub source: InvocationSource,
    pub requested_target: AgentRef,
}

impl InvocationRequest {
    pub const fn new(
        invocation: InvocationContext,
        source: InvocationSource,
        requested_target: AgentRef,
    ) -> Self {
        Self {
            invocation,
            source,
            requested_target,
        }
    }
}

/// Stable correlation identity for one runner invocation.
///
/// This identifies the local behavior run only. It does not imply durable
/// adoption, idempotency, authority, executor ownership, or retry safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InvocationId(Uuid);

impl InvocationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for InvocationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for InvocationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for InvocationId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

/// Correlation carried unchanged through one invocation and its events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationContext {
    pub id: InvocationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<InvocationId>,
}

impl InvocationContext {
    pub const fn root(id: InvocationId) -> Self {
        Self {
            id,
            parent_id: None,
        }
    }

    pub const fn child(id: InvocationId, parent_id: InvocationId) -> Self {
        Self {
            id,
            parent_id: Some(parent_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_context_round_trips_with_optional_parent() {
        let context = InvocationContext::child(
            "11111111-1111-4111-8111-111111111111".parse().unwrap(),
            "22222222-2222-4222-8222-222222222222".parse().unwrap(),
        );

        let encoded = serde_json::to_string(&context).unwrap();
        let decoded: InvocationContext = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, context);
        assert!(encoded.contains("\"parent_id\""));
    }

    #[test]
    fn root_serialization_omits_parent_and_malformed_ids_are_rejected() {
        let context =
            InvocationContext::root("33333333-3333-4333-8333-333333333333".parse().unwrap());

        let encoded = serde_json::to_string(&context).unwrap();

        assert!(!encoded.contains("parent_id"));
        assert!("not-an-invocation".parse::<InvocationId>().is_err());
    }
}
