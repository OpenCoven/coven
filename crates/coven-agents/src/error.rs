use thiserror::Error;

use crate::AgentId;

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
