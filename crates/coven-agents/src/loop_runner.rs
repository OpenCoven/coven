use std::{
    collections::BTreeMap,
    io,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{AgentId, BoxError, RunFailure, RunOptions, RunResult, Runner};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopCheckpointStatus {
    Pending,
    Running,
    Blocked,
    Completed,
    Failed,
    Exhausted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopAttempt {
    pub id: String,
    /// Must identify one process lifetime, not merely a host or daemon name.
    pub process_instance_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopCheckpoint {
    pub loop_id: String,
    pub starting_agent: AgentId,
    pub completed_iterations: usize,
    pub status: LoopCheckpointStatus,
    pub next_input: Option<String>,
    #[serde(default)]
    pub active_attempt: Option<LoopAttempt>,
    #[serde(default)]
    pub blocked_reason: Option<String>,
}

#[async_trait]
pub trait LoopJournal: Send + Sync {
    async fn load(&self, loop_id: &str) -> Result<Option<LoopCheckpoint>, BoxError>;

    /// Lists checkpoints so a restarted host can discover work to recover.
    async fn list(&self) -> Result<Vec<LoopCheckpoint>, BoxError>;

    /// Atomically replaces `expected` with `next`.
    ///
    /// Implementations must compare and write as one operation. Returning
    /// `false` means another writer changed the checkpoint first.
    async fn compare_and_set(
        &self,
        loop_id: &str,
        expected: Option<&LoopCheckpoint>,
        next: &LoopCheckpoint,
    ) -> Result<bool, BoxError>;
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryLoopJournal {
    checkpoints: Arc<Mutex<BTreeMap<String, LoopCheckpoint>>>,
}

impl InMemoryLoopJournal {
    pub async fn checkpoint(&self, loop_id: &str) -> Result<Option<LoopCheckpoint>, BoxError> {
        self.load(loop_id).await
    }
}

#[async_trait]
impl LoopJournal for InMemoryLoopJournal {
    async fn load(&self, loop_id: &str) -> Result<Option<LoopCheckpoint>, BoxError> {
        let checkpoints = self.checkpoints.lock().map_err(|_| {
            Box::new(io::Error::other("in-memory loop journal lock poisoned")) as BoxError
        })?;
        Ok(checkpoints.get(loop_id).cloned())
    }

    async fn list(&self) -> Result<Vec<LoopCheckpoint>, BoxError> {
        let checkpoints = self.checkpoints.lock().map_err(|_| {
            Box::new(io::Error::other("in-memory loop journal lock poisoned")) as BoxError
        })?;
        Ok(checkpoints.values().cloned().collect())
    }

    async fn compare_and_set(
        &self,
        loop_id: &str,
        expected: Option<&LoopCheckpoint>,
        next: &LoopCheckpoint,
    ) -> Result<bool, BoxError> {
        if next.loop_id != loop_id {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "checkpoint loop id does not match journal key",
            )));
        }
        let mut checkpoints = self.checkpoints.lock().map_err(|_| {
            Box::new(io::Error::other("in-memory loop journal lock poisoned")) as BoxError
        })?;
        if checkpoints.get(loop_id) != expected {
            return Ok(false);
        }
        checkpoints.insert(loop_id.to_owned(), next.clone());
        Ok(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopControl {
    Complete,
    Continue { input: String },
}

#[async_trait]
pub trait LoopEvaluator<C>: Send + Sync
where
    C: Sync,
{
    async fn evaluate(
        &self,
        iteration: usize,
        result: &RunResult,
        context: &C,
    ) -> Result<LoopControl, BoxError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopReconciliation {
    /// Keep the durable checkpoint unchanged.
    Unchanged,
    /// External state already satisfies the loop's goal.
    Complete,
    /// External state proves the current iteration is safe to run again.
    Resume { input: String },
    /// External state proves an in-flight iteration completed and supplies the
    /// next iteration's input.
    Continue { input: String },
    /// Recovery needs explicit operator input.
    Blocked { reason: String },
}

#[async_trait]
pub trait LoopReconciler<C>: Send + Sync
where
    C: Sync,
{
    async fn reconcile(
        &self,
        checkpoint: &LoopCheckpoint,
        context: &C,
    ) -> Result<LoopReconciliation, BoxError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopOptions {
    pub max_iterations: usize,
    pub run: RunOptions,
}

impl Default for LoopOptions {
    fn default() -> Self {
        Self {
            max_iterations: 8,
            run: RunOptions::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoopRunResult {
    /// `None` means reconciliation proved the external goal was already met,
    /// so no agent iteration ran in this process.
    pub result: Option<RunResult>,
    pub iterations: usize,
}

#[derive(Debug, Error)]
pub enum LoopError {
    #[error("loop journal {operation} failed")]
    JournalFailed {
        operation: &'static str,
        #[source]
        source: BoxError,
    },
    #[error("loop `{loop_id}` checkpoint belongs to starting agent `{actual}`, not `{expected}`")]
    StartingAgentMismatch {
        loop_id: String,
        expected: AgentId,
        actual: AgentId,
    },
    #[error(
        "loop `{loop_id}` has an in-flight iteration; refusing automatic replay because external side effects may have occurred"
    )]
    AmbiguousInFlight { loop_id: String },
    #[error("loop `{loop_id}` has a live in-flight iteration owned by this process")]
    ActiveInFlight { loop_id: String },
    #[error("loop `{loop_id}` checkpoint changed concurrently during {operation}")]
    ConcurrentUpdate {
        loop_id: String,
        operation: &'static str,
    },
    #[error("loop `{loop_id}` is already {status:?}")]
    TerminalCheckpoint {
        loop_id: String,
        status: LoopCheckpointStatus,
    },
    #[error("loop `{loop_id}` pending checkpoint has no next input")]
    MissingNextInput { loop_id: String },
    #[error("loop `{loop_id}` exceeded the maximum of {limit} iterations")]
    MaxIterationsExceeded { loop_id: String, limit: usize },
    #[error("loop `{loop_id}` agent run failed at iteration {iteration}")]
    RunFailed {
        loop_id: String,
        iteration: usize,
        #[source]
        source: RunFailure,
    },
    #[error("loop `{loop_id}` exit evaluation failed at iteration {iteration}")]
    EvaluationFailed {
        loop_id: String,
        iteration: usize,
        #[source]
        source: BoxError,
    },
    #[error("loop `{loop_id}` reconciliation failed")]
    ReconciliationFailed {
        loop_id: String,
        #[source]
        source: BoxError,
    },
    #[error("loop `{loop_id}` reconciliation is blocked: {reason}")]
    ReconciliationBlocked { loop_id: String, reason: String },
    #[error("loop `{loop_id}` reconciliation cannot continue a {status:?} checkpoint")]
    InvalidReconciliation {
        loop_id: String,
        status: LoopCheckpointStatus,
    },
    #[error("loop process instance id must not be empty")]
    InvalidProcessInstanceId,
}

pub struct GoalLoopRunner<C>
where
    C: Sync,
{
    runner: Runner<C>,
    evaluator: Arc<dyn LoopEvaluator<C>>,
    journal: Arc<dyn LoopJournal>,
    reconciler: Option<Arc<dyn LoopReconciler<C>>>,
    process_instance_id: String,
    attempt_sequence: AtomicUsize,
}

impl<C> GoalLoopRunner<C>
where
    C: Send + Sync + 'static,
{
    pub fn new(
        runner: Runner<C>,
        evaluator: Arc<dyn LoopEvaluator<C>>,
        journal: Arc<dyn LoopJournal>,
        process_instance_id: impl Into<String>,
    ) -> Self {
        Self {
            runner,
            evaluator,
            journal,
            reconciler: None,
            process_instance_id: process_instance_id.into(),
            attempt_sequence: AtomicUsize::new(0),
        }
    }

    pub fn with_reconciler(mut self, reconciler: Arc<dyn LoopReconciler<C>>) -> Self {
        self.reconciler = Some(reconciler);
        self
    }

    pub async fn run(
        &self,
        loop_id: impl Into<String>,
        starting_agent: impl Into<AgentId>,
        initial_input: impl Into<String>,
        context: &C,
        options: LoopOptions,
    ) -> Result<LoopRunResult, LoopError> {
        let loop_id = loop_id.into();
        let starting_agent = starting_agent.into();
        let initial_input = initial_input.into();
        if self.process_instance_id.trim().is_empty() {
            return Err(LoopError::InvalidProcessInstanceId);
        }
        let mut resumed = false;
        let mut checkpoint = loop {
            match self.load(&loop_id).await? {
                Some(checkpoint) => {
                    resumed = true;
                    break checkpoint;
                }
                None => {
                    let checkpoint = LoopCheckpoint {
                        loop_id: loop_id.clone(),
                        starting_agent: starting_agent.clone(),
                        completed_iterations: 0,
                        status: LoopCheckpointStatus::Pending,
                        next_input: Some(initial_input.clone()),
                        active_attempt: None,
                        blocked_reason: None,
                    };
                    if self
                        .compare_and_set(None, &checkpoint, "initialization")
                        .await?
                    {
                        break checkpoint;
                    }
                }
            }
        };

        if checkpoint.starting_agent != starting_agent {
            return Err(LoopError::StartingAgentMismatch {
                loop_id,
                expected: starting_agent,
                actual: checkpoint.starting_agent,
            });
        }

        if checkpoint.status == LoopCheckpointStatus::Running
            && checkpoint
                .active_attempt
                .as_ref()
                .is_some_and(|attempt| attempt.process_instance_id == self.process_instance_id)
        {
            return Err(LoopError::ActiveInFlight { loop_id });
        }

        if resumed
            && matches!(
                checkpoint.status,
                LoopCheckpointStatus::Pending | LoopCheckpointStatus::Running
            )
        {
            if let Some(reconciler) = &self.reconciler {
                let reconciliation = match reconciler.reconcile(&checkpoint, context).await {
                    Ok(reconciliation) => reconciliation,
                    Err(source) => {
                        let mut blocked = checkpoint.clone();
                        blocked.status = LoopCheckpointStatus::Blocked;
                        blocked.active_attempt = None;
                        blocked.blocked_reason = Some("offline reconciliation failed".to_owned());
                        self.compare_and_set(Some(&checkpoint), &blocked, "reconciliation failure")
                            .await?;
                        return Err(LoopError::ReconciliationFailed { loop_id, source });
                    }
                };
                match reconciliation {
                    LoopReconciliation::Unchanged => {}
                    LoopReconciliation::Complete => {
                        let mut completed = checkpoint.clone();
                        if checkpoint.status == LoopCheckpointStatus::Running {
                            completed.completed_iterations += 1;
                        }
                        completed.status = LoopCheckpointStatus::Completed;
                        completed.next_input = None;
                        completed.active_attempt = None;
                        completed.blocked_reason = None;
                        self.compare_and_set(
                            Some(&checkpoint),
                            &completed,
                            "offline reconciliation",
                        )
                        .await?;
                        return Ok(LoopRunResult {
                            result: None,
                            iterations: completed.completed_iterations,
                        });
                    }
                    LoopReconciliation::Resume { input } => {
                        let mut pending = checkpoint.clone();
                        pending.status = LoopCheckpointStatus::Pending;
                        pending.next_input = Some(input);
                        pending.active_attempt = None;
                        pending.blocked_reason = None;
                        self.compare_and_set(Some(&checkpoint), &pending, "offline reconciliation")
                            .await?;
                        checkpoint = pending;
                    }
                    LoopReconciliation::Continue { input } => {
                        if checkpoint.status != LoopCheckpointStatus::Running {
                            return Err(LoopError::InvalidReconciliation {
                                loop_id,
                                status: checkpoint.status,
                            });
                        }
                        let mut pending = checkpoint.clone();
                        pending.completed_iterations += 1;
                        pending.status = LoopCheckpointStatus::Pending;
                        pending.next_input = Some(input);
                        pending.active_attempt = None;
                        pending.blocked_reason = None;
                        self.compare_and_set(Some(&checkpoint), &pending, "offline reconciliation")
                            .await?;
                        checkpoint = pending;
                    }
                    LoopReconciliation::Blocked { reason } => {
                        let mut blocked = checkpoint.clone();
                        blocked.status = LoopCheckpointStatus::Blocked;
                        blocked.active_attempt = None;
                        blocked.blocked_reason = Some(reason.clone());
                        self.compare_and_set(Some(&checkpoint), &blocked, "blocked reconciliation")
                            .await?;
                        return Err(LoopError::ReconciliationBlocked { loop_id, reason });
                    }
                }
            }
        }

        match checkpoint.status {
            LoopCheckpointStatus::Pending => {}
            LoopCheckpointStatus::Running => {
                return Err(LoopError::AmbiguousInFlight { loop_id });
            }
            LoopCheckpointStatus::Blocked => {
                return Err(LoopError::ReconciliationBlocked {
                    loop_id,
                    reason: checkpoint
                        .blocked_reason
                        .unwrap_or_else(|| "operator recovery is required".to_owned()),
                });
            }
            LoopCheckpointStatus::Completed
            | LoopCheckpointStatus::Failed
            | LoopCheckpointStatus::Exhausted => {
                return Err(LoopError::TerminalCheckpoint {
                    loop_id,
                    status: checkpoint.status,
                });
            }
        }

        loop {
            if checkpoint.completed_iterations >= options.max_iterations {
                let mut exhausted = checkpoint.clone();
                exhausted.status = LoopCheckpointStatus::Exhausted;
                exhausted.next_input = None;
                exhausted.active_attempt = None;
                self.compare_and_set(Some(&checkpoint), &exhausted, "exhaustion")
                    .await?;
                return Err(LoopError::MaxIterationsExceeded {
                    loop_id,
                    limit: options.max_iterations,
                });
            }

            let input =
                checkpoint
                    .next_input
                    .clone()
                    .ok_or_else(|| LoopError::MissingNextInput {
                        loop_id: loop_id.clone(),
                    })?;
            let iteration = checkpoint.completed_iterations + 1;
            let mut running = checkpoint.clone();
            running.status = LoopCheckpointStatus::Running;
            running.next_input = None;
            running.blocked_reason = None;
            running.active_attempt = Some(LoopAttempt {
                id: format!(
                    "{}:{iteration}:{}",
                    self.process_instance_id,
                    self.attempt_sequence.fetch_add(1, Ordering::Relaxed)
                ),
                process_instance_id: self.process_instance_id.clone(),
            });
            self.compare_and_set(Some(&checkpoint), &running, "iteration claim")
                .await?;
            checkpoint = running;

            let result = match self
                .runner
                .run(starting_agent.clone(), input, context, options.run.clone())
                .await
            {
                Ok(result) => result,
                Err(source) => {
                    let mut failed = checkpoint.clone();
                    failed.status = LoopCheckpointStatus::Failed;
                    failed.active_attempt = None;
                    self.compare_and_set(Some(&checkpoint), &failed, "run failure")
                        .await?;
                    return Err(LoopError::RunFailed {
                        loop_id,
                        iteration,
                        source,
                    });
                }
            };

            let control = match self.evaluator.evaluate(iteration, &result, context).await {
                Ok(control) => control,
                Err(source) => {
                    let mut failed = checkpoint.clone();
                    failed.status = LoopCheckpointStatus::Failed;
                    failed.active_attempt = None;
                    self.compare_and_set(Some(&checkpoint), &failed, "evaluation failure")
                        .await?;
                    return Err(LoopError::EvaluationFailed {
                        loop_id,
                        iteration,
                        source,
                    });
                }
            };

            match control {
                LoopControl::Complete => {
                    let mut completed = checkpoint.clone();
                    completed.completed_iterations = iteration;
                    completed.status = LoopCheckpointStatus::Completed;
                    completed.active_attempt = None;
                    self.compare_and_set(Some(&checkpoint), &completed, "completion")
                        .await?;
                    return Ok(LoopRunResult {
                        result: Some(result),
                        iterations: iteration,
                    });
                }
                LoopControl::Continue { input } => {
                    let mut next = checkpoint.clone();
                    next.completed_iterations = iteration;
                    next.next_input = Some(input);
                    if iteration >= options.max_iterations {
                        next.status = LoopCheckpointStatus::Exhausted;
                        next.next_input = None;
                        next.active_attempt = None;
                        self.compare_and_set(Some(&checkpoint), &next, "exhaustion")
                            .await?;
                        return Err(LoopError::MaxIterationsExceeded {
                            loop_id,
                            limit: options.max_iterations,
                        });
                    }
                    next.status = LoopCheckpointStatus::Pending;
                    next.active_attempt = None;
                    self.compare_and_set(Some(&checkpoint), &next, "continuation")
                        .await?;
                    checkpoint = next;
                }
            }
        }
    }

    async fn load(&self, loop_id: &str) -> Result<Option<LoopCheckpoint>, LoopError> {
        self.journal
            .load(loop_id)
            .await
            .map_err(|source| LoopError::JournalFailed {
                operation: "load",
                source,
            })
    }

    async fn compare_and_set(
        &self,
        expected: Option<&LoopCheckpoint>,
        next: &LoopCheckpoint,
        operation: &'static str,
    ) -> Result<bool, LoopError> {
        let changed = self
            .journal
            .compare_and_set(&next.loop_id, expected, next)
            .await
            .map_err(|source| LoopError::JournalFailed { operation, source })?;
        if !changed {
            return Err(LoopError::ConcurrentUpdate {
                loop_id: next.loop_id.clone(),
                operation,
            });
        }
        Ok(true)
    }
}
