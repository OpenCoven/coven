use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{AgentId, ConfigError, RunFailureKind};

const MAX_IDENTITY_BYTES: usize = 127;

/// Validates one identity component of an invocation reference.
///
/// Invocation identity is correlation data that survives logging, queues, and
/// transport, so unrestricted strings are not sufficient: an empty component
/// correlates nothing, an oversized one truncates, and unbounded whitespace or
/// control characters make ids ambiguous across systems that trim or escape.
pub(crate) fn validate_identity_component(value: &str) -> Result<(), &'static str> {
    if value.is_empty() {
        return Err("must not be empty");
    }
    if value.len() > MAX_IDENTITY_BYTES {
        return Err("must not exceed 127 bytes");
    }
    if value.trim() != value {
        return Err("must not start or end with whitespace");
    }
    if value.chars().any(char::is_control) {
        return Err("must not contain control characters");
    }
    Ok(())
}

/// Stable identity for one invocation, from start through its terminal event.
///
/// A `Runner` run emits exactly one `InvocationStarted` event and exactly one
/// terminal `InvocationCompleted` or `InvocationFailed` event, and every
/// event in between carries the same [`InvocationId`]. Nested work correlates
/// to its parent through the optional parent identity on
/// [`RunOptions::parent_invocation`](crate::RunOptions::parent_invocation),
/// so observers can reconstruct parent/child relationships without parsing
/// model prose.
///
/// Construction is validated: ids must not be empty, must not exceed 127
/// bytes, must not start or end with whitespace, and must not contain control
/// characters. [`InvocationId::generate`] exists for callers that do not have
/// a durable identity; it is unique within one process lifetime only, so
/// durable callers must supply their own stable id until durable invocation
/// persistence lands.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InvocationId(String);

impl InvocationId {
    /// Validates and creates an invocation id.
    pub fn try_new(value: impl Into<String>) -> Result<Self, ConfigError> {
        let value = value.into();
        if let Err(reason) = validate_identity_component(&value) {
            return Err(ConfigError::InvalidInvocationId { reason });
        }
        Ok(Self(value))
    }

    /// Generates a process-scoped invocation id.
    ///
    /// The id is unique within one process lifetime and stable for the
    /// duration of the run, but it is not durable across processes. Callers
    /// that need an identity to survive a restart must pass their own id
    /// through run options instead of relying on this generator.
    pub fn generate() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let counter = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or_default();
        Self(format!("inv_{nanos:016x}_{counter:04x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InvocationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A validated reference to an agent target with an optional revision pin.
///
/// Agent identity for routing and delegation is more than an unrestricted
/// logical string: [`AgentRef`] validates its target and can pin a revision,
/// so a future delegation contract can bind a child invocation to the exact
/// agent definition its parent intended. The revision is optional while
/// agents carry no revision metadata; it must be present once routing depends
/// on it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AgentRef {
    agent: AgentId,
    revision: Option<String>,
}

impl AgentRef {
    /// Validates and creates a reference without a revision pin.
    pub fn try_new(agent: AgentId) -> Result<Self, ConfigError> {
        Self::validate_component("agent", agent.as_str())?;
        Ok(Self {
            agent,
            revision: None,
        })
    }

    /// Validates and pins a revision on the reference.
    pub fn with_revision(self, revision: impl Into<String>) -> Result<Self, ConfigError> {
        let revision = revision.into();
        Self::validate_component("revision", &revision)?;
        Ok(Self {
            revision: Some(revision),
            ..self
        })
    }

    fn validate_component(component: &'static str, value: &str) -> Result<(), ConfigError> {
        if let Err(reason) = validate_identity_component(value) {
            return Err(ConfigError::InvalidAgentRef { component, reason });
        }
        Ok(())
    }

    /// The referenced agent id.
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// The pinned revision, when the reference carries one.
    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }

    /// Builds a reference for an agent id the runner already resolved.
    ///
    /// Registered agent ids were accepted by `Runner::new`, and a requested
    /// but unregistered starting agent is reported verbatim in terminal
    /// failure events, so this path deliberately skips re-validation.
    pub(crate) fn from_agent(agent: AgentId) -> Self {
        Self {
            agent,
            revision: None,
        }
    }
}

impl fmt::Display for AgentRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.revision {
            Some(revision) => write!(formatter, "{revision}@{}", self.agent),
            None => self.agent.fmt(formatter),
        }
    }
}

