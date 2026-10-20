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
    FileInvocationJournal, InMemoryInvocationJournal, InvocationContext, InvocationDisposition,
    InvocationFailureKind, InvocationId, InvocationJournal, InvocationRecord, InvocationRequest,
    InvocationSource, InvocationState, Model, ModelAction, ModelRequest, ModelResponse,
    RunControls, RunError, RunOptions, RunSignal, Runner, StopReason, Tool, ToolCall,
    ToolDefinition,
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
    DurableInvocationRunner::new(runner.clone(), journal, process)
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
