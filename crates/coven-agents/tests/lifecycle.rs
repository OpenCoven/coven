//! Bounded lifecycle: deadlines on an injected clock, cooperative
//! cancellation, and the distinction between a clean stop and an effect whose
//! termination is unknown. Interleavings are made deterministic by acting on
//! the token or clock from inside scripted models and tools; nothing sleeps.

use std::{
    collections::VecDeque,
    io,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use coven_agents::{
    Agent, AgentRef, BoxError, CancellationToken, InvocationContext, InvocationEvent,
    InvocationEventKind, InvocationFailureKind, InvocationId, InvocationObserver,
    InvocationRequest, InvocationSource, ManualClock, Model, ModelAction, ModelRequest,
    ModelResponse, NotStarted, RunControls, RunError, RunEvent, RunFailure, RunFailureKind,
    RunItem, RunObserver, RunOptions, RunSignal, Runner, StopReason, Tool, ToolCall,
    ToolDefinition,
};
use serde_json::{json, Value};

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

impl Events {
    fn runs(&self) -> Vec<RunEvent> {
        self.runs.lock().unwrap().clone()
    }

    fn invocation_kinds(&self) -> Vec<InvocationEventKind> {
        self.invocations
            .lock()
            .unwrap()
            .iter()
            .map(|event| event.event.clone())
            .collect()
    }
}

type Hook = Box<dyn Fn() + Send + Sync>;

/// Replays scripted responses and runs `hook` while "generating", so a test
/// can cancel or move the clock while the model call is in flight.
struct ScriptedModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    calls: AtomicUsize,
    hook: Option<Hook>,
}

impl ScriptedModel {
    fn new(responses: impl IntoIterator<Item = ModelResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            calls: AtomicUsize::new(0),
            hook: None,
        })
    }

    fn with_hook(responses: impl IntoIterator<Item = ModelResponse>, hook: Hook) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            calls: AtomicUsize::new(0),
            hook: Some(hook),
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
        if let Some(hook) = &self.hook {
            hook();
        }
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| Box::new(io::Error::other("no scripted response")) as BoxError)
    }
}

enum Outcome {
    Ok,
    Fail,
    /// Fail with `NotStarted` if a stop is visible, without producing an
    /// effect.
    ProveNotStartedOnStop,
}

/// A tool whose `hook` runs mid-execution, standing in for a caller that
/// cancels or for time passing while the tool works.
struct ScriptedTool {
    name: &'static str,
    hook: Option<Hook>,
    outcome: Outcome,
    effects: Arc<AtomicUsize>,
    executions: AtomicUsize,
    observed: Mutex<Vec<(bool, Option<Duration>)>>,
}

impl ScriptedTool {
    fn new(name: &'static str, outcome: Outcome, hook: Option<Hook>) -> Arc<Self> {
        Arc::new(Self {
            name,
            hook,
            outcome,
            effects: Arc::new(AtomicUsize::new(0)),
            executions: AtomicUsize::new(0),
            observed: Mutex::new(Vec::new()),
        })
    }

    fn effects(&self) -> usize {
        self.effects.load(Ordering::SeqCst)
    }

    fn executions(&self) -> usize {
        self.executions.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Tool<()> for ScriptedTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name, "scripted", json!({ "type": "object" }))
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
        self.executions.fetch_add(1, Ordering::SeqCst);
        if let Some(hook) = &self.hook {
            hook();
        }
        self.observed
            .lock()
            .unwrap()
            .push((signal.is_stop_requested(), signal.remaining()));
        match self.outcome {
            Outcome::ProveNotStartedOnStop if signal.is_stop_requested() => {
                Err(Box::new(NotStarted::new("observed stop before writing")))
            }
            Outcome::ProveNotStartedOnStop | Outcome::Ok => {
                self.effects.fetch_add(1, Ordering::SeqCst);
                Ok(json!({ "ran": self.name }))
            }
            Outcome::Fail => {
                self.effects.fetch_add(1, Ordering::SeqCst);
                Err(Box::new(io::Error::other("connection reset")))
            }
        }
    }
}

fn worker() -> AgentRef {
    AgentRef::with_revision("worker", "rev-1").unwrap()
}

fn runner_with(
    model: Arc<ScriptedModel>,
    tools: Vec<Arc<ScriptedTool>>,
    clock: Arc<ManualClock>,
    events: Arc<Events>,
) -> Runner<()> {
    let mut agent =
        Agent::new("worker", "Worker", "work", model).with_revision("rev-1".parse().unwrap());
    for tool in tools {
        agent = agent.with_tool(tool);
    }
    Runner::new([agent])
        .unwrap()
        .with_clock(clock)
        .with_observer(events.clone())
        .with_invocation_observer(events)
}

