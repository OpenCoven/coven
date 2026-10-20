//! Provider-neutral, in-process agent behavior primitives for OpenCoven.
//!
//! The crate owns bounded model/tool loops, local control transfer, and
//! bounded local delegation with attenuated authority. Host adapters supply
//! provider transport, session persistence, context resolution, and policy.
//! The crate records local invocations durably, at most once per id; distributed
//! orchestration, placement and retries remain outside it.

mod agent;
mod agent_loop;
mod delegation;
mod durable_runner;
mod durable_store;
mod error;
mod guardrail;
mod invocation;
mod invocation_journal;
mod lifecycle;
mod loop_journal;
mod loop_runner;
mod model;
mod observer;
mod review;
mod runner;
mod session;
mod tool;

pub use agent::{Agent, AgentId, Handoff};
pub use delegation::{
    AuthorityError, AuthorityScope, ContentDigest, ContextManifest, ContextProvenance, ContextRef,
    ContextResolver, DelegationBudget, DelegationDigests, DelegationGrant, DelegationSpec,
    DelegationSpecError, PreparedDelegation, ProjectRef, ResultContract, TaskInput,
    MAX_AUTHORITY_ENTRIES, MAX_CONTEXT_REFS, MAX_TASK_INPUT_BYTES,
};
pub use durable_runner::{
    DurableInvocation, DurableInvocationError, DurableInvocationRunner, InvocationDisposition,
};
pub use error::{BoxError, ConfigError, GuardrailStage, RunError, RunFailure};
pub use guardrail::{GuardrailVerdict, InputGuardrail, OutputGuardrail};
pub use invocation::{
    AgentRef, AgentRefError, AgentRevision, InvocationContext, InvocationEvent,
    InvocationEventKind, InvocationEventVersion, InvocationFailureKind, InvocationId,
    InvocationRequest, InvocationSource,
};
pub use invocation_journal::{
    invocation_request_digest, FileInvocationJournal, InMemoryInvocationJournal, InvocationAttempt,
    InvocationJournal, InvocationRecord, InvocationState,
};
pub use lifecycle::{
    CancellationToken, Clock, ManualClock, NotStarted, RunControls, RunSignal, StopReason,
    SystemClock,
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
pub use runner::{
    DelegationDisposition, DelegationFailure, DelegationResult, RunOptions, RunResult, Runner,
};
pub use session::{InMemorySession, SessionStore};
pub use tool::{Tool, ToolDefinition};
