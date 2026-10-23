//! Lifecycle across a delegation boundary: cancellation propagates from
//! parent to child and not back, a child's deadline never outlives its
//! parent's, concurrent children are bounded per parent, and a descendant
//! whose termination is unknown makes its parent require recovery.

use std::{
    collections::VecDeque,
    future::poll_fn,
    io,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::{Poll, Waker},
    time::Duration,
};

use async_trait::async_trait;
use coven_agents::{
    Agent, AgentRef, AuthorityError, AuthorityScope, BoxError, CancellationToken, ContextManifest,
    ContextRef, ContextResolver, DelegationBudget, DelegationDisposition, DelegationGrant,
    DelegationSpec, InvocationContext, InvocationEvent, InvocationEventKind, InvocationFailureKind,
    InvocationId, InvocationObserver, InvocationRequest, InvocationSource, ManualClock, Model,
    ModelAction, ModelRequest, ModelResponse, PreparedDelegation, ProjectRef, ResultContract,
    RunControls, RunError, RunEvent, RunFailureKind, RunItem, RunObserver, RunOptions, RunSignal,
    Runner, StopReason, TaskInput, Tool, ToolCall, ToolDefinition,
};
use serde_json::{json, Value};

/// A one-shot gate built on std only: `wait` stays pending until `open`.
#[derive(Default)]
struct Gate {
    open: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl Gate {
    fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }

    async fn wait(&self) {
        poll_fn(|cx| {
            if self.open.load(Ordering::SeqCst) {
                return Poll::Ready(());
            }
            *self.waker.lock().unwrap() = Some(cx.waker().clone());
            if self.open.load(Ordering::SeqCst) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
}

type Hook = Box<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Events {
    runs: Mutex<Vec<RunEvent>>,
    invocations: Mutex<Vec<InvocationEvent>>,
}

impl RunObserver for Events {
    fn on_event(&self, event: &RunEvent) {
        self.runs.lock().unwrap().push(event.clone());
    }
}

impl InvocationObserver for Events {
    fn on_event(&self, event: &InvocationEvent) {
        self.invocations.lock().unwrap().push(event.clone());
    }
}

struct ScriptedModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    hook: Option<Hook>,
    entered: Option<Arc<Gate>>,
    release: Option<Arc<Gate>>,
}

impl ScriptedModel {
    fn new(responses: impl IntoIterator<Item = ModelResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            hook: None,
            entered: None,
            release: None,
        })
    }

    fn hooked(responses: impl IntoIterator<Item = ModelResponse>, hook: Hook) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            hook: Some(hook),
            entered: None,
            release: None,
        })
    }

    /// Opens `entered` when called, then waits for `release`.
    fn gated(
        responses: impl IntoIterator<Item = ModelResponse>,
        entered: Arc<Gate>,
        release: Arc<Gate>,
    ) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            hook: None,
            entered: Some(entered),
            release: Some(release),
        })
    }
}

#[async_trait]
impl Model<()> for ScriptedModel {
    async fn generate(
        &self,
        _request: ModelRequest,
        _context: &(),
    ) -> Result<ModelResponse, BoxError> {
        if let Some(hook) = &self.hook {
            hook();
        }
        if let Some(entered) = &self.entered {
            entered.open();
        }
        if let Some(release) = &self.release {
            release.wait().await;
        }
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| Box::new(io::Error::other("no scripted response")) as BoxError)
    }
}

/// Moves the clock while it "works", then succeeds or fails.
struct ClockTool {
    clock: Arc<ManualClock>,
    advance: Duration,
    fail: bool,
    executions: AtomicUsize,
}

#[async_trait]
impl Tool<()> for ClockTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("work", "moves the clock", json!({ "type": "object" }))
    }

    async fn execute(&self, _arguments: Value, _context: &()) -> Result<Value, BoxError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        self.clock.advance(self.advance);
        if self.fail {
            Err(Box::new(io::Error::other("connection reset")))
        } else {
            Ok(json!("worked"))
        }
    }
}

struct NoContext;

#[async_trait]
impl ContextResolver<()> for NoContext {
    async fn resolve(
        &self,
        _reference: &ContextRef,
        _context: &(),
    ) -> Result<Vec<RunItem>, BoxError> {
        unreachable!("these delegations select no context")
    }
}

fn project() -> ProjectRef {
    ProjectRef::new("proj").unwrap()
}