fn request() -> InvocationRequest {
    InvocationRequest::new(
        InvocationContext::root(InvocationId::new()),
        InvocationSource::Caller,
        worker(),
    )
}

fn call(id: &str, tool: &str) -> ModelAction {
    ModelAction::ToolCall(ToolCall::new(id, tool, json!({})))
}

async fn run(
    runner: &Runner<()>,
    controls: RunControls,
) -> Result<coven_agents::RunResult, RunFailure> {
    runner
        .run_invocation_controlled(
            request(),
            "do the work",
            &(),
            RunOptions::default(),
            controls,
        )
        .await
}

/// The last two run events are the stop observation and its terminal pair.
fn assert_stop_events(events: &Events, reason: StopReason, kind: RunFailureKind) {
    let runs = events.runs();
    let stops = runs
        .iter()
        .filter(|event| matches!(event, RunEvent::CancellationRequested { .. }))
        .count();
    assert_eq!(stops, 1, "exactly one stop observation: {runs:?}");
    let [.., observed, terminal] = runs.as_slice() else {
        panic!("too few events: {runs:?}");
    };
    assert!(
        matches!(observed, RunEvent::CancellationRequested { reason: r, .. } if *r == reason),
        "{observed:?}"
    );
    assert!(
        matches!(terminal, RunEvent::RunFailed { kind: k, .. } if *k == kind),
        "{terminal:?}"
    );

    let invocation_kind = match kind {
        RunFailureKind::Cancelled => InvocationFailureKind::Cancelled,
        RunFailureKind::TerminationUnknown => InvocationFailureKind::TerminationUnknown,
        other => panic!("not a stop kind: {other:?}"),
    };
    assert_eq!(
        events.invocation_kinds(),
        vec![
            InvocationEventKind::Started {},
            InvocationEventKind::CancellationRequested { reason },
            InvocationEventKind::Failed {
                failed_at: Some(worker()),
                kind: invocation_kind,
            },
        ]
    );
}

#[tokio::test]
async fn cancellation_before_start_stops_without_a_model_turn() {
    let model = ScriptedModel::new([ModelResponse::final_output("never")]);
    let events = Arc::new(Events::default());
    let runner = runner_with(
        model.clone(),
        vec![],
        Arc::new(ManualClock::new()),
        events.clone(),
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let failure = run(&runner, RunControls::new().with_cancellation(cancellation))
        .await
        .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::Cancelled {
            reason: StopReason::Cancelled,
            ..
        }
    ));
    assert_eq!(model.calls(), 0);
    assert_eq!(failure.turns, 0);
    assert_stop_events(&events, StopReason::Cancelled, RunFailureKind::Cancelled);
}

#[tokio::test]
async fn deadline_passing_during_a_successful_tool_is_acknowledged_at_the_next_checkpoint() {
    let clock = Arc::new(ManualClock::new());
    let tick = clock.clone();
    let tool = ScriptedTool::new(
        "slow",
        Outcome::Ok,
        Some(Box::new(move || tick.advance(Duration::from_secs(11)))),
    );
    let model = ScriptedModel::new([
        ModelResponse::actions(vec![call("c1", "slow")]),
        ModelResponse::final_output("too late"),
    ]);
    let events = Arc::new(Events::default());
    let runner = runner_with(model.clone(), vec![tool.clone()], clock, events.clone());

    let failure = run(
        &runner,
        RunControls::new().with_timeout(Duration::from_secs(10)),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::Cancelled {
            reason: StopReason::DeadlineExceeded,
            ..
        }
    ));
    // The completed effect is known and kept; the run still does not succeed.
    assert_eq!(tool.effects(), 1);
    assert_eq!(model.calls(), 1, "no second model turn after the deadline");
    assert!(failure
        .new_items
        .iter()
        .any(|item| matches!(item, RunItem::ToolResult { call_id, .. } if call_id == "c1")));
    assert_stop_events(
        &events,
        StopReason::DeadlineExceeded,
        RunFailureKind::Cancelled,
    );
}

#[tokio::test]
async fn stop_during_a_model_call_discards_its_actions() {
    let cancellation = CancellationToken::new();
    let cancel = cancellation.clone();
    let tool = ScriptedTool::new("write", Outcome::Ok, None);
    let model = ScriptedModel::with_hook(
        [ModelResponse {
            assistant_message: Some("writing now".to_owned()),
            actions: vec![call("c1", "write")],
        }],
        Box::new(move || cancel.cancel()),
    );
    let events = Arc::new(Events::default());
    let runner = runner_with(
        model,
        vec![tool.clone()],
        Arc::new(ManualClock::new()),
        events.clone(),
    );

    let failure = run(&runner, RunControls::new().with_cancellation(cancellation))
        .await
        .unwrap_err();

    assert!(matches!(failure.error, RunError::Cancelled { .. }));
    assert_eq!(tool.executions(), 0);
    assert_eq!(
        failure.new_items.as_ref(),
        [RunItem::UserMessage {
            content: "do the work".to_owned()
        }],
        "a response that arrives after the stop is not recorded"
    );
    assert_stop_events(&events, StopReason::Cancelled, RunFailureKind::Cancelled);
}

