//! Durable invocation lifecycle: reservation before adoption, adoption before
//! the first model turn, and settlement after. Crashes are simulated at each
//! transition, either by a journal write that fails before or after it takes
//! effect, or by dropping a run's future while a tool is in flight. Every
//! case must recover deterministically or keep its uncertainty explicit, and
//! nothing may run twice.

use std::{
    collections::VecDeque,
    future::Future,
    io,
    num::NonZeroUsize,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll, Waker},
};

use async_trait::async_trait;
use coven_agents::{
    invocation_request_digest, Agent, AgentId, AgentRef, BoxError, CancellationToken,
    ContentDigest, DurableInvocation, DurableInvocationError, DurableInvocationRunner,
    FileInvocationJournal, InMemoryInvocationJournal, InvocationCapacity, InvocationContext,
    InvocationDisposition, InvocationFailureKind, InvocationId, InvocationJournal,
    InvocationRecord, InvocationRequest, InvocationSource, InvocationState, Model, ModelAction,
    ModelRequest, ModelResponse, RunControls, RunError, RunOptions, RunSignal, Runner, StopReason,
    Tool, ToolCall, ToolDefinition,
};
use serde_json::{json, Value};

struct ScriptedModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    calls: AtomicUsize,
}

impl ScriptedModel {
    fn new(responses: impl IntoIterator<Item = ModelResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Model<()> for ScriptedModel {
    async fn generate(
        &self,
        _request: ModelRequest,
        _context: &(),
    ) -> Result<ModelResponse, BoxError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| Box::new(io::Error::other("no scripted response")) as BoxError)
    }
}

/// Completes once its flag is set. Polled manually, so it needs no waker.
struct Gate(Arc<AtomicBool>);

impl Future for Gate {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<()> {
        if self.0.load(Ordering::SeqCst) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

enum Behavior {
    /// Waits for the gate before succeeding.
    Block(Arc<AtomicBool>),
    /// Cancels the run, then fails without proving it did nothing.
    CancelThenFail(CancellationToken),
    /// Fails with a delegated child whose termination is unknown.
    ReportUnknownChild,
    ReportDurableUnknownChild,
}

struct WorkTool {
    behavior: Behavior,
    executions: AtomicUsize,
}

impl WorkTool {
    fn new(behavior: Behavior) -> Arc<Self> {
        Arc::new(Self {
            behavior,
            executions: AtomicUsize::new(0),
        })
    }

    fn executions(&self) -> usize {
        self.executions.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Tool<()> for WorkTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("work", "does work", json!({ "type": "object" }))
    }

    async fn execute(&self, _arguments: Value, _context: &()) -> Result<Value, BoxError> {
        unreachable!("the runner calls execute_with_signal")
    }

    async fn execute_with_signal(
        &self,
        _arguments: Value,
        _context: &(),
        _signal: &RunSignal,
    ) -> Result<Value, BoxError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        match &self.behavior {
            Behavior::Block(gate) => {
                Gate(gate.clone()).await;
                Ok(json!({ "worked": true }))
            }
            Behavior::CancelThenFail(cancellation) => {
                cancellation.cancel();
                Err(Box::new(io::Error::other("connection reset")))
            }
            Behavior::ReportDurableUnknownChild => {
                Err(Box::new(DurableInvocationError::AdoptionUnknown {
                    invocation: InvocationId::new(),
                    source: Box::new(io::Error::other("lost adoption receipt")),
                }))
            }
            Behavior::ReportUnknownChild => Err(Box::new(RunError::TerminationUnknown {
                agent: AgentId::from("child"),
                call_id: "child-call".to_owned(),
                reason: StopReason::Cancelled,
                source: Box::new(io::Error::other("child connection reset")),
            })),
        }
    }
}

/// Fails one numbered write, either before it takes effect or after.
struct FaultyJournal {
    inner: InMemoryInvocationJournal,
    writes: AtomicUsize,
    fail_write: usize,
    after_write: bool,
}

impl FaultyJournal {
    fn failing(fail_write: usize, after_write: bool) -> Arc<Self> {
        Arc::new(Self {
            inner: InMemoryInvocationJournal::default(),
            writes: AtomicUsize::new(0),
            fail_write,
            after_write,
        })
    }
}

#[async_trait]
impl InvocationJournal for FaultyJournal {
    async fn load(&self, invocation: InvocationId) -> Result<Option<InvocationRecord>, BoxError> {
        self.inner.load(invocation).await
    }

    async fn list(&self) -> Result<Vec<InvocationRecord>, BoxError> {
        self.inner.list().await
    }

