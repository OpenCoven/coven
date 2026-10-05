//! Provider-neutral, in-process agent behavior primitives for OpenCoven.
//!
//! The crate owns bounded model/tool loops and local control transfer.
//! Host adapters supply provider transport, session persistence, and policy;
//! durable invocation orchestration remains outside this crate.

mod agent;
mod error;
mod guardrail;
mod invocation;
mod loop_journal;
mod loop_runner;
mod model;
mod observer;
mod review;
mod runner;
mod session;
mod tool;

pub use agent::{Agent, AgentId, Handoff};
pub use error::{BoxError, ConfigError, GuardrailStage, RunError, RunFailure};
pub use guardrail::{GuardrailVerdict, InputGuardrail, OutputGuardrail};
pub use invocation::{
    AgentRef, AgentRefError, AgentRevision, InvocationContext, InvocationEvent,
    InvocationEventKind, InvocationEventVersion, InvocationFailureKind, InvocationId,
    InvocationRequest, InvocationSource,
};
pub use loop_journal::FileLoopJournal;
pub use loop_runner::{
    GoalLoopRunner, InMemoryLoopJournal, LoopAttempt, LoopCheckpoint, LoopCheckpointStatus,
    LoopControl, LoopError, LoopEvaluator, LoopJournal, LoopOptions, LoopReconciler,
    LoopReconciliation, LoopRecoveryFence, LoopRunResult,
};
pub use model::{
    HandoffCall, HandoffDefinition, Model, ModelAction, ModelRequest, ModelResponse, RunItem,
    ToolCall,
};
pub use observer::{
    InvocationObserver, NoopInvocationObserver, NoopObserver, RunEvent, RunFailureKind, RunObserver,
};
pub use review::{ProposalReview, ReviewOutcome, ReviewVerdict, ToolProposal};
pub use runner::{RunOptions, RunResult, Runner};
pub use session::{InMemorySession, SessionStore};
pub use tool::{Tool, ToolDefinition};