#[tokio::test]
async fn tool_failing_after_a_stop_without_proof_is_termination_unknown() {
    let cancellation = CancellationToken::new();
    let cancel = cancellation.clone();
    let tool = ScriptedTool::new(
        "charge",
        Outcome::Fail,
        Some(Box::new(move || cancel.cancel())),
    );
    let model = ScriptedModel::new([
        ModelResponse::actions(vec![call("c1", "charge")]),
        ModelResponse::final_output("never"),
    ]);
    let events = Arc::new(Events::default());
    let runner = runner_with(
        model.clone(),
        vec![tool.clone()],
        Arc::new(ManualClock::new()),
        events.clone(),
    );

    let failure = run(&runner, RunControls::new().with_cancellation(cancellation))
        .await
        .unwrap_err();

    match &failure.error {
        RunError::TerminationUnknown {
            agent,
            call_id,
            reason,
            ..
        } => {
            assert_eq!(agent.as_str(), "worker");
            assert_eq!(call_id, "c1");
            assert_eq!(*reason, StopReason::Cancelled);
        }
        other => panic!("expected termination unknown, got {other:?}"),
    }
    // Never retried, never reported as success or as a clean stop.
    assert_eq!(tool.executions(), 1);
    assert_eq!(model.calls(), 1);
    assert_stop_events(
        &events,
        StopReason::Cancelled,
        RunFailureKind::TerminationUnknown,
    );
}

#[tokio::test]
async fn deadline_expiring_inside_a_failing_tool_is_termination_unknown() {
    let clock = Arc::new(ManualClock::new());
    let tick = clock.clone();
    let tool = ScriptedTool::new(
        "deploy",
        Outcome::Fail,
        Some(Box::new(move || tick.advance(Duration::from_secs(30)))),
    );
    let model = ScriptedModel::new([ModelResponse::actions(vec![call("c1", "deploy")])]);
    let events = Arc::new(Events::default());
    let runner = runner_with(model, vec![tool.clone()], clock, events.clone());

    let failure = run(
        &runner,
        RunControls::new().with_timeout(Duration::from_secs(5)),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::TerminationUnknown {
            reason: StopReason::DeadlineExceeded,
            ..
        }
    ));
    assert_eq!(tool.executions(), 1);
    assert_stop_events(
        &events,
        StopReason::DeadlineExceeded,
        RunFailureKind::TerminationUnknown,
    );
}

#[tokio::test]
async fn tool_proving_not_started_after_a_stop_is_a_clean_cancellation() {
    let cancellation = CancellationToken::new();
    let cancel = cancellation.clone();
    let tool = ScriptedTool::new(
        "write",
        Outcome::ProveNotStartedOnStop,
        Some(Box::new(move || cancel.cancel())),
    );
    let model = ScriptedModel::new([ModelResponse::actions(vec![call("c1", "write")])]);
    let events = Arc::new(Events::default());
    let runner = runner_with(
        model,
        vec![tool.clone()],
        Arc::new(ManualClock::new()),
        events.clone(),
    );

    let failure = run(&runner, RunControls::new().with_cancellation(cancellation))
        .await
        .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::Cancelled {
            reason: StopReason::Cancelled,
            ..
        }
    ));
    assert_eq!(tool.effects(), 0);
    assert_eq!(tool.observed.lock().unwrap().as_slice(), [(true, None)]);
    assert_stop_events(&events, StopReason::Cancelled, RunFailureKind::Cancelled);
}

#[tokio::test]
async fn not_started_without_a_stop_is_an_ordinary_tool_failure() {
    struct Refuses;

    #[async_trait]
    impl Tool<()> for Refuses {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("refuse", "refuses", json!({ "type": "object" }))
        }

        async fn execute(&self, _arguments: Value, _context: &()) -> Result<Value, BoxError> {
            Err(Box::new(NotStarted::new("not now")))
        }
    }

    let model = ScriptedModel::new([ModelResponse::actions(vec![call("c1", "refuse")])]);
    let events = Arc::new(Events::default());
    let agent = Agent::new("worker", "Worker", "work", model)
        .with_revision("rev-1".parse().unwrap())
        .with_tool(Arc::new(Refuses));
    let runner = Runner::new([agent])
        .unwrap()
        .with_observer(events.clone())
        .with_invocation_observer(events.clone());

    let failure = run(
        &runner,
        RunControls::new().with_cancellation(CancellationToken::new()),
    )
    .await
    .unwrap_err();

    assert!(matches!(failure.error, RunError::ToolFailed { .. }));
    assert!(!events
        .runs()
        .iter()
        .any(|event| matches!(event, RunEvent::CancellationRequested { .. })));
}