    async fn compare_and_set(
        &self,
        invocation: InvocationId,
        expected: Option<&InvocationRecord>,
        next: &InvocationRecord,
    ) -> Result<bool, BoxError> {
        let write = self.writes.fetch_add(1, Ordering::SeqCst) + 1;
        if write != self.fail_write {
            return self.inner.compare_and_set(invocation, expected, next).await;
        }
        if self.after_write {
            self.inner
                .compare_and_set(invocation, expected, next)
                .await?;
        }
        Err(Box::new(io::Error::other("power lost during write")))
    }
}

const RESERVATION: usize = 1;
const ADOPTION: usize = 2;
const SETTLEMENT: usize = 3;

fn worker() -> AgentRef {
    AgentRef::with_revision("worker", "rev-1").unwrap()
}

fn runner(model: Arc<ScriptedModel>, tool: Option<Arc<WorkTool>>) -> Arc<Runner<()>> {
    let mut agent =
        Agent::new("worker", "Worker", "work", model).with_revision("rev-1".parse().unwrap());
    if let Some(tool) = tool {
        agent = agent.with_tool(tool);
    }
    Arc::new(Runner::new([agent]).unwrap())
}

fn durable(
    runner: &Arc<Runner<()>>,
    journal: Arc<dyn InvocationJournal>,
    process: &str,
) -> DurableInvocationRunner<()> {
    DurableInvocationRunner::new(
        runner.clone(),
        journal,
        process,
        Arc::new(InvocationCapacity::new(NonZeroUsize::new(16).unwrap())),
    )
}

fn request() -> InvocationRequest {
    InvocationRequest::new(
        InvocationContext::root(InvocationId::new()),
        InvocationSource::Caller,
        worker(),
    )
}

fn work_then_finish() -> [ModelResponse; 2] {
    [
        ModelResponse::actions(vec![ModelAction::ToolCall(ToolCall::new(
            "c1",
            "work",
            json!({}),
        ))]),
        ModelResponse::final_output("done"),
    ]
}

async fn submit(
    runner: &DurableInvocationRunner<()>,
    request: &InvocationRequest,
    input: &str,
) -> Result<DurableInvocation, DurableInvocationError> {
    runner
        .run(
            request.clone(),
            input,
            &(),
            RunOptions::default(),
            RunControls::default(),
        )
        .await
}

fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

fn dispatched_state(outcome: Result<DurableInvocation, DurableInvocationError>) -> InvocationState {
    match outcome.unwrap() {
        DurableInvocation::Dispatched { record, .. } => record.state,
        DurableInvocation::Replayed { record } => {
            panic!("expected a dispatch, replayed {record:?}")
        }
    }
}

fn assert_recovery_required(outcome: Result<DurableInvocation, DurableInvocationError>) {
    assert!(
        matches!(
            outcome,
            Err(DurableInvocationError::RecoveryRequired { .. })
        ),
        "expected recovery required, got {outcome:?}"
    );
}

#[tokio::test]
async fn completes_once_and_answers_duplicates_from_the_journal() {
    let model = ScriptedModel::new([ModelResponse::final_output("done")]);
    let runner = runner(model.clone(), None);
    let journal = Arc::new(InMemoryInvocationJournal::default());
    let first = durable(&runner, journal.clone(), "process-1");
    let request = request();

    let record = match submit(&first, &request, "summarize").await.unwrap() {
        DurableInvocation::Dispatched { record, outcome } => {
            assert_eq!(outcome.unwrap().final_output, "done");
            record
        }
        other => panic!("expected a dispatch, got {other:?}"),
    };
    assert_eq!(
        record.state,
        InvocationState::Completed {
            final_agent: AgentId::from("worker"),
            turns: 1,
            control_transfers: 0,
            output_digest: ContentDigest::of_bytes(b"done"),
        }
    );
    assert_eq!(
        record.request_digest,
        invocation_request_digest(&request, "summarize", &RunOptions::default())
    );
    assert_eq!(
        record.attempt.as_ref().unwrap().process_instance_id,
        "process-1"
    );

    let after_restart = durable(&runner, journal.clone(), "process-2");
    for runner in [&first, &after_restart] {
        match submit(runner, &request, "summarize").await.unwrap() {
            DurableInvocation::Replayed { record: replayed } => assert_eq!(replayed, record),
            other => panic!("expected a replay, got {other:?}"),
        }
    }
    assert_eq!(model.calls(), 1);
    assert_eq!(journal.list().await.unwrap(), vec![*record]);
}

#[tokio::test]
async fn reusing_an_invocation_id_for_a_different_request_is_a_conflict() {
    let model = ScriptedModel::new([ModelResponse::final_output("done")]);
    let runner = runner(model.clone(), None);
    let durable = durable(
        &runner,
        Arc::new(InMemoryInvocationJournal::default()),
        "process-1",
    );
    let request = request();
    submit(&durable, &request, "summarize").await.unwrap();

    let changed_input = submit(&durable, &request, "summarize differently").await;
    assert!(matches!(
        changed_input,
        Err(DurableInvocationError::Conflict { invocation, .. }) if invocation == request.invocation.id
    ));
    let changed_options = durable
        .run(
            request.clone(),
            "summarize",
            &(),
            RunOptions {
                max_turns: 3,
                ..RunOptions::default()
            },
            RunControls::default(),
        )
        .await;
    assert!(matches!(
        changed_options,
        Err(DurableInvocationError::Conflict { .. })
    ));
    assert_eq!(model.calls(), 1);
}

#[tokio::test]
async fn a_failed_reservation_runs_nothing_and_can_be_resubmitted() {
    let model = ScriptedModel::new([ModelResponse::final_output("done")]);
    let runner = runner(model.clone(), None);
    let journal = FaultyJournal::failing(RESERVATION, true);
    let request = request();

    let outcome = submit(
        &durable(&runner, journal.clone(), "process-1"),
        &request,
        "go",
    )
    .await;
    assert!(matches!(
        outcome,
        Err(DurableInvocationError::JournalFailed {
            operation: "reservation",
            ..
        })
    ));
    assert_eq!(model.calls(), 0);

    let after_restart = durable(&runner, journal.clone(), "process-2");
    let reserved = journal.load(request.invocation.id).await.unwrap().unwrap();
    assert_eq!(reserved.state, InvocationState::Reserved {});
    assert_eq!(
        after_restart.disposition(&reserved),
        InvocationDisposition::ProvenNotAdopted
    );
    assert!(matches!(
        dispatched_state(submit(&after_restart, &request, "go").await),
        InvocationState::Completed { .. }
    ));
    assert_eq!(model.calls(), 1);
}

#[tokio::test]
async fn an_adoption_that_never_landed_is_proven_not_adopted_and_runs_once() {
    let model = ScriptedModel::new([ModelResponse::final_output("done")]);
    let runner = runner(model.clone(), None);
    let journal = FaultyJournal::failing(ADOPTION, false);
    let request = request();

    let outcome = submit(
        &durable(&runner, journal.clone(), "process-1"),
        &request,
        "go",
    )
    .await;
    assert!(matches!(
        outcome,
        Err(DurableInvocationError::AdoptionUnknown { .. })
    ));
    assert_eq!(model.calls(), 0);

    let after_restart = durable(&runner, journal.clone(), "process-2");
    let stored = journal.load(request.invocation.id).await.unwrap().unwrap();
    assert_eq!(
        after_restart.disposition(&stored),
        InvocationDisposition::ProvenNotAdopted
    );
    assert!(matches!(
        dispatched_state(submit(&after_restart, &request, "go").await),
        InvocationState::Completed { .. }
    ));
    assert_eq!(model.calls(), 1);
}

#[tokio::test]
async fn an_adoption_whose_write_outcome_is_unknown_is_never_dispatched() {
    let model = ScriptedModel::new([ModelResponse::final_output("unused")]);
    let runner = runner(model.clone(), None);
    let journal = FaultyJournal::failing(ADOPTION, true);
    let request = request();

    let outcome = submit(
        &durable(&runner, journal.clone(), "process-1"),
        &request,
        "go",
    )
    .await;
    assert!(matches!(
        outcome,
        Err(DurableInvocationError::AdoptionUnknown { .. })
    ));

    let stored = journal.load(request.invocation.id).await.unwrap().unwrap();
    assert_eq!(stored.state, InvocationState::Running {});
    for process in ["process-1", "process-2"] {
        let reader = durable(&runner, journal.clone(), process);
        assert_eq!(
            reader.disposition(&stored),
            InvocationDisposition::RecoveryRequired
        );
        assert_recovery_required(submit(&reader, &request, "go").await);
    }
    assert_eq!(model.calls(), 0);
}

#[tokio::test]
async fn an_attempt_lost_mid_run_requires_recovery_in_every_process() {
    let model = ScriptedModel::new(work_then_finish());
    let tool = WorkTool::new(Behavior::Block(Arc::new(AtomicBool::new(false))));
    let runner = runner(model.clone(), Some(tool.clone()));
    let journal = Arc::new(InMemoryInvocationJournal::default());
    let owner = durable(&runner, journal.clone(), "process-1");
    let request = request();

    {
        let run = submit(&owner, &request, "go");
        let mut run = std::pin::pin!(run);
        assert!(poll_once(run.as_mut()).is_pending());
        assert_eq!(tool.executions(), 1);
        let running = journal.load(request.invocation.id).await.unwrap().unwrap();
        assert_eq!(owner.disposition(&running), InvocationDisposition::Running);
        // The process dies here: the run's future is dropped mid-tool.
    }

    let stored = journal.load(request.invocation.id).await.unwrap().unwrap();
    assert_eq!(stored.state, InvocationState::Running {});
    for process in ["process-1", "process-2"] {
        let reader = durable(&runner, journal.clone(), process);
        assert_eq!(
            reader.disposition(&stored),
            InvocationDisposition::RecoveryRequired
        );
        assert_recovery_required(submit(&reader, &request, "go").await);
    }
    assert_eq!(tool.executions(), 1);
    assert_eq!(model.calls(), 1);
}

#[tokio::test]
async fn a_duplicate_of_a_running_invocation_is_refused_without_running() {
    let gate = Arc::new(AtomicBool::new(false));
    let model = ScriptedModel::new(work_then_finish());
    let tool = WorkTool::new(Behavior::Block(gate.clone()));
    let runner = runner(model.clone(), Some(tool.clone()));
    let journal = Arc::new(InMemoryInvocationJournal::default());
    let owner = durable(&runner, journal.clone(), "process-1");
    let request = request();

    let run = submit(&owner, &request, "go");
    let mut run = std::pin::pin!(run);
    assert!(poll_once(run.as_mut()).is_pending());

    let same_process = durable(&runner, journal.clone(), "process-1");
    for runner in [&owner, &same_process] {
        assert!(matches!(
            submit(runner, &request, "go").await,
            Err(DurableInvocationError::ActiveInFlight { .. })
        ));
    }
    // Another process lifetime cannot tell that this attempt is alive.
    assert_recovery_required(
        submit(
            &durable(&runner, journal.clone(), "process-2"),
            &request,
            "go",
        )
        .await,
    );

    gate.store(true, Ordering::SeqCst);
    let Poll::Ready(finished) = poll_once(run.as_mut()) else {
        panic!("the run should finish once the tool is released");
    };
    assert!(matches!(
        dispatched_state(finished),
        InvocationState::Completed { .. }
    ));
    assert!(matches!(
        submit(&owner, &request, "go").await.unwrap(),
        DurableInvocation::Replayed { .. }
    ));
    assert_eq!(tool.executions(), 1);
    assert_eq!(model.calls(), 2);
}

#[tokio::test]
async fn a_failed_settlement_returns_the_outcome_and_leaves_recovery_required() {
    let model = ScriptedModel::new([ModelResponse::final_output("done")]);
    let runner = runner(model.clone(), None);
    let journal = FaultyJournal::failing(SETTLEMENT, false);
    let owner = durable(&runner, journal.clone(), "process-1");
    let request = request();

    match submit(&owner, &request, "go").await {
        Err(DurableInvocationError::SettlementFailed { outcome, .. }) => {
            assert_eq!(outcome.unwrap().final_output, "done");
        }
        other => panic!("expected a settlement failure, got {other:?}"),
    }
    let stored = journal.load(request.invocation.id).await.unwrap().unwrap();
    assert_eq!(stored.state, InvocationState::Running {});
    assert_recovery_required(submit(&owner, &request, "go").await);
    assert_eq!(model.calls(), 1);
}

#[tokio::test]
async fn a_failure_before_the_first_model_turn_is_rejected() {
    let model = ScriptedModel::new([]);
    let runner = runner(model.clone(), None);
    let durable = durable(
        &runner,
        Arc::new(InMemoryInvocationJournal::default()),
        "process-1",
    );
    let request = InvocationRequest::new(
        InvocationContext::root(InvocationId::new()),
        InvocationSource::Caller,
        AgentRef::with_revision("worker", "rev-2").unwrap(),
    );

    assert_eq!(
        dispatched_state(submit(&durable, &request, "go").await),
        InvocationState::Rejected {
            kind: InvocationFailureKind::Configuration
        }
    );
    assert!(matches!(
        submit(&durable, &request, "go").await.unwrap(),
        DurableInvocation::Replayed { .. }
    ));
    assert_eq!(model.calls(), 0);
}

#[tokio::test]
async fn an_execution_failure_is_recorded_with_its_kind() {
    let model = ScriptedModel::new([]);
    let runner = runner(model.clone(), None);
    let durable = durable(
        &runner,
        Arc::new(InMemoryInvocationJournal::default()),
        "process-1",
    );
    let request = request();

    assert_eq!(
        dispatched_state(submit(&durable, &request, "go").await),
        InvocationState::Failed {
            kind: InvocationFailureKind::Model,
            turns: 1
        }
    );
    assert!(matches!(
        submit(&durable, &request, "go").await.unwrap(),
        DurableInvocation::Replayed { .. }
    ));
    assert_eq!(model.calls(), 1);
}

#[tokio::test]
async fn an_acknowledged_cancellation_is_settled() {
    let model = ScriptedModel::new([ModelResponse::final_output("unused")]);
    let runner = runner(model.clone(), None);
    let durable = durable(
        &runner,
        Arc::new(InMemoryInvocationJournal::default()),
        "process-1",
    );
    let request = request();
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let outcome = durable
        .run(
            request.clone(),
            "go",
            &(),
            RunOptions::default(),
            RunControls::new().with_cancellation(cancellation),
        )
        .await;
    assert_eq!(
        dispatched_state(outcome),
        InvocationState::Cancelled { turns: 0 }
    );
    assert!(matches!(
        submit(&durable, &request, "go").await.unwrap(),
        DurableInvocation::Replayed { .. }
    ));
    assert_eq!(model.calls(), 0);
}

#[tokio::test]
async fn cancellation_ambiguity_is_kept_and_never_replayed_or_rerun() {
    let cancellation = CancellationToken::new();
    let model = ScriptedModel::new(work_then_finish());
    let tool = WorkTool::new(Behavior::CancelThenFail(cancellation.clone()));
    let runner = runner(model.clone(), Some(tool.clone()));
    let durable = durable(
        &runner,
        Arc::new(InMemoryInvocationJournal::default()),
        "process-1",
    );
    let request = request();

    let outcome = durable
        .run(
            request.clone(),
            "go",
            &(),
            RunOptions::default(),
            RunControls::new().with_cancellation(cancellation),
        )
        .await;
    assert_eq!(
        dispatched_state(outcome),
        InvocationState::TerminationUnknown {
            call_id: "c1".to_owned()
        }
    );
    assert_recovery_required(submit(&durable, &request, "go").await);
    assert_eq!(tool.executions(), 1);
}

#[tokio::test]
async fn an_unresolved_descendant_requires_recovery() {
    let model = ScriptedModel::new(work_then_finish());
    let tool = WorkTool::new(Behavior::ReportUnknownChild);
    let runner = runner(model.clone(), Some(tool.clone()));
    let durable = durable(
        &runner,
        Arc::new(InMemoryInvocationJournal::default()),
        "process-1",
    );
    let request = request();

    assert_eq!(
        dispatched_state(submit(&durable, &request, "go").await),
        InvocationState::RecoveryRequired {
            call_id: "c1".to_owned()
        }
    );
    assert_recovery_required(submit(&durable, &request, "go").await);
    assert_eq!(tool.executions(), 1);
}

#[tokio::test]
async fn a_file_journal_keeps_records_across_reopen_and_stores_no_content() {
    let directory = tempfile::tempdir().unwrap();
    let model = ScriptedModel::new([ModelResponse::final_output("confidential answer")]);
    let runner = runner(model.clone(), None);
    let request = request();

    let record = {
        let journal = Arc::new(FileInvocationJournal::new(directory.path()).unwrap());
        let owner = durable(&runner, journal, "process-1");
        submit(&owner, &request, "confidential prompt")
            .await
            .unwrap()
            .record()
            .clone()
    };

    let reopened = Arc::new(FileInvocationJournal::new(directory.path()).unwrap());
    assert_eq!(reopened.list().await.unwrap(), vec![record.clone()]);
    let after_restart = durable(&runner, reopened, "process-2");
    match submit(&after_restart, &request, "confidential prompt")
        .await
        .unwrap()
    {
        DurableInvocation::Replayed { record: replayed } => assert_eq!(*replayed, record),
        other => panic!("expected a replay, got {other:?}"),
    }
    assert_eq!(model.calls(), 1);

    let mut checkpoints = Vec::new();
    for entry in walk(directory.path()) {
        let bytes = std::fs::read(&entry).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("confidential"), "{entry:?} stores content");
        if entry
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            checkpoints.push(serde_json::from_slice::<Value>(&bytes).unwrap());
        }
    }
    // Reserved, running and completed: one checkpoint per transition.
    assert_eq!(checkpoints.len(), 3);
    for checkpoint in &checkpoints {
        assert_eq!(checkpoint["schema_version"], json!(1));
        assert!(checkpoint["record"].is_object());
    }
}

