use crate::{AgentId, GuardrailStage, InvocationContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunFailureKind {
    Configuration,
    Session,
    InputGuardrail,
    OutputGuardrail,
    Model,
    Tool,
    Handoff,
    InvalidResponse,
    Limit,
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
            | Self::ToolStarted { invocation, .. }
            | Self::ToolCompleted { invocation, .. }
            | Self::Handoff { invocation, .. }
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
