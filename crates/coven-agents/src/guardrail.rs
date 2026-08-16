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
/// Checks the original user input before the starting agent runs.
///
/// Input guardrails attached only to handoff targets do not run in this MVP.
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