#[tokio::test]
async fn a_file_journal_refuses_an_unknown_schema_version() {
    let directory = tempfile::tempdir().unwrap();
    let journal = FileInvocationJournal::new(directory.path()).unwrap();
    let model = ScriptedModel::new([ModelResponse::final_output("done")]);
    let runner = runner(model, None);
    let request = request();
    submit(
        &durable(&runner, Arc::new(journal.clone()), "process-1"),
        &request,
        "go",
    )
    .await
    .unwrap();

    for entry in walk(directory.path()) {
        if entry
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let mut checkpoint: Value =
                serde_json::from_slice(&std::fs::read(&entry).unwrap()).unwrap();
            checkpoint["schema_version"] = json!(2);
            std::fs::write(&entry, serde_json::to_vec(&checkpoint).unwrap()).unwrap();
        }
    }

    let error = journal.load(request.invocation.id).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "unsupported invocation journal schema version 2"
    );
}

#[test]
fn the_request_digest_covers_identity_target_input_and_options() {
    let request = request();
    let options = RunOptions::default();
    let digest = invocation_request_digest(&request, "go", &options);
    assert_eq!(digest, invocation_request_digest(&request, "go", &options));

    let other_id = InvocationRequest::new(
        InvocationContext::root(InvocationId::new()),
        InvocationSource::Caller,
        worker(),
    );
    let other_target = InvocationRequest::new(
        request.invocation.clone(),
        InvocationSource::Caller,
        AgentRef::with_revision("worker", "rev-2").unwrap(),
    );
    let other_source = InvocationRequest::new(
        request.invocation.clone(),
        InvocationSource::Agent {
            agent: AgentRef::new("parent").unwrap(),
        },
        worker(),
    );
    let variants = [
        invocation_request_digest(&other_id, "go", &options),
        invocation_request_digest(&other_target, "go", &options),
        invocation_request_digest(&other_source, "go", &options),
        invocation_request_digest(&request, "stop", &options),
        invocation_request_digest(
            &request,
            "go",
            &RunOptions {
                max_turns: 2,
                ..RunOptions::default()
            },
        ),
        invocation_request_digest(
            &request,
            "go",
            &RunOptions {
                session_id: Some("session".to_owned()),
                ..RunOptions::default()
            },
        ),
    ];
    for variant in variants {
        assert_ne!(variant, digest);
    }
}

