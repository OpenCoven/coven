use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
