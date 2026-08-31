//! Provider-neutral agent runtime primitives for OpenCoven.
//!
//! The crate owns deterministic orchestration, not provider transport,
//! persistence, sandboxing, or user interface concerns.

mod agent;
mod error;
mod guardrail;
mod invocation;
mod loop_journal;
mod loop_runner;
mod model;
mod observer;
mod runner;
mod session;
mod tool;

pub use agent::{Agent, AgentId, Handoff};
pub use error::{BoxError, ConfigError, GuardrailStage, RunError, RunFailure};
pub use guardrail::{GuardrailVerdict, InputGuardrail, OutputGuardrail};
pub use invocation::{
    AgentRef, InvocationEvent, InvocationId, InvocationObserver, NoopInvocationObserver,
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
pub use observer::{NoopObserver, RunEvent, RunFailureKind, RunObserver};
pub use runner::{RunOptions, RunResult, Runner};
pub use session::{InMemorySession, SessionStore};
pub use tool::{Tool, ToolDefinition};
