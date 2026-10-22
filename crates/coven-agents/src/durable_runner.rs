//! Runs invocations against an [`InvocationJournal`] so each invocation id
//! executes at most once and its outcome survives a restart.
//!
//! The order is fixed: reserve the exact request, adopt it with one attempt,
//! run, then settle. A duplicate submission with the same id and request is
//! answered from the journal without running again; the same id with a
//! different request is a conflict. Nothing is retried. Work whose outcome is
//! unknown, whether a stop interrupted a tool, a descendant is unresolved, or
//! an adopted attempt is no longer live, is reported as requiring recovery and
//! is never dispatched again by this runner.

use std::{
    collections::BTreeSet,
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use thiserror::Error;

use crate::{
    agent_loop::invocation_failure_kind, invocation_request_digest, BoxError, ContentDigest,
    ContextResolver, DelegationDigests, InvocationAttempt, InvocationId, InvocationJournal,
    InvocationRecord, InvocationRequest, InvocationSource, InvocationState, PreparedDelegation,
    RunControls, RunError, RunFailure, RunOptions, RunResult, Runner,
};

/// Attempt sequence shared by every runner in this process, so attempt ids
/// stay unique even when several runners share one process instance id.
static ATTEMPT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Attempts currently executing in this process. An adopted record whose
/// attempt is not listed here is not running anywhere this process can see.
static ACTIVE_ATTEMPTS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Keeps an attempt listed as active until the run settles or its future is
/// dropped.
struct ActiveAttempt(String);

impl ActiveAttempt {
    fn register(id: &str) -> Self {
        active_attempts().insert(id.to_owned());
        Self(id.to_owned())
    }
}

impl Drop for ActiveAttempt {
    fn drop(&mut self) {
        active_attempts().remove(&self.0);
    }
}

fn active_attempts() -> std::sync::MutexGuard<'static, BTreeSet<String>> {
    ACTIVE_ATTEMPTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What a journal record means for a runner in this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InvocationDisposition {
    /// Reserved and never adopted. Dispatching it is safe.
    ProvenNotAdopted,
    /// Adopted by an attempt that is executing in this process now.
    Running,
    /// Settled with a known outcome. A duplicate submission replays it.
    Settled,
    /// Its outcome is unknown: a stop interrupted a tool, a descendant is
    /// unresolved, or its adopted attempt is no longer live here. It must be
    /// reconciled before anything runs again.
    RecoveryRequired,
}

/// What one durable submission did.
#[derive(Debug)]
pub enum DurableInvocation {
    /// This call dispatched the invocation and recorded its outcome.
    Dispatched {
        record: Box<InvocationRecord>,
        outcome: Box<Result<RunResult, RunFailure>>,
    },
    /// The invocation was already settled for the same request. Nothing ran.
    Replayed { record: Box<InvocationRecord> },
}

impl DurableInvocation {
    pub fn record(&self) -> &InvocationRecord {
        match self {
            Self::Dispatched { record, .. } | Self::Replayed { record } => record,
        }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DurableInvocationError {
    #[error("process instance id must not be empty")]
    InvalidProcessInstanceId,
    #[error("invocation journal {operation} failed")]
    JournalFailed {
        operation: &'static str,
        #[source]
        source: BoxError,
    },
    #[error("invocation `{invocation}` is already recorded for a different request")]
    Conflict {
        invocation: InvocationId,
        recorded: ContentDigest,
        requested: ContentDigest,
    },
    #[error("invocation `{invocation}` is already running in this process")]
    ActiveInFlight { invocation: InvocationId },
    #[error(
        "invocation `{}` has an unknown outcome and must be reconciled before anything runs",
        record.invocation.id
    )]
    RecoveryRequired { record: Box<InvocationRecord> },
    #[error("adoption of invocation `{invocation}` could not be confirmed, so it was not run")]
    AdoptionUnknown {
        invocation: InvocationId,
        #[source]
        source: BoxError,
    },
    #[error("invocation `{invocation}` ran but its outcome could not be recorded")]
    SettlementFailed {
        invocation: InvocationId,
        /// What the run returned. The journal still shows it running, so a
        /// later read reports it as requiring recovery.
        outcome: Box<Result<RunResult, RunFailure>>,
        #[source]
        source: BoxError,
    },
}

impl DurableInvocationError {
    /// Whether the journal cannot establish a settled child outcome. Hosts
    /// must propagate this uncertainty instead of treating it as a safe retry.
    pub fn leaves_unknown_effect(&self) -> bool {
        matches!(
            self,
            Self::ActiveInFlight { .. }
                | Self::RecoveryRequired { .. }
                | Self::AdoptionUnknown { .. }
                | Self::SettlementFailed { .. }
        )
    }
}

/// Runs invocations through `runner` with their lifecycle in `journal`.
pub struct DurableInvocationRunner<C>
where
    C: Sync,
{
    runner: Arc<Runner<C>>,
    journal: Arc<dyn InvocationJournal>,
    process_instance_id: String,
}

impl<C> DurableInvocationRunner<C>
where
    C: Send + Sync + 'static,
{
    /// `process_instance_id` must change on every process start, so that an
    /// attempt adopted before a restart is never mistaken for a live one.
    pub fn new(
        runner: Arc<Runner<C>>,
        journal: Arc<dyn InvocationJournal>,
        process_instance_id: impl Into<String>,
    ) -> Self {
        Self {
            runner,
            journal,
            process_instance_id: process_instance_id.into(),
        }
    }

    /// What `record` means for this runner.
    pub fn disposition(&self, record: &InvocationRecord) -> InvocationDisposition {
        match &record.state {
            InvocationState::Reserved {} => InvocationDisposition::ProvenNotAdopted,
            InvocationState::Running {} => {
                let live = record.attempt.as_ref().is_some_and(|attempt| {
                    attempt.process_instance_id == self.process_instance_id
                        && active_attempts().contains(&attempt.id)
                });
                if live {
                    InvocationDisposition::Running
                } else {
                    InvocationDisposition::RecoveryRequired
                }
            }
            state if state.is_settled() => InvocationDisposition::Settled,
            _ => InvocationDisposition::RecoveryRequired,
        }
    }

    /// Runs `request` at most once for its invocation id.
    ///
    /// A new id is reserved, adopted by a fresh attempt, run under
    /// `controls`, and settled. An id already settled for the same request
    /// returns [`DurableInvocation::Replayed`] without running. The same id
    /// with a different request, input or options is a
    /// [`DurableInvocationError::Conflict`]. An id whose outcome is unknown
    /// returns [`DurableInvocationError::RecoveryRequired`] and is not run.
    pub async fn run(
        &self,
        request: InvocationRequest,
        input: impl Into<String>,
        context: &C,
        options: RunOptions,
        controls: RunControls,
    ) -> Result<DurableInvocation, DurableInvocationError> {
        let input = input.into();
        let digest = invocation_request_digest(&request, &input, &options);
        self.run_journaled(
            &request,
            digest,
            None,
            self.runner.run_invocation_controlled(
                request.clone(),
                input,
                context,
                options,
                controls,
            ),
        )
        .await
    }

    /// Journals one prepared child under its existing parent identity before
    /// resolving context or dispatching the target. The existing delegation
    /// boundary enforces authority, context, result bounds and child controls.
    /// This does not schedule, retry or reconcile the parent or child.
    pub async fn run_delegation(
        &self,
        delegation: PreparedDelegation,
        resolver: &dyn ContextResolver<C>,
        context: &C,
        controls: RunControls,
    ) -> Result<DurableInvocation, DurableInvocationError> {
        let spec = delegation.spec();
        let timeout = match (controls.timeout(), spec.budget.timeout()) {
            (Some(parent), Some(child)) => Some(parent.min(child)),
            (parent, child) => parent.or(child),
        };
        let controls = match timeout {
            Some(timeout) => controls.with_timeout(timeout),
            None => controls,
        };
        // Start the inherited bound before journal I/O. Starting it only at
        // target dispatch would give the child back time spent adopting it.
        let signal = self.runner.start_signal(controls);
        let target = match spec.target.revision() {
            Some(_) => spec.target.clone(),
            None => self
                .runner
                .registered_agent_ref(spec.target.id())
                .unwrap_or_else(|| spec.target.clone()),
        };
        let request = InvocationRequest::new(
            delegation.invocation().clone(),
            InvocationSource::Agent {
                agent: delegation.source().clone(),
            },
            target,
        );
        let options = RunOptions {
            max_turns: spec.budget.max_turns(),
            max_handoffs: spec.budget.max_handoffs(),
            session_id: None,
        };
        let digests = delegation.digests().clone();
        // Domain separated from direct requests: the same id cannot bypass
        // attenuation by being resubmitted through the direct entry point.
        let digest = ContentDigest::of_bytes(
            &serde_json::to_vec(&(
                "coven.agent-delegated-invocation-request.v1",
                invocation_request_digest(&request, spec.task.as_str(), &options),
                &digests,
            ))
            .expect("delegation metadata always serializes"),
        );
        self.run_journaled(&request, digest, Some(digests), async {
            self.runner
                .run_delegation_controlled(delegation, resolver, context, signal.child_controls())
                .await
                .map(|result| result.run)
                .map_err(|failure| *failure.failure)
        })
        .await
    }

    async fn run_journaled(
        &self,
        request: &InvocationRequest,
        digest: ContentDigest,
        delegation: Option<DelegationDigests>,
        execute: impl Future<Output = Result<RunResult, RunFailure>>,
    ) -> Result<DurableInvocation, DurableInvocationError> {
        if self.process_instance_id.trim().is_empty() {
            return Err(DurableInvocationError::InvalidProcessInstanceId);
        }
        let invocation = request.invocation.id;

        let (running, active) = loop {
            let record = self
                .reserve_or_load(request, &digest, delegation.as_ref())
                .await?;
            if record.request_digest != digest {
                return Err(DurableInvocationError::Conflict {
                    invocation,
                    recorded: record.request_digest,
                    requested: digest,
                });
            }
            match self.disposition(&record) {
                InvocationDisposition::Settled => {
                    return Ok(DurableInvocation::Replayed {
                        record: Box::new(record),
                    });
                }
                InvocationDisposition::Running => {
                    return Err(DurableInvocationError::ActiveInFlight { invocation });
                }
                InvocationDisposition::RecoveryRequired => {
                    return Err(DurableInvocationError::RecoveryRequired {
                        record: Box::new(record),
                    });
                }
                InvocationDisposition::ProvenNotAdopted => {}
            }

            let attempt = InvocationAttempt {
                id: format!(
                    "{}:{}",
                    self.process_instance_id,
                    ATTEMPT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
                ),
                process_instance_id: self.process_instance_id.clone(),
            };
            // Listed before the adoption is written, so a duplicate submitted
            // in this process never sees its own live attempt as abandoned.
            let active = ActiveAttempt::register(&attempt.id);
            let running = InvocationRecord {
                state: InvocationState::Running {},
                attempt: Some(attempt),
                ..record.clone()
            };
            match self
                .journal
                .compare_and_set(invocation, Some(&record), &running)
                .await
            {
                Ok(true) => break (running, active),
                // Another submitter changed the record first; decide again
                // from what it wrote.
                Ok(false) => continue,
                Err(source) => {
                    return Err(DurableInvocationError::AdoptionUnknown { invocation, source });
                }
            }
        };

        let outcome = execute.await;

        let settled = InvocationRecord {
            state: settled_state(&outcome),
            ..running.clone()
        };
        let written = self
            .journal
            .compare_and_set(invocation, Some(&running), &settled)
            .await;
        drop(active);
        match written {
            Ok(true) => Ok(DurableInvocation::Dispatched {
                record: Box::new(settled),
                outcome: Box::new(outcome),
            }),
            Ok(false) => Err(DurableInvocationError::SettlementFailed {
                invocation,
                outcome: Box::new(outcome),
                source: "the record changed while its attempt ran".into(),
            }),
            Err(source) => Err(DurableInvocationError::SettlementFailed {
                invocation,
                outcome: Box::new(outcome),
                source,
            }),
        }
    }

    /// Returns the stored record for this request's id, reserving it first
    /// when there is none.
    async fn reserve_or_load(
        &self,
        request: &InvocationRequest,
        digest: &ContentDigest,
        delegation: Option<&DelegationDigests>,
    ) -> Result<InvocationRecord, DurableInvocationError> {
        let invocation = request.invocation.id;
        loop {
            let stored = self.journal.load(invocation).await.map_err(|source| {
                DurableInvocationError::JournalFailed {
                    operation: "load",
                    source,
                }
            })?;
            if let Some(record) = stored {
                return Ok(record);
            }
            let reserved = InvocationRecord {
                invocation: request.invocation.clone(),
                source: request.source.clone(),
                requested_target: request.requested_target.clone(),
                request_digest: digest.clone(),
                delegation: delegation.cloned(),
                state: InvocationState::Reserved {},
                attempt: None,
            };
            // A failed reservation leaves at most a reserved record, which
            // is proven not adopted, so resubmitting is always safe.
            let written = self
                .journal
                .compare_and_set(invocation, None, &reserved)
                .await
                .map_err(|source| DurableInvocationError::JournalFailed {
                    operation: "reservation",
                    source,
                })?;
            if written {
                return Ok(reserved);
            }
        }
    }
}

/// The durable state for a finished run. Unknown effects outrank every other
/// reading; a failure before the first model turn ran nothing.
fn settled_state(outcome: &Result<RunResult, RunFailure>) -> InvocationState {
    let failure = match outcome {
        Ok(result) => {
            return InvocationState::Completed {
                final_agent: result.final_agent.clone(),
                turns: result.turns,
                control_transfers: result.handoffs,
                output_digest: ContentDigest::of_bytes(result.final_output.as_bytes()),
            };
        }
        Err(failure) => failure,
    };
    match &failure.error {
        RunError::TerminationUnknown { call_id, .. } => InvocationState::TerminationUnknown {
            call_id: call_id.clone(),
        },
        RunError::RecoveryRequired { call_id, .. } => InvocationState::RecoveryRequired {
            call_id: call_id.clone(),
        },
        RunError::Cancelled { .. } => InvocationState::Cancelled {
            turns: failure.turns,
        },
        error if failure.turns == 0 => InvocationState::Rejected {
            kind: invocation_failure_kind(error.failure_kind()),
        },
        error => InvocationState::Failed {
            kind: invocation_failure_kind(error.failure_kind()),
            turns: failure.turns,
        },
    }
}
