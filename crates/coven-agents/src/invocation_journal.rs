//! Durable invocation lifecycle, kept apart from conversation sessions.
//!
//! An [`InvocationRecord`] holds metadata and digests only: the invocation's
//! identity, source and target, a digest of the exact request, its state, and
//! the attempt that adopted it. No prompt, transcript or output is stored, so
//! the journal is not a second session store.
//!
//! The state names follow Psyche's attempt vocabulary (`specs/psyche/TECH.md`,
//! "Node machine"). A record is written `reserved` before anything runs and
//! `running` (adopted by one attempt) before the first model turn. A reserved
//! record was therefore never adopted: it is `proven_not_adopted` and safe to
//! dispatch. Once adopted, only a settled record is trusted; a `running` record
//! whose attempt is no longer live means the outcome is unknown and recovery
//! is required. `adoption_unknown` is never persisted: when the adoption write
//! itself fails, nothing is dispatched, and a later read finds either a
//! reserved record (proven not adopted) or a running one (recovery required).

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    durable_store::{DurableStore, StoreFormat},
    AgentId, AgentRef, BoxError, ContentDigest, DelegationDigests, InvocationContext,
    InvocationFailureKind, InvocationId, InvocationRequest, InvocationSource, RunOptions,
};

const REQUEST_CONTRACT: &str = "coven.agent-invocation-request.v1";

const FORMAT: StoreFormat = StoreFormat {
    label: "invocation journal",
    schema_version: 1,
    record_field: "record",
};

/// One attempt to execute an invocation, bound to one process lifetime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationAttempt {
    pub id: String,
    /// Must identify one process lifetime, not merely a host or daemon name.
    pub process_instance_id: String,
}

/// Where an invocation stands in its durable lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InvocationState {
    /// The request and its digest are durable and nothing has run. Written
    /// before adoption, so a reserved record is proven not adopted.
    Reserved {},
    /// The record's attempt adopted the invocation before its first model
    /// turn. Effects may have happened from here on.
    Running {},
    /// The invocation completed. Only a digest of the output is kept.
    Completed {
        final_agent: AgentId,
        turns: usize,
        control_transfers: usize,
        output_digest: ContentDigest,
    },
    /// The invocation failed before its first model turn, so no model was
    /// called and no tool ran.
    Rejected { kind: InvocationFailureKind },
    /// The invocation failed with a known kind after it started executing.
    /// Completed tool effects may exist; the failure itself is not ambiguous.
    Failed {
        kind: InvocationFailureKind,
        turns: usize,
    },
    /// A stop was acknowledged with no tool in flight.
    Cancelled { turns: usize },
    /// A stop arrived while a tool ran and the tool did not prove it produced
    /// nothing. Its effect may or may not have happened.
    TerminationUnknown { call_id: String },
    /// A tool reported a descendant invocation whose outcome is unknown.
    RecoveryRequired { call_id: String },
}

impl InvocationState {
    /// Whether the outcome is final and known: completed, rejected, failed or
    /// cancelled.
    pub fn is_settled(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. }
                | Self::Rejected { .. }
                | Self::Failed { .. }
                | Self::Cancelled { .. }
        )
    }

    /// Whether the outcome is final but leaves an effect whose result is
    /// unknown, so the work must be reconciled before anything is retried.
    pub fn leaves_unknown_effect(&self) -> bool {
        matches!(
            self,
            Self::TerminationUnknown { .. } | Self::RecoveryRequired { .. }
        )
    }
}

/// The durable lifecycle record of one invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationRecord {
    pub invocation: InvocationContext,
    pub source: InvocationSource,
    pub requested_target: AgentRef,
    /// Digest of the exact request; see [`invocation_request_digest`].
    pub request_digest: ContentDigest,
    /// Bounded delegation evidence; never contains task or context payloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegation: Option<DelegationDigests>,
    pub state: InvocationState,
    /// The attempt that adopted the invocation. Absent while reserved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<InvocationAttempt>,
}

