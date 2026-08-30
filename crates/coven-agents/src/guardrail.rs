use async_trait::async_trait;

use crate::BoxError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardrailVerdict {
    Allow,
    Reject { reason: String },
}

impl GuardrailVerdict {
    pub fn reject(reason: impl Into<String>) -> Self {
        Self::Reject {
            reason: reason.into(),
        }
    }
}

#[async_trait]
/// Checks the bounded ingress of an agent before its first model turn.
///
/// The runner evaluates the starting agent's input guardrails against the
/// original user input before the run begins, and a handoff target's input
/// guardrails against the same original user input before the target's first
/// model turn, so entering an agent through a handoff cannot grant access that
/// direct entry would reject. Input guardrails never inspect a serialized
/// transcript; the structured task/context manifest for delegated invocations
/// is a separate contract (see OpenCoven/coven#804).
pub trait InputGuardrail<C>: Send + Sync
where
    C: Sync,
{
    fn name(&self) -> &str;

    async fn check(&self, input: &str, context: &C) -> Result<GuardrailVerdict, BoxError>;
}

#[async_trait]
/// Checks the final output produced by the agent that completes the run.
///
/// Output guardrails on intermediate agents do not run in this MVP.
pub trait OutputGuardrail<C>: Send + Sync
where
    C: Sync,
{
    fn name(&self) -> &str;

    async fn check(&self, output: &str, context: &C) -> Result<GuardrailVerdict, BoxError>;
}