fn grant(parent: InvocationContext, budget: DelegationBudget) -> DelegationGrant {
    DelegationGrant {
        invocation: parent,
        agent: AgentRef::new("planner").unwrap(),
        authority: AuthorityScope::new(project())
            .with_tool("work")
            .unwrap()
            .with_delegation(),
        context: ContextManifest::default(),
        budget,
    }
}

fn spec(target: &str, budget: DelegationBudget) -> DelegationSpec {
    DelegationSpec {
        target: AgentRef::new(target).unwrap(),
        task: TaskInput::new("child task").unwrap(),
        context: ContextManifest::default(),
        authority: AuthorityScope::new(project()).with_tool("work").unwrap(),
        budget,
        result: ResultContract::new(1024).unwrap(),
    }
}

fn prepare(parent: &InvocationContext, budget: DelegationBudget) -> PreparedDelegation {
    PreparedDelegation::prepare(
        &grant(parent.clone(), DelegationBudget::new(8, 2).unwrap()),
        spec("worker", budget),
    )
    .unwrap()
}

fn root() -> InvocationContext {
    InvocationContext::root(InvocationId::new())
}

fn work_call(id: &str) -> ModelAction {
    ModelAction::ToolCall(ToolCall::new(id, "work", json!({})))
}

#[tokio::test]
async fn cancelling_the_parent_stops_the_child_and_never_the_reverse() {
    let clock = Arc::new(ManualClock::new());
    let parent_token = CancellationToken::new();
    let parent_signal = RunSignal::new(
        RunControls::new().with_cancellation(parent_token.clone()),
        clock.clone(),
    );

    // The child's own token is cancelled: the parent is unaffected.
    let detached = parent_signal.child_controls();
    detached.cancellation().unwrap().cancel();
    assert!(!parent_token.is_cancelled());
    assert_eq!(parent_signal.stop_reason(), None);

    // The parent is cancelled while the child's model call is in flight.
    let cancel_parent = parent_token.clone();
    let child_model = ScriptedModel::hooked(
        [ModelResponse::actions(vec![work_call("c1")])],
        Box::new(move || cancel_parent.cancel()),
    );
    let tool = Arc::new(ClockTool {
        clock: clock.clone(),
        advance: Duration::ZERO,
        fail: false,
        executions: AtomicUsize::new(0),
    });
    let runner =
        Runner::new([Agent::new("worker", "Worker", "work", child_model).with_tool(tool.clone())])
            .unwrap()
            .with_clock(clock);

    let failure = runner
        .run_delegation_controlled(
            prepare(&root(), DelegationBudget::new(4, 0).unwrap()),
            &NoContext,
            &(),
            parent_signal.child_controls(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        failure.failure.error,
        RunError::Cancelled {
            reason: StopReason::Cancelled,
            ..
        }
    ));
    assert_eq!(failure.disposition(), DelegationDisposition::Cancelled);
    assert_eq!(tool.executions.load(Ordering::SeqCst), 0);
}

/// Runs a child whose tool takes `tool_takes`, with the parent having
/// `parent_remaining` left and the spec allowing `own_timeout`. Returns
/// whether the child stopped on its deadline after the tool.
async fn child_stops_on_deadline(
    parent_remaining: Duration,
    own_timeout: Duration,
    tool_takes: Duration,
) -> bool {
    let clock = Arc::new(ManualClock::new());
    let parent_signal = RunSignal::new(
        RunControls::new().with_timeout(parent_remaining + Duration::from_secs(1)),
        clock.clone(),
    );
    clock.advance(Duration::from_secs(1));
    assert_eq!(parent_signal.remaining(), Some(parent_remaining));

    let tool = Arc::new(ClockTool {
        clock: clock.clone(),
        advance: tool_takes,
        fail: false,
        executions: AtomicUsize::new(0),
    });
    let model = ScriptedModel::new([
        ModelResponse::actions(vec![work_call("c1")]),
        ModelResponse::final_output("done"),
    ]);
    let runner = Runner::new([Agent::new("worker", "Worker", "work", model).with_tool(tool)])
        .unwrap()
        .with_clock(clock);

    let outcome = runner
        .run_delegation_controlled(
            prepare(
                &root(),
                DelegationBudget::new(4, 0)
                    .unwrap()
                    .with_timeout(own_timeout)
                    .unwrap(),
            ),
            &NoContext,
            &(),
            parent_signal.child_controls(),
        )
        .await;
    match outcome {
        Ok(result) => {
            assert_eq!(result.run.final_output, "done");
            false
        }
        Err(failure) => {
            assert!(matches!(
                failure.failure.error,
                RunError::Cancelled {
                    reason: StopReason::DeadlineExceeded,
                    ..
                }
            ));
            true
        }
    }
}