/// Digest of everything that defines one invocation request: its identity,
/// source and target, the input, and the run options. Resubmitting the same
/// invocation id with any of these changed is a conflict, never a replay.
/// Cancellation and deadline controls are runtime bounds, not part of the
/// request, and are excluded.
pub fn invocation_request_digest(
    request: &InvocationRequest,
    input: &str,
    options: &RunOptions,
) -> ContentDigest {
    #[derive(Serialize)]
    struct DigestedRequest<'a> {
        contract: &'static str,
        invocation: &'a InvocationContext,
        source: &'a InvocationSource,
        requested_target: &'a AgentRef,
        input_digest: ContentDigest,
        max_turns: usize,
        max_handoffs: usize,
        session_id: Option<&'a str>,
    }
    let digested = DigestedRequest {
        contract: REQUEST_CONTRACT,
        invocation: &request.invocation,
        source: &request.source,
        requested_target: &request.requested_target,
        input_digest: ContentDigest::of_bytes(input.as_bytes()),
        max_turns: options.max_turns,
        max_handoffs: options.max_handoffs,
        session_id: options.session_id.as_deref(),
    };
    let bytes = serde_json::to_vec(&digested).expect("request metadata always serializes");
    ContentDigest::of_bytes(&bytes)
}

/// Durable storage for invocation records.
#[async_trait]
pub trait InvocationJournal: Send + Sync {
    async fn load(&self, invocation: InvocationId) -> Result<Option<InvocationRecord>, BoxError>;

    /// Lists records so a restarted host can find work that needs attention.
    async fn list(&self) -> Result<Vec<InvocationRecord>, BoxError>;

    /// Atomically replaces `expected` with `next`.
    ///
    /// Implementations must compare and write as one operation. Returning
    /// `false` means another writer changed the record first. An error means
    /// the write may or may not have taken effect.
    async fn compare_and_set(
        &self,
        invocation: InvocationId,
        expected: Option<&InvocationRecord>,
        next: &InvocationRecord,
    ) -> Result<bool, BoxError>;
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryInvocationJournal {
    records: Arc<Mutex<BTreeMap<InvocationId, InvocationRecord>>>,
}

impl InMemoryInvocationJournal {
    fn records(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<InvocationId, InvocationRecord>>, BoxError> {
        self.records.lock().map_err(|_| {
            Box::new(io::Error::other(
                "in-memory invocation journal lock poisoned",
            )) as BoxError
        })
    }
}

#[async_trait]
impl InvocationJournal for InMemoryInvocationJournal {
    async fn load(&self, invocation: InvocationId) -> Result<Option<InvocationRecord>, BoxError> {
        Ok(self.records()?.get(&invocation).cloned())
    }

    async fn list(&self) -> Result<Vec<InvocationRecord>, BoxError> {
        Ok(self.records()?.values().cloned().collect())
    }

    async fn compare_and_set(
        &self,
        invocation: InvocationId,
        expected: Option<&InvocationRecord>,
        next: &InvocationRecord,
    ) -> Result<bool, BoxError> {
        check_key(invocation, next)?;
        let mut records = self.records()?;
        if records.get(&invocation) != expected {
            return Ok(false);
        }
        records.insert(invocation, next.clone());
        Ok(true)
    }
}

/// A file-backed journal with the same crash-safe checkpoints as
/// [`crate::FileLoopJournal`]: each write is staged, synced and renamed into
/// place under an exclusive lock, so a crash leaves the previous record or
/// the new one, never a torn file.
#[derive(Debug, Clone)]
pub struct FileInvocationJournal {
    store: DurableStore,
}

impl FileInvocationJournal {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, io::Error> {
        Ok(Self {
            store: DurableStore::new(root, FORMAT)?,
        })
    }

    pub fn root(&self) -> &Path {
        self.store.root()
    }
}

#[async_trait]
impl InvocationJournal for FileInvocationJournal {
    async fn load(&self, invocation: InvocationId) -> Result<Option<InvocationRecord>, BoxError> {
        self.store.load(&invocation.to_string())
    }

    async fn list(&self) -> Result<Vec<InvocationRecord>, BoxError> {
        let mut records: Vec<InvocationRecord> = self.store.list()?;
        records.sort_by_key(|record| record.invocation.id);
        Ok(records)
    }

    async fn compare_and_set(
        &self,
        invocation: InvocationId,
        expected: Option<&InvocationRecord>,
        next: &InvocationRecord,
    ) -> Result<bool, BoxError> {
        check_key(invocation, next)?;
        self.store
            .compare_and_set(&invocation.to_string(), expected, next)
    }
}

fn check_key(invocation: InvocationId, record: &InvocationRecord) -> Result<(), BoxError> {
    if record.invocation.id != invocation {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            "record invocation id does not match journal key",
        )));
    }
    Ok(())
}
