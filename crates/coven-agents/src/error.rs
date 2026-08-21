use thiserror::Error;

use crate::{AgentId, RunItem};

pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardrailStage {
    Input,
    Output,
}

impl std::fmt::Display for GuardrailStage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input => formatter.write_str("input"),
            Self::Output => formatter.write_str("output"),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("at least one agent is required")]
    NoAgents,
    #[error("agent id `{0}` is registered more than once")]
    DuplicateAgent(AgentId),
    #[error("agent `{agent}` registers tool `{tool}` more than once")]
    DuplicateTool { agent: AgentId, tool: String },
    #[error("agent `{agent}` registers handoff `{handoff}` more than once")]
    DuplicateHandoff { agent: AgentId, handoff: String },
    #[error("agent `{agent}` handoff `{handoff}` targets unknown agent `{target}`")]
    UnknownHandoffTarget {
        agent: AgentId,
        handoff: String,
        target: AgentId,
    },
}

#[derive(Debug, Error)]
pub enum RunError {
    #[error("starting agent `{0}` is not registered")]
    UnknownStartingAgent(AgentId),
    #[error("runner configuration became invalid: {reason}")]
    InvalidConfiguration { reason: String },
    #[error("session id was provided but no session store is configured")]
    SessionUnavailable,
    #[error("session {operation} failed")]
    SessionFailed {
        operation: &'static str,
        #[source]
        source: BoxError,
    },
    #[error("model call for agent `{agent}` failed")]
    ModelFailed {
        agent: AgentId,
        #[source]
        source: BoxError,
    },
    #[error("{stage} guardrail `{guardrail}` for agent `{agent}` failed")]
    GuardrailFailed {
        agent: AgentId,
        guardrail: String,
        stage: GuardrailStage,
        #[source]
        source: BoxError,
    },
    #[error("{stage} guardrail `{guardrail}` for agent `{agent}` rejected the run: {reason}")]
    GuardrailRejected {
        agent: AgentId,
        guardrail: String,
        stage: GuardrailStage,
        reason: String,
    },
    #[error("agent `{agent}` requested unknown tool `{tool}`")]
    UnknownTool { agent: AgentId, tool: String },
    #[error("agent `{agent}` reused tool call id `{call_id}`")]
    DuplicateToolCallId { agent: AgentId, call_id: String },
    #[error("tool `{tool}` for agent `{agent}` failed")]
    ToolFailed {
        agent: AgentId,
        tool: String,
        #[source]
        source: BoxError,
    },
    #[error("agent `{agent}` requested unknown handoff `{handoff}`")]
    UnknownHandoff { agent: AgentId, handoff: String },
    #[error("agent `{agent}` returned an invalid model response: {reason}")]
    InvalidModelResponse { agent: AgentId, reason: String },
    #[error("run exceeded the maximum of {limit} model turns")]
    MaxTurnsExceeded { limit: usize },
    #[error("run exceeded the maximum of {limit} handoffs")]
    MaxHandoffsExceeded { limit: usize },
}

/// A failed run together with the transcript the run had produced when it
/// failed.
///
/// A failing tool does not undo the work that preceded it: the user message,
/// the assistant message, and the tool call that failed are all real transcript
/// items. Returning a bare error would discard them, leaving callers unable to
/// render the conversation, log it, or decide whether to retry. Every failure
/// path therefore reports its partial transcript.
///
/// The runner never writes `new_items` to the session store on failure, so a
/// failed run cannot silently become durable history. Persisting a partial
/// transcript is a deliberate caller decision.
#[derive(Debug)]
pub struct RunFailure {
    pub error: RunError,
    /// Items produced during this run before it failed, in order. Always begins
    /// with the user message that started the run.
    pub new_items: Vec<RunItem>,
    /// Model turns started before the failure. Zero when the run failed before
    /// the first turn.
    pub turns: usize,
    /// Handoffs performed before the failure.
    pub handoffs: usize,
}

impl RunFailure {
    /// Discards the partial transcript and keeps only the error.
    pub fn into_error(self) -> RunError {
        self.error
    }
}

impl std::fmt::Display for RunFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.error, formatter)
    }
}

impl std::error::Error for RunFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_failure_exposes_the_wrapped_error_as_its_source() {
        let failure = RunFailure {
            error: RunError::SessionUnavailable,
            new_items: Vec::new(),
            turns: 0,
            handoffs: 0,
        };

        let source = std::error::Error::source(&failure).expect("wrapped RunError source");
        assert_eq!(
            source.to_string(),
            "session id was provided but no session store is configured"
        );
    }
}
