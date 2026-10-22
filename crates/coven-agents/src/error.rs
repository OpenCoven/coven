use thiserror::Error;

use crate::{
    AgentId, AgentRef, AgentRefError, ContentDigest, InvocationContext, InvocationId,
    RunFailureKind, RunItem, StopReason,
};

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
    #[error("agent `{agent}` does not form a valid agent reference")]
    InvalidAgentRef {
        agent: AgentId,
        #[source]
        source: AgentRefError,
    },
    #[error("agent id `{0}` is registered more than once")]
    DuplicateAgent(AgentId),
    #[error("agent `{agent}` registers tool `{tool}` more than once")]
    DuplicateTool { agent: AgentId, tool: String },
    #[error("agent `{agent}` registers handoff `{handoff}` more than once")]
    DuplicateHandoff { agent: AgentId, handoff: String },
    #[error("agent `{agent}` registers proposal review `{name}` more than once")]
    DuplicateProposalReview { agent: AgentId, name: String },
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
    #[error("starting agent reference `{requested}` does not match registered `{registered}`")]
    AgentRevisionMismatch {
        requested: Box<AgentRef>,
        registered: Box<AgentRef>,
    },
    #[error("explicit invocation target `{agent}` requires an immutable registered revision")]
    AgentRevisionRequired { agent: AgentId },
    #[error("starting agent `{agent}` does not form a valid agent reference")]
    InvalidAgentRef {
        agent: AgentId,
        #[source]
        source: AgentRefError,
    },
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
    #[error("proposal review `{reviewer}` for agent `{agent}` failed")]
    ProposalReviewFailed {
        agent: AgentId,
        reviewer: String,
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
    #[error("agent `{agent}` requested tool `{tool}` outside its delegated authority")]
    UnauthorizedTool { agent: AgentId, tool: String },
    #[error("context reference `{reference}` could not be resolved")]
    ContextFailed {
        reference: String,
        #[source]
        source: BoxError,
    },
    #[error("context reference `{reference}` resolved to items whose digest does not match")]
    ContextDigestMismatch {
        reference: String,
        expected: ContentDigest,
        actual: ContentDigest,
    },
    #[error("final output is {bytes} bytes; the result contract allows {limit}")]
    OutputTooLarge { limit: usize, bytes: usize },
    #[error("invocation stopped at agent `{agent}` with no tool in flight: {reason}")]
    Cancelled { agent: AgentId, reason: StopReason },
    #[error(
        "tool call `{call_id}` for agent `{agent}` was in flight when the invocation stopped \
         ({reason}); whether it produced an effect is unknown"
    )]
    TerminationUnknown {
        agent: AgentId,
        call_id: String,
        reason: StopReason,
        #[source]
        source: BoxError,
    },
    #[error(
        "tool call `{call_id}` for agent `{agent}` reported a descendant whose termination is \
         unknown; reconcile it before any retry"
    )]
    RecoveryRequired {
        agent: AgentId,
        call_id: String,
        #[source]
        source: BoxError,
    },
    #[error("parent invocation `{parent}` already has {limit} delegated children running")]
    ChildLimitExceeded { parent: InvocationId, limit: usize },
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
    /// Stable identity shared by every event and outcome for this invocation.
    pub invocation: Box<InvocationContext>,
    pub error: RunError,
    /// Items produced during this run before it failed, in order. Always begins
    /// with the user message that started the run.
    pub new_items: Box<[RunItem]>,
    /// Model turns started before the failure. Zero when the run failed before
    /// the first turn.
    pub turns: usize,
    /// Handoffs performed before the failure.
    pub handoffs: usize,
}

impl RunError {
    /// The failure kind the runner reports for this error in both observer
    /// streams. Every error has exactly one kind.
    pub fn failure_kind(&self) -> RunFailureKind {
        match self {
            Self::UnknownStartingAgent(_)
            | Self::AgentRevisionMismatch { .. }
            | Self::AgentRevisionRequired { .. }
            | Self::InvalidAgentRef { .. }
            | Self::InvalidConfiguration { .. } => RunFailureKind::Configuration,
            Self::SessionUnavailable | Self::SessionFailed { .. } => RunFailureKind::Session,
            Self::ProposalReviewFailed { .. } => RunFailureKind::ProposalReview,
            Self::ModelFailed { .. } => RunFailureKind::Model,
            Self::GuardrailFailed { stage, .. } | Self::GuardrailRejected { stage, .. } => {
                match stage {
                    GuardrailStage::Input => RunFailureKind::InputGuardrail,
                    GuardrailStage::Output => RunFailureKind::OutputGuardrail,
                }
            }
            Self::UnknownTool { .. } | Self::ToolFailed { .. } => RunFailureKind::Tool,
            Self::UnauthorizedTool { .. } => RunFailureKind::Authority,
            Self::ContextFailed { .. } | Self::ContextDigestMismatch { .. } => {
                RunFailureKind::Context
            }
            Self::OutputTooLarge { .. }
            | Self::ChildLimitExceeded { .. }
            | Self::MaxTurnsExceeded { .. }
            | Self::MaxHandoffsExceeded { .. } => RunFailureKind::Limit,
            Self::Cancelled { .. } => RunFailureKind::Cancelled,
            Self::TerminationUnknown { .. } => RunFailureKind::TerminationUnknown,
            Self::RecoveryRequired { .. } => RunFailureKind::RecoveryRequired,
            Self::DuplicateToolCallId { .. } | Self::InvalidModelResponse { .. } => {
                RunFailureKind::InvalidResponse
            }
            Self::UnknownHandoff { .. } => RunFailureKind::Handoff,
        }
    }

    /// Whether this failure leaves an effect whose outcome is unknown, here
    /// or in a descendant, so that work must be reconciled before any retry.
    pub fn leaves_unknown_effect(&self) -> bool {
        matches!(
            self,
            Self::TerminationUnknown { .. } | Self::RecoveryRequired { .. }
        )
    }
}

/// Whether `error`, or any error in its source chain, reports a run or durable
/// invocation with an unknown effect. Wrapping a child failure in another error keeps
/// it visible as long as the wrapper reports its source.
pub(crate) fn chain_leaves_unknown_effect(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error
            .downcast_ref::<RunError>()
            .is_some_and(RunError::leaves_unknown_effect)
            || error
                .downcast_ref::<crate::DurableInvocationError>()
                .is_some_and(crate::DurableInvocationError::leaves_unknown_effect)
        {
            return true;
        }
        current = error.source();
    }
    false
}

impl RunFailure {
    /// The failure kind both observer streams reported for this run.
    pub fn kind(&self) -> RunFailureKind {
        self.error.failure_kind()
    }

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
            invocation: Box::new(InvocationContext::root(Default::default())),
            error: RunError::SessionUnavailable,
            new_items: Vec::new().into_boxed_slice(),
            turns: 0,
            handoffs: 0,
        };

        let source = std::error::Error::source(&failure).expect("wrapped RunError source");
        assert_eq!(
            source.to_string(),
            "session id was provided but no session store is configured"
        );
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn run_failure_stays_below_clippys_large_error_threshold() {
        assert!(
            std::mem::size_of::<RunFailure>() < 128,
            "RunFailure grew to {} bytes",
            std::mem::size_of::<RunFailure>()
        );
    }
}