#[tokio::test]
async fn child_deadline_is_the_earlier_of_parent_remaining_and_its_own_timeout() {
    let secs = Duration::from_secs;
    // The parent's remaining time is the binding bound.
    assert!(child_stops_on_deadline(secs(3), secs(10), secs(4)).await);
    // The child's own timeout is the binding bound.
    assert!(child_stops_on_deadline(secs(30), secs(2), secs(3)).await);
    // Neither bound is reached.
    assert!(!child_stops_on_deadline(secs(30), secs(10), secs(4)).await);
}

#[test]
fn prepare_refuses_a_child_timeout_that_widens_the_parents() {
    let parent = grant(
        root(),
        DelegationBudget::new(8, 2)
            .unwrap()
            .with_timeout(Duration::from_secs(5))
            .unwrap(),
    );
    let refused =
        |budget| PreparedDelegation::prepare(&parent, spec("worker", budget)).unwrap_err();

    assert_eq!(
        refused(DelegationBudget::new(1, 0).unwrap()),
        AuthorityError::DeadlineWidened {
            parent_ms: 5000,
            requested_ms: None,
        }
    );
    assert_eq!(
        refused(
            DelegationBudget::new(1, 0)
                .unwrap()
                .with_timeout(Duration::from_millis(5001))
                .unwrap()
        ),
        AuthorityError::DeadlineWidened {
            parent_ms: 5000,
            requested_ms: Some(5001),
        }
    );
    assert!(PreparedDelegation::prepare(
        &parent,
        spec(
            "worker",
            DelegationBudget::new(1, 0)
                .unwrap()
                .with_timeout(Duration::from_secs(5))
                .unwrap()
        )
    )
    .is_ok());
}

#[test]
fn budget_timeout_is_an_optional_validated_wire_field() {
    let unbounded = DelegationBudget::new(2, 1).unwrap();
    assert_eq!(
        serde_json::to_value(unbounded).unwrap(),
        json!({ "max_turns": 2, "max_handoffs": 1 })
    );
    let bounded = unbounded.with_timeout(Duration::from_millis(1500)).unwrap();
    let encoded = serde_json::to_value(bounded).unwrap();
    assert_eq!(
        encoded,
        json!({ "max_turns": 2, "max_handoffs": 1, "timeout_ms": 1500 })
    );
    assert_eq!(
        serde_json::from_value::<DelegationBudget>(encoded).unwrap(),
        bounded
    );
    assert!(serde_json::from_value::<DelegationBudget>(
        json!({ "max_turns": 2, "max_handoffs": 1, "timeout_ms": 0 })
    )
    .is_err());
    assert!(unbounded.with_timeout(Duration::from_micros(10)).is_err());
}

