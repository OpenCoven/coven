use crate::{
    AgentId, GuardrailStage, InvocationContext, InvocationEvent, ReviewOutcome, StopReason,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunFailureKind {
    Configuration,
    Session,
    ProposalReview,
    InputGuardrail,
    OutputGuardrail,
    Model,
    Tool,
    Handoff,
    InvalidResponse,
    Limit,
    /// A delegated invocation tried to use authority it was not granted.
    Authority,
    /// Selected context could not be resolved or did not match its digest.
    Context,
    /// A stop was acknowledged at a checkpoint with no tool in flight.
    Cancelled,
    /// A tool was in flight when a stop arrived and did not prove it
    /// produced no effect.
    TerminationUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEvent {
    RunStarted {
        invocation: InvocationContext,
        starting_agent: AgentId,
    },
    GuardrailChecked {
        invocation: InvocationContext,
        agent: AgentId,
        guardrail: String,
        stage: GuardrailStage,
        allowed: bool,
    },
    ModelRequested {
        invocation: InvocationContext,
        agent: AgentId,
        turn: usize,
    },
    /// A registered reviewer returned a verdict for a proposed tool call.
    /// Emitted once per reviewer consulted, before any `ToolStarted` for the
    /// same call. Carries the payload-free outcome only, never the reason.
    ProposalReviewed {
        invocation: InvocationContext,
        agent: AgentId,
        reviewer: String,
        tool: String,
        call_id: String,
        verdict: ReviewOutcome,
    },
    ToolStarted {
        invocation: InvocationContext,
        agent: AgentId,
        tool: String,
        call_id: String,
    },
    ToolCompleted {
        invocation: InvocationContext,
        agent: AgentId,
        tool: String,
        call_id: String,
    },
    Handoff {
        invocation: InvocationContext,
        from: AgentId,
        to: AgentId,
        name: String,
    },
    /// The runner observed a stop request. Emitted at most once, immediately
    /// before the terminal `RunFailed`.
    CancellationRequested {
        invocation: InvocationContext,
        agent: AgentId,
        reason: StopReason,
    },
    RunCompleted {
        invocation: InvocationContext,
        final_agent: AgentId,
        turns: usize,
        handoffs: usize,
    },
    RunFailed {
        invocation: InvocationContext,
        agent: AgentId,
        kind: RunFailureKind,
    },
}

impl RunEvent {
    pub const fn invocation(&self) -> &InvocationContext {
        match self {
            Self::RunStarted { invocation, .. }
            | Self::GuardrailChecked { invocation, .. }
            | Self::ModelRequested { invocation, .. }
            | Self::ProposalReviewed { invocation, .. }
            | Self::ToolStarted { invocation, .. }
            | Self::ToolCompleted { invocation, .. }
            | Self::Handoff { invocation, .. }
            | Self::CancellationRequested { invocation, .. }
            | Self::RunCompleted { invocation, .. }
            | Self::RunFailed { invocation, .. } => invocation,
        }
    }
}

pub trait RunObserver: Send + Sync {
    fn on_event(&self, event: &RunEvent);
}

#[derive(Debug, Default)]
pub struct NoopObserver;

impl RunObserver for NoopObserver {
    fn on_event(&self, _event: &RunEvent) {}
}

pub trait InvocationObserver: Send + Sync {
    fn on_event(&self, event: &InvocationEvent);
}

#[derive(Debug, Default)]
pub struct NoopInvocationObserver;

impl InvocationObserver for NoopInvocationObserver {
    fn on_event(&self, _event: &InvocationEvent) {}
}
