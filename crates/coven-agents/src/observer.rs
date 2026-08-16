use crate::{AgentId, GuardrailStage};

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
        starting_agent: AgentId,
    },
    GuardrailChecked {
        agent: AgentId,
        guardrail: String,
        stage: GuardrailStage,
        allowed: bool,
    },
    ModelRequested {
        agent: AgentId,
        turn: usize,
    },
    ToolStarted {
        agent: AgentId,
        tool: String,
        call_id: String,
    },
    ToolCompleted {
        agent: AgentId,
        tool: String,
        call_id: String,
    },
    Handoff {
        from: AgentId,
        to: AgentId,
        name: String,
    },
    RunCompleted {
        final_agent: AgentId,
        turns: usize,
        handoffs: usize,
    },
    RunFailed {
        agent: AgentId,
        kind: RunFailureKind,
    },
}

pub trait RunObserver: Send + Sync {
    fn on_event(&self, event: &RunEvent);
}

#[derive(Debug, Default)]
pub struct NoopObserver;

impl RunObserver for NoopObserver {
    fn on_event(&self, _event: &RunEvent) {}
}