#[test]
fn the_record_has_a_closed_snake_case_wire_shape() {
    let record = InvocationRecord {
        delegation: None,
        invocation: InvocationContext::root(
            "11111111-1111-4111-8111-111111111111".parse().unwrap(),
        ),
        source: InvocationSource::Caller,
        requested_target: worker(),
        request_digest: ContentDigest::of_bytes(b"request"),
        state: InvocationState::TerminationUnknown {
            call_id: "c1".to_owned(),
        },
        attempt: None,
    };
    let encoded = serde_json::to_value(&record).unwrap();
    assert_eq!(
        encoded["state"],
        json!({ "type": "termination_unknown", "call_id": "c1" })
    );
    assert_eq!(
        serde_json::from_value::<InvocationRecord>(encoded.clone()).unwrap(),
        record
    );
    let mut extra = encoded;
    extra["prompt"] = json!("must not be accepted");
    assert!(serde_json::from_value::<InvocationRecord>(extra).is_err());
}

#[test]
fn durable_run_futures_are_send() {
    fn assert_send<T: Send>(_: &T) {}
    let model = ScriptedModel::new([]);
    let runner = runner(model, None);
    let durable = durable(
        &runner,
        Arc::new(InMemoryInvocationJournal::default()),
        "process-1",
    );
    let request = request();
    let run = submit(&durable, &request, "go");
    assert_send(&run);
}