#[tokio::test]
async fn stop_after_a_successful_call_prevents_the_rest_of_the_batch() {
    let cancellation = CancellationToken::new();
    let cancel = cancellation.clone();
    let first = ScriptedTool::new(
        "first",
        Outcome::Ok,
        Some(Box::new(move || cancel.cancel())),
    );
    let second = ScriptedTool::new("second", Outcome::Ok, None);
    let model = ScriptedModel::new([ModelResponse::actions(vec![
        call("c1", "first"),
        call("c2", "second"),
    ])]);
    let events = Arc::new(Events::default());
    let runner = runner_with(
        model,
        vec![first.clone(), second.clone()],
        Arc::new(ManualClock::new()),
        events.clone(),
    );

    let failure = run(&runner, RunControls::new().with_cancellation(cancellation))
        .await
        .unwrap_err();

    assert!(matches!(failure.error, RunError::Cancelled { .. }));
    assert_eq!(first.effects(), 1);
    assert_eq!(second.executions(), 0);
    let recorded_calls: Vec<&str> = failure
        .new_items
        .iter()
        .filter_map(|item| match item {
            RunItem::ToolCall { call, .. } => Some(call.id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        recorded_calls,
        ["c1"],
        "the undispatched call is not recorded"
    );
    assert_stop_events(&events, StopReason::Cancelled, RunFailureKind::Cancelled);
}

#[tokio::test]
async fn tools_observe_the_remaining_deadline_on_the_injected_clock() {
    let clock = Arc::new(ManualClock::new());
    let tick = clock.clone();
    let tool = ScriptedTool::new("read", Outcome::Ok, None);
    let model = ScriptedModel::with_hook(
        [
            ModelResponse::actions(vec![call("c1", "read")]),
            ModelResponse::final_output("done"),
        ],
        Box::new(move || tick.advance(Duration::from_secs(3))),
    );
    let events = Arc::new(Events::default());
    let runner = runner_with(model, vec![tool.clone()], clock, events);

    let result = run(
        &runner,
        RunControls::new().with_timeout(Duration::from_secs(10)),
    )
    .await
    .unwrap();

    assert_eq!(result.final_output, "done");
    assert_eq!(
        tool.observed.lock().unwrap().as_slice(),
        [(false, Some(Duration::from_secs(7)))]
    );
}

#[tokio::test]
async fn legacy_entry_points_never_observe_a_stop() {
    let tool = ScriptedTool::new("read", Outcome::Ok, None);
    let model = ScriptedModel::new([
        ModelResponse::actions(vec![call("c1", "read")]),
        ModelResponse::final_output("done"),
    ]);
    let events = Arc::new(Events::default());
    let runner = runner_with(
        model,
        vec![tool.clone()],
        Arc::new(ManualClock::new()),
        events,
    );

    let result = runner
        .run("worker", "do the work", &(), RunOptions::default())
        .await
        .unwrap();

    assert_eq!(result.final_output, "done");
    assert_eq!(tool.observed.lock().unwrap().as_slice(), [(false, None)]);
}

#[test]
fn stop_vocabulary_has_a_closed_snake_case_wire_shape() {
    let event = InvocationEvent::new(
        InvocationContext::root(
            "11111111-1111-4111-8111-111111111111"
                .parse::<InvocationId>()
                .unwrap(),
        ),
        InvocationSource::Caller,
        worker(),
        InvocationEventKind::CancellationRequested {
            reason: StopReason::DeadlineExceeded,
        },
    );
    let encoded = serde_json::to_value(&event).unwrap();
    assert_eq!(
        encoded["event"],
        json!({ "type": "cancellation_requested", "reason": "deadline_exceeded" })
    );
    assert_eq!(
        serde_json::from_value::<InvocationEvent>(encoded).unwrap(),
        event
    );
    assert_eq!(
        serde_json::to_value(InvocationFailureKind::TerminationUnknown).unwrap(),
        json!("termination_unknown")
    );
    assert_eq!(
        serde_json::to_value(InvocationFailureKind::Cancelled).unwrap(),
        json!("cancelled")
    );
    assert_eq!(
        serde_json::to_value(InvocationFailureKind::RecoveryRequired).unwrap(),
        json!("recovery_required")
    );
    assert!(serde_json::from_value::<StopReason>(json!("timeout")).is_err());
}