#[tokio::test]
async fn child_limit_refuses_a_concurrent_sibling_until_the_slot_is_released() {
    let entered = Arc::new(Gate::default());
    let release = Arc::new(Gate::default());
    let events = Arc::new(Events::default());
    let runner = Runner::new([
        Agent::new(
            "worker",
            "Worker",
            "work",
            ScriptedModel::gated(
                [
                    ModelResponse::final_output("first"),
                    ModelResponse::final_output("third"),
                ],
                entered.clone(),
                release.clone(),
            ),
        ),
        Agent::new(
            "helper",
            "Helper",
            "help",
            ScriptedModel::new([ModelResponse::final_output("other parent")]),
        ),
    ])
    .unwrap()
    .with_child_limit(1)
    .with_observer(events.clone())
    .with_invocation_observer(events.clone());

    let parent = root();
    let other_parent = root();
    let budget = DelegationBudget::new(2, 0).unwrap();
    let first = prepare(&parent, budget);
    let sibling = prepare(&parent, budget);
    let sibling_invocation = sibling.invocation().clone();
    let elsewhere = PreparedDelegation::prepare(
        &grant(other_parent.clone(), DelegationBudget::new(8, 2).unwrap()),
        spec("helper", budget),
    )
    .unwrap();

    let (first_outcome, (sibling_outcome, elsewhere_outcome)) =
        tokio::join!(runner.run_delegation(first, &NoContext, &()), async {
            entered.wait().await;
            let sibling_outcome = runner.run_delegation(sibling, &NoContext, &()).await;
            let elsewhere_outcome = runner.run_delegation(elsewhere, &NoContext, &()).await;
            release.open();
            (sibling_outcome, elsewhere_outcome)
        });

    assert_eq!(first_outcome.unwrap().run.final_output, "first");
    assert_eq!(
        elsewhere_outcome.unwrap().run.final_output,
        "other parent",
        "the limit is per parent"
    );
    let refused = sibling_outcome.unwrap_err();
    assert!(matches!(
        refused.failure.error,
        RunError::ChildLimitExceeded { parent: p, limit: 1 } if p == parent.id
    ));
    assert_eq!(refused.disposition(), DelegationDisposition::Failed);
    assert_eq!(refused.failure.turns, 0);
    let sibling_events: Vec<InvocationEventKind> = events
        .invocations
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.invocation == sibling_invocation)
        .inspect(|event| {
            assert_eq!(
                serde_json::to_value(event).unwrap()["delegation"],
                serde_json::to_value(&refused.digests).unwrap()
            );
        })
        .map(|event| event.event.clone())
        .collect();
    assert_eq!(
        sibling_events,
        vec![
            InvocationEventKind::Started {},
            InvocationEventKind::Failed {
                failed_at: None,
                kind: InvocationFailureKind::Limit,
            },
        ]
    );

    // The slot was released when the first child finished.
    let third = runner
        .run_delegation(prepare(&parent, budget), &NoContext, &())
        .await
        .unwrap();
    assert_eq!(third.run.final_output, "third");
}

#[tokio::test]
async fn dropping_a_running_child_releases_its_slot() {
    let entered = Arc::new(Gate::default());
    let never = Arc::new(Gate::default());
    let after = ScriptedModel::new([ModelResponse::final_output("after drop")]);
    let runner = Runner::new([
        Agent::new(
            "worker",
            "Worker",
            "work",
            ScriptedModel::gated([], entered.clone(), never),
        ),
        Agent::new("helper", "Helper", "help", after),
    ])
    .unwrap()
    .with_child_limit(1);
    let parent = root();
    let budget = DelegationBudget::new(2, 0).unwrap();

    tokio::select! {
        _ = runner.run_delegation(prepare(&parent, budget), &NoContext, &()) => {
            panic!("the gated child cannot finish");
        }
        () = entered.wait() => {}
    }

    let next = PreparedDelegation::prepare(
        &grant(parent.clone(), DelegationBudget::new(8, 2).unwrap()),
        spec("helper", budget),
    )
    .unwrap();
    let result = runner.run_delegation(next, &NoContext, &()).await.unwrap();
    assert_eq!(result.run.final_output, "after drop");
}

/// Delegates to a child runner and reports its failure, optionally wrapped.
struct DelegateTool {
    child: Runner<()>,
    prepared: Mutex<Option<PreparedDelegation>>,
    wrap: bool,
    dispositions: Mutex<Vec<DelegationDisposition>>,
}

#[derive(Debug)]
struct Wrapped(Box<dyn std::error::Error + Send + Sync>);

impl std::fmt::Display for Wrapped {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "delegation failed: {}", self.0)
    }
}

impl std::error::Error for Wrapped {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[async_trait]
impl Tool<()> for DelegateTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("delegate", "runs a child", json!({ "type": "object" }))
    }

    async fn execute(&self, _arguments: Value, _context: &()) -> Result<Value, BoxError> {
        unreachable!("the runner calls execute_with_signal")
    }

    async fn execute_with_signal(
        &self,
        _arguments: Value,
        _context: &(),
        signal: &RunSignal,
    ) -> Result<Value, BoxError> {
        let prepared = self
            .prepared
            .lock()
            .unwrap()
            .take()
            .expect("one delegation");
        match self
            .child
            .run_delegation_controlled(prepared, &NoContext, &(), signal.child_controls())
            .await
        {
            Ok(result) => Ok(json!(result.run.final_output)),
            Err(failure) => {
                self.dispositions
                    .lock()
                    .unwrap()
                    .push(failure.disposition());
                if self.wrap {
                    Err(Box::new(Wrapped(Box::new(failure))))
                } else {
                    Err(Box::new(failure))
                }
            }
        }
    }
}