fn walk(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else {
                files.push(entry.path());
            }
        }
    }
    files
}

struct NoContext;

#[async_trait]
impl coven_agents::ContextResolver<()> for NoContext {
    async fn resolve(
        &self,
        _: &coven_agents::ContextRef,
        _: &(),
    ) -> Result<Vec<coven_agents::RunItem>, BoxError> {
        panic!("empty manifest must not resolve context")
    }
}

fn child() -> coven_agents::PreparedDelegation {
    child_with_work(true)
}

fn child_with_work(allow: bool) -> coven_agents::PreparedDelegation {
    use coven_agents::{
        AuthorityScope, ContextManifest, DelegationBudget, DelegationGrant, DelegationSpec,
        PreparedDelegation, ProjectRef, ResultContract, TaskInput,
    };
    let authority = AuthorityScope::new(ProjectRef::new("project").unwrap());
    let authority = if allow {
        authority.with_tool("work").unwrap()
    } else {
        authority
    };
    let grant = DelegationGrant {
        invocation: InvocationContext::root(InvocationId::new()),
        agent: AgentRef::new("parent").unwrap(),
        authority: authority.clone().with_delegation(),
        context: ContextManifest::default(),
        budget: DelegationBudget::new(2, 0).unwrap(),
    };
    PreparedDelegation::prepare(
        &grant,
        DelegationSpec {
            target: worker(),
            task: TaskInput::new("PRIVATE_CHILD_TASK").unwrap(),
            context: ContextManifest::default(),
            authority,
            budget: DelegationBudget::new(2, 0).unwrap(),
            result: ResultContract::new(64).unwrap(),
        },
    )
    .unwrap()
}

#[tokio::test]
async fn delegated_child_is_journaled_with_parent_and_digests_without_content() {
    let delegation = child();
    let identity = delegation.invocation().clone();
    let digests = delegation.digests().clone();
    let model = ScriptedModel::new([ModelResponse::final_output("PRIVATE_CHILD_OUTPUT")]);
    let runner = runner(model.clone(), None);
    let dir = tempfile::tempdir().unwrap();
    let journal = Arc::new(FileInvocationJournal::new(dir.path()).unwrap());
    let result = durable(&runner, journal.clone(), "process")
        .run_delegation(delegation, &NoContext, &(), RunControls::default())
        .await
        .unwrap();
    assert_eq!(result.record().invocation, identity);
    assert_eq!(result.record().delegation.as_ref(), Some(&digests));
    assert!(matches!(
        result.record().state,
        InvocationState::Completed { .. }
    ));
    drop(journal);
    let reopened = FileInvocationJournal::new(dir.path()).unwrap();
    assert_eq!(
        reopened.load(identity.id).await.unwrap().as_ref(),
        Some(result.record())
    );
    for path in walk(dir.path()) {
        let text = String::from_utf8_lossy(&std::fs::read(path).unwrap()).into_owned();
        assert!(!text.contains("PRIVATE_CHILD_TASK"));
        assert!(!text.contains("PRIVATE_CHILD_OUTPUT"));
    }
    assert_eq!(model.calls(), 1);
}

#[tokio::test]
async fn delegated_child_unknown_adoption_never_dispatches() {
    let delegation = child();
    let identity = delegation.invocation().clone();
    let model = ScriptedModel::new([ModelResponse::final_output("done")]);
    let runner = runner(model.clone(), None);
    let journal = FaultyJournal::failing(ADOPTION, true);
    let result = durable(&runner, journal.clone(), "process")
        .run_delegation(delegation, &NoContext, &(), RunControls::default())
        .await;
    assert!(matches!(
        result,
        Err(DurableInvocationError::AdoptionUnknown { .. })
    ));
    let record = journal.load(identity.id).await.unwrap().unwrap();
    assert_eq!(record.invocation, identity);
    assert!(record.delegation.is_some());
    assert_eq!(
        durable(&runner, journal, "restart").disposition(&record),
        InvocationDisposition::RecoveryRequired
    );
    assert_eq!(model.calls(), 0);
}