/// Canonical invocation telemetry.
///
/// Every invocation opens with exactly one [`InvocationEvent::InvocationStarted`]
/// and closes with exactly one terminal
/// [`InvocationEvent::InvocationCompleted`] or
/// [`InvocationEvent::InvocationFailed`] event, so an observer can pair
/// per-invocation state without leaking or orphaning it. Each event carries
/// the invocation id that correlates it to the run; the started event carries
/// the optional parent identity for nested work.
///
/// Attempt and executor binding is deliberately absent: the behavior loop has
/// no executor concept yet, and the loop journal already records
/// process-scoped attempt ids for journaled loops. The binding arrives with
/// the executor seam, not before.
///
/// `RunObserver` remains the legacy per-run telemetry stream during
/// migration; these canonical events are the contract later slices fold it
/// into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationEvent {
    /// The invocation started. The first event of every invocation.
    InvocationStarted {
        invocation: InvocationId,
        /// The parent invocation, when this is nested work.
        parent: Option<InvocationId>,
        /// The requested target agent.
        target: AgentRef,
    },
    /// The legacy pointer-swap handoff transferred control to another agent.
    ///
    /// This is control transfer inside one invocation, not durable A2A
    /// delegation: no child invocation exists, the transcript continues
    /// in place, and no authority boundary is crossed. The explicit
    /// delegation contract replaces it in a later slice.
    ControlTransferred {
        invocation: InvocationId,
        from: AgentRef,
        to: AgentRef,
    },
    /// The invocation completed. A terminal event.
    InvocationCompleted {
        invocation: InvocationId,
        /// The agent that produced the final output.
        final_target: AgentRef,
        turns: usize,
        control_transfers: usize,
    },
    /// The invocation failed. A terminal event.
    InvocationFailed {
        invocation: InvocationId,
        /// The requested target, reported verbatim even when the failure
        /// happened before the runner resolved it.
        target: AgentRef,
        kind: RunFailureKind,
    },
}

/// Receives canonical [`InvocationEvent`] telemetry.
pub trait InvocationObserver: Send + Sync {
    fn on_invocation_event(&self, event: &InvocationEvent);
}

/// An [`InvocationObserver`] that discards every event.
#[derive(Debug, Default)]
pub struct NoopInvocationObserver;

impl InvocationObserver for NoopInvocationObserver {
    fn on_invocation_event(&self, _event: &InvocationEvent) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_id_rejects_invalid_values() {
        let oversized = "a".repeat(MAX_IDENTITY_BYTES + 1);
        assert_eq!(
            InvocationId::try_new("").unwrap_err().to_string(),
            "invocation id must not be empty"
        );
        assert_eq!(
            InvocationId::try_new(oversized).unwrap_err().to_string(),
            "invocation id must not exceed 127 bytes"
        );
        assert_eq!(
            InvocationId::try_new(" inv").unwrap_err().to_string(),
            "invocation id must not start or end with whitespace"
        );
        assert_eq!(
            InvocationId::try_new("inv\nx").unwrap_err().to_string(),
            "invocation id must not contain control characters"
        );
    }

    #[test]
    fn generated_invocation_ids_are_unique_and_valid() {
        let first = InvocationId::generate();
        let second = InvocationId::generate();
        assert_ne!(first, second);
        assert_eq!(InvocationId::try_new(first.as_str()), Ok(first));
    }

    #[test]
    fn agent_ref_requires_a_valid_agent_and_revision() {
        assert_eq!(
            AgentRef::try_new(AgentId::from("")).unwrap_err().to_string(),
            "agent reference agent must not be empty"
        );
        let reference = AgentRef::try_new(AgentId::from("triage")).unwrap();
        assert_eq!(reference.agent(), &AgentId::from("triage"));
        assert_eq!(reference.revision(), None);
        let rejection = reference.clone().with_revision("   ").unwrap_err();
        assert_eq!(
            rejection.to_string(),
            "agent reference revision must not start or end with whitespace"
        );
    }

    #[test]
    fn agent_ref_displays_the_revision_pin() {
        let reference = AgentRef::try_new(AgentId::from("triage"))
            .unwrap()
            .with_revision("v7")
            .unwrap();
        assert_eq!(reference.revision(), Some("v7"));
        assert_eq!(reference.to_string(), "triage@v7");
        assert_eq!(
            AgentRef::try_new(AgentId::from("triage")).unwrap().to_string(),
            "triage"
        );
    }
}