async fn parent_over_unknown_child(wrap: bool) {
    // Child: its tool runs past the child's own deadline and then fails, so
    // the child's termination is unknown. The parent itself never stops.
    let clock = Arc::new(ManualClock::new());
    let child_tool = Arc::new(ClockTool {
        clock: clock.clone(),
        advance: Duration::from_secs(5),
        fail: true,
        executions: AtomicUsize::new(0),
    });
    let child = Runner::new([Agent::new(
        "worker",
        "Worker",
        "work",
        ScriptedModel::new([ModelResponse::actions(vec![work_call("c1")])]),
    )
    .with_tool(child_tool.clone())])
    .unwrap()
    .with_clock(clock.clone());

    let parent_invocation = root();
    let delegate = Arc::new(DelegateTool {
        child,
        prepared: Mutex::new(Some(prepare(
            &parent_invocation,
            DelegationBudget::new(4, 0)
                .unwrap()
                .with_timeout(Duration::from_secs(1))
                .unwrap(),
        ))),
        wrap,
        dispositions: Mutex::new(Vec::new()),
    });
    let parent_model = ScriptedModel::new([
        ModelResponse::actions(vec![ModelAction::ToolCall(ToolCall::new(
            "d1",
            "delegate",
            json!({}),
        ))]),
        ModelResponse::final_output("never"),
    ]);
    let events = Arc::new(Events::default());
    let parent = Runner::new([Agent::new("planner", "Planner", "plan", parent_model)
        .with_revision("rev-1".parse().unwrap())
        .with_tool(delegate.clone())])
    .unwrap()
    .with_clock(clock)
    .with_observer(events.clone())
    .with_invocation_observer(events.clone());

    let failure = parent
        .run_invocation_controlled(
            InvocationRequest::new(
                parent_invocation,
                InvocationSource::Caller,
                AgentRef::with_revision("planner", "rev-1").unwrap(),
            ),
            "plan it",
            &(),
            RunOptions::default(),
            RunControls::new().with_cancellation(CancellationToken::new()),
        )
        .await
        .unwrap_err();

    assert_eq!(
        delegate.dispositions.lock().unwrap().as_slice(),
        [DelegationDisposition::RecoveryRequired]
    );
    match &failure.error {
        RunError::RecoveryRequired { agent, call_id, .. } => {
            assert_eq!(agent.as_str(), "planner");
            assert_eq!(call_id, "d1");
        }
        other => panic!("expected recovery required, got {other:?}"),
    }
    assert!(failure.error.leaves_unknown_effect());
    // Nothing is retried, and the parent saw no stop of its own.
    assert_eq!(child_tool.executions.load(Ordering::SeqCst), 1);
    assert!(!events
        .runs
        .lock()
        .unwrap()
        .iter()
        .any(|event| matches!(event, RunEvent::CancellationRequested { .. })));
    assert!(matches!(
        events.runs.lock().unwrap().last(),
        Some(RunEvent::RunFailed {
            kind: RunFailureKind::RecoveryRequired,
            ..
        })
    ));
}

#[tokio::test]
async fn unknown_child_termination_makes_the_parent_require_recovery() {
    parent_over_unknown_child(false).await;
}

#[tokio::test]
async fn recovery_required_survives_wrapping_that_reports_its_source() {
    parent_over_unknown_child(true).await;
}

#[tokio::test]
async fn cancelled_child_carries_digests_on_every_event() {
    let events = Arc::new(Events::default());
    let runner = Runner::new([Agent::new(
        "worker",
        "Worker",
        "work",
        ScriptedModel::new([ModelResponse::final_output("must not run")]),
    )])
    .unwrap()
    .with_invocation_observer(events.clone());
    let prepared = prepare(&root(), DelegationBudget::new(2, 0).unwrap());
    let digests = prepared.digests().clone();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let failure = runner
        .run_delegation_controlled(
            prepared,
            &NoContext,
            &(),
            RunControls::new().with_cancellation(cancellation),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.disposition(), DelegationDisposition::Cancelled);
    let events = events.invocations.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(events.iter().any(|event| matches!(
        event.event,
        InvocationEventKind::CancellationRequested { .. }
    )));
    for event in events.iter() {
        assert_eq!(
            serde_json::to_value(event).unwrap()["delegation"],
            serde_json::to_value(&digests).unwrap()
        );
    }
}