#[tokio::test]
async fn dropped_delegated_child_keeps_parent_link_and_requires_recovery() {
    let delegation = child();
    let identity = delegation.invocation().clone();
    let model = ScriptedModel::new(work_then_finish());
    let tool = WorkTool::new(Behavior::Block(Arc::new(AtomicBool::new(false))));
    let runner = runner(model.clone(), Some(tool.clone()));
    let journal = Arc::new(InMemoryInvocationJournal::default());
    let owner = durable(&runner, journal.clone(), "process");
    {
        let run = owner.run_delegation(delegation, &NoContext, &(), RunControls::default());
        let mut run = std::pin::pin!(run);
        assert!(poll_once(run.as_mut()).is_pending());
        assert_eq!(tool.executions(), 1);
        let running = journal.load(identity.id).await.unwrap().unwrap();
        assert_eq!(owner.disposition(&running), InvocationDisposition::Running);
    }
    let record = journal.load(identity.id).await.unwrap().unwrap();
    assert_eq!(record.invocation, identity);
    assert!(record.delegation.is_some());
    assert_eq!(
        durable(&runner, journal, "restart").disposition(&record),
        InvocationDisposition::RecoveryRequired
    );
    assert_eq!(tool.executions(), 1);
    assert_eq!(model.calls(), 1);
}

#[tokio::test]
async fn direct_submission_cannot_reuse_a_delegated_child_id() {
    let delegation = child();
    let request = InvocationRequest::new(
        delegation.invocation().clone(),
        InvocationSource::Agent {
            agent: delegation.source().clone(),
        },
        worker(),
    );
    let input = delegation.spec().task.as_str().to_owned();
    let model = ScriptedModel::new([ModelResponse::final_output("done")]);
    let runner = runner(model.clone(), None);
    let journal = Arc::new(InMemoryInvocationJournal::default());
    let owner = durable(&runner, journal, "process");
    owner
        .run_delegation(delegation, &NoContext, &(), RunControls::default())
        .await
        .unwrap();
    let result = owner
        .run(
            request,
            input,
            &(),
            RunOptions {
                max_turns: 2,
                max_handoffs: 0,
                session_id: None,
            },
            RunControls::default(),
        )
        .await;
    assert!(matches!(
        result,
        Err(DurableInvocationError::Conflict { .. })
    ));
    assert_eq!(model.calls(), 1);
}

#[tokio::test]
async fn delegated_child_settlement_failure_keeps_uncertainty() {
    let delegation = child();
    let identity = delegation.invocation().clone();
    let model = ScriptedModel::new([ModelResponse::final_output("done")]);
    let runner = runner(model.clone(), None);
    let journal = FaultyJournal::failing(SETTLEMENT, false);
    let error = durable(&runner, journal.clone(), "process")
        .run_delegation(delegation, &NoContext, &(), RunControls::default())
        .await
        .unwrap_err();
    assert!(error.leaves_unknown_effect());
    assert!(matches!(
        error,
        DurableInvocationError::SettlementFailed { .. }
    ));
    let record = journal.load(identity.id).await.unwrap().unwrap();
    assert_eq!(record.invocation, identity);
    assert_eq!(
        durable(&runner, journal, "restart").disposition(&record),
        InvocationDisposition::RecoveryRequired
    );
    assert_eq!(model.calls(), 1);
}

#[tokio::test]
async fn durable_child_still_enforces_attenuated_tool_authority() {
    let model = ScriptedModel::new(work_then_finish());
    let tool = WorkTool::new(Behavior::Block(Arc::new(AtomicBool::new(true))));
    let runner = runner(model.clone(), Some(tool.clone()));
    let journal = Arc::new(InMemoryInvocationJournal::default());
    let result = durable(&runner, journal, "process")
        .run_delegation(
            child_with_work(false),
            &NoContext,
            &(),
            RunControls::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        result.record().state,
        InvocationState::Failed {
            kind: InvocationFailureKind::Authority,
            ..
        }
    ));
    assert_eq!(tool.executions(), 0);
    assert_eq!(model.calls(), 1);
}

#[tokio::test]
async fn durable_child_ambiguity_propagates_to_parent() {
    let model = ScriptedModel::new(work_then_finish());
    let tool = WorkTool::new(Behavior::ReportDurableUnknownChild);
    let runner = runner(model, Some(tool));
    let journal = Arc::new(InMemoryInvocationJournal::default());
    let result = submit(&durable(&runner, journal, "process"), &request(), "go").await;
    assert!(matches!(
        dispatched_state(result),
        InvocationState::RecoveryRequired { .. }
    ));
}

struct SlowAdoptionJournal {
    inner: InMemoryInvocationJournal,
    clock: Arc<coven_agents::ManualClock>,
}

#[async_trait]
impl InvocationJournal for SlowAdoptionJournal {
    async fn load(&self, id: InvocationId) -> Result<Option<InvocationRecord>, BoxError> {
        self.inner.load(id).await
    }
    async fn list(&self) -> Result<Vec<InvocationRecord>, BoxError> {
        self.inner.list().await
    }
    async fn compare_and_set(
        &self,
        id: InvocationId,
        expected: Option<&InvocationRecord>,
        next: &InvocationRecord,
    ) -> Result<bool, BoxError> {
        if matches!(next.state, InvocationState::Running {}) {
            self.clock.advance(std::time::Duration::from_secs(1));
        }
        self.inner.compare_and_set(id, expected, next).await
    }
}

#[tokio::test]
async fn child_deadline_includes_time_spent_journaling() {
    let clock = Arc::new(coven_agents::ManualClock::new());
    let model = ScriptedModel::new([ModelResponse::final_output("too late")]);
    let runner = Arc::new(
        Runner::new([Agent::new("worker", "Worker", "work", model.clone())
            .with_revision("rev-1".parse().unwrap())])
        .unwrap()
        .with_clock(clock.clone()),
    );
    let journal = Arc::new(SlowAdoptionJournal {
        inner: InMemoryInvocationJournal::default(),
        clock,
    });
    let result = durable(&runner, journal, "process")
        .run_delegation(
            child(),
            &NoContext,
            &(),
            RunControls::new().with_timeout(std::time::Duration::from_secs(1)),
        )
        .await
        .unwrap();
    assert!(matches!(
        result.record().state,
        InvocationState::Cancelled { turns: 0 }
    ));
    assert_eq!(model.calls(), 0);
}

fn capacity_one() -> Arc<InvocationCapacity> {
    Arc::new(InvocationCapacity::new(NonZeroUsize::new(1).unwrap()))
}

#[tokio::test]
async fn capacity_is_shared_and_duplicates_still_report_their_actual_state() {
    let gate = Arc::new(AtomicBool::new(false));
    let model = ScriptedModel::new(work_then_finish());
    let tool = WorkTool::new(Behavior::Block(gate.clone()));
    let runner = runner(model.clone(), Some(tool.clone()));
    let journal = Arc::new(InMemoryInvocationJournal::default());
    let capacity = capacity_one();
    let owner =
        DurableInvocationRunner::new(runner.clone(), journal.clone(), "process", capacity.clone());
    let peer = DurableInvocationRunner::new(runner, journal.clone(), "process", capacity.clone());
    let first = request();
    let second = request();
    let mut pending = Box::pin(submit(&owner, &first, "go"));
    assert!(poll_once(pending.as_mut()).is_pending());
    assert_eq!(capacity.used(), 1);
    assert!(matches!(
        submit(&peer, &first, "go").await,
        Err(DurableInvocationError::ActiveInFlight { .. })
    ));
    assert!(matches!(
        submit(&peer, &first, "different").await,
        Err(DurableInvocationError::Conflict { .. })
    ));
    assert!(matches!(
        submit(&peer, &second, "go").await,
        Err(DurableInvocationError::CapacityUnavailable)
    ));
    let reserved = journal.load(second.invocation.id).await.unwrap().unwrap();
    assert_eq!(reserved.state, InvocationState::Reserved {});
    assert_eq!(reserved.attempt, None);
    assert_eq!(tool.executions(), 1);
    assert_eq!(model.calls(), 1);

    gate.store(true, Ordering::SeqCst);
    assert!(poll_once(pending.as_mut()).is_ready());
    assert_eq!(capacity.used(), 0);
    // The reserved request can be submitted explicitly after capacity is free.
    let second_model = ScriptedModel::new([ModelResponse::final_output("second")]);
    let second_runner = runner_with_model(second_model.clone());
    let next =
        DurableInvocationRunner::new(second_runner, journal.clone(), "process", capacity.clone());
    assert!(matches!(
        submit(&next, &second, "go").await.unwrap(),
        DurableInvocation::Dispatched { .. }
    ));
    assert_eq!(second_model.calls(), 1);
    assert_eq!(capacity.used(), 0);
}

fn runner_with_model(model: Arc<ScriptedModel>) -> Arc<Runner<()>> {
    runner(model, None)
}

#[tokio::test]
async fn dropping_an_adopted_future_keeps_its_capacity_charged() {
    let model = ScriptedModel::new(work_then_finish());
    let tool = WorkTool::new(Behavior::Block(Arc::new(AtomicBool::new(false))));
    let runner = runner(model.clone(), Some(tool.clone()));
    let journal = Arc::new(InMemoryInvocationJournal::default());
    let capacity = capacity_one();
    let owner = DurableInvocationRunner::new(runner, journal.clone(), "process", capacity.clone());
    let first = request();
    let mut pending = Box::pin(submit(&owner, &first, "go"));
    assert!(poll_once(pending.as_mut()).is_pending());
    drop(pending);
    assert_eq!(capacity.used(), 1);
    assert_recovery_required(submit(&owner, &first, "go").await);
    assert!(matches!(
        submit(&owner, &request(), "go").await,
        Err(DurableInvocationError::CapacityUnavailable)
    ));
    assert_eq!(tool.executions(), 1);
    assert_eq!(model.calls(), 1);
}

#[tokio::test]
async fn uncertain_adoption_settlement_and_tool_outcomes_keep_capacity_charged() {
    for (write, after) in [
        (ADOPTION, false),
        (ADOPTION, true),
        (SETTLEMENT, false),
        (SETTLEMENT, true),
    ] {
        let model = ScriptedModel::new([ModelResponse::final_output("done")]);
        let journal = FaultyJournal::failing(write, after);
        let capacity = capacity_one();
        let owner = DurableInvocationRunner::new(
            runner(model.clone(), None),
            journal,
            "process",
            capacity.clone(),
        );
        let first = request();
        let result = submit(&owner, &first, "go").await;
        assert!(
            matches!(
                result,
                Err(DurableInvocationError::AdoptionUnknown { .. }
                    | DurableInvocationError::SettlementFailed { .. })
            ),
            "{result:?}"
        );
        assert_eq!(capacity.used(), 1, "write {write}, after {after}");
        let calls = model.calls();
        assert!(matches!(
            submit(&owner, &request(), "go").await,
            Err(DurableInvocationError::CapacityUnavailable)
        ));
        assert_eq!(model.calls(), calls);
        // A durable settled duplicate is readable even when the lost reply
        // left this process's capacity conservatively charged.
        if write == SETTLEMENT && after {
            assert!(matches!(
                submit(&owner, &first, "go").await.unwrap(),
                DurableInvocation::Replayed { .. }
            ));
        }
    }
    for behavior in [
        Behavior::ReportUnknownChild,
        Behavior::CancelThenFail(CancellationToken::new()),
    ] {
        let model = ScriptedModel::new(work_then_finish());
        let tool = WorkTool::new(behavior);
        let capacity = capacity_one();
        let cancellation = match &tool.behavior {
            Behavior::CancelThenFail(token) => token.clone(),
            _ => CancellationToken::new(),
        };
        let owner = DurableInvocationRunner::new(
            runner(model, Some(tool)),
            Arc::new(InMemoryInvocationJournal::default()),
            "process",
            capacity.clone(),
        );
        let result = owner
            .run(
                request(),
                "go",
                &(),
                RunOptions::default(),
                RunControls::new().with_cancellation(cancellation),
            )
            .await
            .unwrap();
        assert!(result.record().state.leaves_unknown_effect());
        assert_eq!(capacity.used(), 1);
    }
}

#[tokio::test]
async fn known_rejection_and_cancellation_release_capacity() {
    let model = ScriptedModel::new([ModelResponse::final_output("done")]);
    let capacity = capacity_one();
    let owner = DurableInvocationRunner::new(
        runner(model.clone(), None),
        Arc::new(InMemoryInvocationJournal::default()),
        "process",
        capacity.clone(),
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let result = owner
        .run(
            request(),
            "go",
            &(),
            RunOptions::default(),
            RunControls::new().with_cancellation(cancellation),
        )
        .await
        .unwrap();
    assert!(matches!(
        result.record().state,
        InvocationState::Cancelled { .. }
    ));
    assert_eq!(capacity.used(), 0);
    let mut invalid = request();
    invalid.requested_target = AgentRef::new("missing").unwrap();
    let result = submit(&owner, &invalid, "go").await.unwrap();
    assert!(matches!(
        result.record().state,
        InvocationState::Rejected { .. }
    ));
    assert_eq!(capacity.used(), 0);
    assert_eq!(model.calls(), 0);
}

#[tokio::test]
async fn delegated_and_direct_work_share_capacity_before_context_or_model_execution() {
    let gate = Arc::new(AtomicBool::new(false));
    let model = ScriptedModel::new(work_then_finish());
    let tool = WorkTool::new(Behavior::Block(gate.clone()));
    let runner = runner(model.clone(), Some(tool.clone()));
    let journal = Arc::new(InMemoryInvocationJournal::default());
    let capacity = capacity_one();
    let owner = DurableInvocationRunner::new(runner, journal.clone(), "process", capacity.clone());
    let direct = request();
    let mut pending = Box::pin(submit(&owner, &direct, "go"));
    assert!(poll_once(pending.as_mut()).is_pending());
    let delegation = child();
    let identity = delegation.invocation().clone();
    let result = owner
        .run_delegation(delegation, &NoContext, &(), RunControls::default())
        .await;
    assert!(matches!(
        result,
        Err(DurableInvocationError::CapacityUnavailable)
    ));
    let stored = journal.load(identity.id).await.unwrap().unwrap();
    assert_eq!(stored.invocation.parent_id, identity.parent_id);
    assert_eq!(stored.state, InvocationState::Reserved {});
    assert_eq!(stored.attempt, None);
    assert!(stored.delegation.is_some());
    assert_eq!(model.calls(), 1);
    assert_eq!(tool.executions(), 1);
    gate.store(true, Ordering::SeqCst);
    assert!(poll_once(pending.as_mut()).is_ready());
    assert_eq!(capacity.used(), 0);
}

struct ContendedAdoptionJournal {
    inner: InMemoryInvocationJournal,
    contested: AtomicBool,
}

#[async_trait]
impl InvocationJournal for ContendedAdoptionJournal {
    async fn load(&self, id: InvocationId) -> Result<Option<InvocationRecord>, BoxError> {
        self.inner.load(id).await
    }
    async fn list(&self) -> Result<Vec<InvocationRecord>, BoxError> {
        self.inner.list().await
    }
    async fn compare_and_set(
        &self,
        id: InvocationId,
        expected: Option<&InvocationRecord>,
        next: &InvocationRecord,
    ) -> Result<bool, BoxError> {
        if matches!(next.state, InvocationState::Running {})
            && !self.contested.swap(true, Ordering::SeqCst)
        {
            return Ok(false);
        }
        self.inner.compare_and_set(id, expected, next).await
    }
}

#[tokio::test]
async fn failed_reservation_and_contended_adoption_do_not_leak_capacity() {
    let model = ScriptedModel::new([ModelResponse::final_output("done")]);
    let capacity = capacity_one();
    let owner = DurableInvocationRunner::new(
        runner(model.clone(), None),
        FaultyJournal::failing(RESERVATION, false),
        "process",
        capacity.clone(),
    );
    assert!(matches!(
        submit(&owner, &request(), "go").await,
        Err(DurableInvocationError::JournalFailed { .. })
    ));
    assert_eq!(capacity.used(), 0);
    assert_eq!(model.calls(), 0);
    let journal = Arc::new(ContendedAdoptionJournal {
        inner: InMemoryInvocationJournal::default(),
        contested: AtomicBool::new(false),
    });
    let owner = DurableInvocationRunner::new(
        runner(model.clone(), None),
        journal,
        "process",
        capacity.clone(),
    );
    assert!(matches!(
        submit(&owner, &request(), "go").await.unwrap(),
        DurableInvocation::Dispatched { .. }
    ));
    assert_eq!(capacity.used(), 0);
    assert_eq!(model.calls(), 1);
}

struct InterruptedAdoptionJournal {
    inner: InMemoryInvocationJournal,
}

#[async_trait]
impl InvocationJournal for InterruptedAdoptionJournal {
    async fn load(&self, id: InvocationId) -> Result<Option<InvocationRecord>, BoxError> {
        self.inner.load(id).await
    }
    async fn list(&self) -> Result<Vec<InvocationRecord>, BoxError> {
        self.inner.list().await
    }
    async fn compare_and_set(
        &self,
        id: InvocationId,
        expected: Option<&InvocationRecord>,
        next: &InvocationRecord,
    ) -> Result<bool, BoxError> {
        let changed = self.inner.compare_and_set(id, expected, next).await?;
        if matches!(next.state, InvocationState::Running {}) {
            // Adoption is durable but its response has not arrived. Dropping
            // this future must retain admission without dispatching the model.
            std::future::pending::<()>().await;
        }
        Ok(changed)
    }
}

#[tokio::test]
async fn dropping_the_adoption_write_keeps_capacity_even_before_model_dispatch() {
    let journal = Arc::new(InterruptedAdoptionJournal {
        inner: InMemoryInvocationJournal::default(),
    });
    let model = ScriptedModel::new([ModelResponse::final_output("done")]);
    let capacity = capacity_one();
    let owner = DurableInvocationRunner::new(
        runner(model.clone(), None),
        journal.clone(),
        "process",
        capacity.clone(),
    );
    let first = request();
    let mut pending = Box::pin(submit(&owner, &first, "go"));
    assert!(poll_once(pending.as_mut()).is_pending());
    assert_eq!(
        journal
            .load(first.invocation.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        InvocationState::Running {}
    );
    drop(pending);
    assert_recovery_required(submit(&owner, &first, "go").await);
    assert_eq!(capacity.used(), 1);
    assert_eq!(model.calls(), 0);
}
