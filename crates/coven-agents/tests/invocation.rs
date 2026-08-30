use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use coven_agents::{
    Agent, AgentId, AgentRef, BoxError, Handoff, HandoffCall, InvocationEvent, InvocationId,
    InvocationObserver, Model, ModelAction, ModelRequest, ModelResponse, RunEvent, RunFailureKind,
    RunObserver, RunOptions, Runner,
};

struct QueueModel {
    responses: Mutex<VecDeque<ModelResponse>>,
}

impl QueueModel {
    fn new(responses: impl IntoIterator<Item = ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
        }
    }
}

#[async_trait]
impl Model<()> for QueueModel {
    async fn generate(
        &self,
        _request: ModelRequest,
        _context: &(),
    ) -> Result<ModelResponse, BoxError> {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| Box::new(io::Error::other("no queued response")) as BoxError)
    }
}

#[derive(Default)]
struct RecordingInvocationObserver {
    events: Mutex<Vec<InvocationEvent>>,
}

impl RecordingInvocationObserver {
    fn events(&self) -> Vec<InvocationEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl InvocationObserver for RecordingInvocationObserver {
    fn on_invocation_event(&self, event: &InvocationEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

#[derive(Default)]
struct RecordingRunObserver {
    events: Mutex<Vec<RunEvent>>,
}

impl RecordingRunObserver {
    fn events(&self) -> Vec<RunEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl RunObserver for RecordingRunObserver {
    fn on_event(&self, event: &RunEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

fn event_invocation(event: &InvocationEvent) -> &InvocationId {
    match event {
        InvocationEvent::InvocationStarted { invocation, .. }
        | InvocationEvent::ControlTransferred { invocation, .. }
        | InvocationEvent::InvocationCompleted { invocation, .. }
        | InvocationEvent::InvocationFailed { invocation, .. } => invocation,
    }
}

/// Asserts the invocation identity contract: the canonical stream opens with
/// exactly one `InvocationStarted`, closes with exactly one terminal event,
/// and every event in between carries the same invocation identity.
fn assert_single_invocation_identity(events: &[InvocationEvent]) -> InvocationId {
    assert!(
        !events.is_empty(),
        "an invocation must emit at least one event"
    );
    assert!(
        matches!(
            events.first(),
            Some(InvocationEvent::InvocationStarted { .. })
        ),
        "InvocationStarted must be the first event, got {events:?}"
    );
    let terminal = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                InvocationEvent::InvocationCompleted { .. }
                    | InvocationEvent::InvocationFailed { .. }
            )
        })
        .count();
    assert_eq!(terminal, 1, "expected one terminal event, got {events:?}");

    let identity = event_invocation(events.first().expect("non-empty stream")).clone();
    for event in events {
        assert_eq!(
            event_invocation(event),
            &identity,
            "every event must carry the same invocation identity, got {events:?}"
        );
    }
    identity
}

#[tokio::test]
async fn one_identity_carries_from_started_to_terminal_event() {
    let observer = Arc::new(RecordingInvocationObserver::default());
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("done")]));
    let runner = Runner::new([Agent::new("a", "A", "Answer.", model)])
        .unwrap()
        .with_invocation_observer(observer.clone());

    let result = runner
        .run("a", "Hello", &(), RunOptions::default())
        .await
        .unwrap();
    let events = observer.events();
    let identity = assert_single_invocation_identity(&events);

    assert_eq!(result.invocation, identity);
    match &events[0] {
        InvocationEvent::InvocationStarted { parent, target, .. } => {
            assert_eq!(*parent, None);
            assert_eq!(target.agent(), &AgentId::from("a"));
            assert_eq!(target.revision(), None);
        }
        other => panic!("the first event must be InvocationStarted, got {other:?}"),
    }
    match events.last().expect("non-empty stream") {
        InvocationEvent::InvocationCompleted {
            final_target,
            turns,
            control_transfers,
            ..
        } => {
            assert_eq!(final_target.agent(), &AgentId::from("a"));
            assert_eq!(*turns, 1);
            assert_eq!(*control_transfers, 0);
        }
        other => panic!("the last event must be InvocationCompleted, got {other:?}"),
    }
}

#[tokio::test]
async fn caller_supplied_identity_is_used_verbatim() {
    let observer = Arc::new(RecordingInvocationObserver::default());
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("done")]));
    let runner = Runner::new([Agent::new("a", "A", "Answer.", model)])
        .unwrap()
        .with_invocation_observer(observer.clone());

    let options = RunOptions {
        invocation_id: Some(InvocationId::try_new("inv-fixed").unwrap()),
        ..RunOptions::default()
    };
    let result = runner.run("a", "Hello", &(), options).await.unwrap();
    let identity = assert_single_invocation_identity(&observer.events());

    assert_eq!(identity.as_str(), "inv-fixed");
    assert_eq!(result.invocation, identity);
}

#[tokio::test]
async fn generated_identities_differ_across_runs() {
    let model = Arc::new(QueueModel::new([
        ModelResponse::final_output("first"),
        ModelResponse::final_output("second"),
    ]));
    let runner = Runner::new([Agent::new("a", "A", "Answer.", model)]).unwrap();

    let first = runner
        .run("a", "One", &(), RunOptions::default())
        .await
        .unwrap();
    let second = runner
        .run("a", "Two", &(), RunOptions::default())
        .await
        .unwrap();

    assert_ne!(first.invocation, second.invocation);
}

#[tokio::test]
async fn parent_identity_correlates_nested_work() {
    let observer = Arc::new(RecordingInvocationObserver::default());
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("done")]));
    let runner = Runner::new([Agent::new("child", "Child", "Answer.", model)])
        .unwrap()
        .with_invocation_observer(observer.clone());

    let parent = InvocationId::try_new("inv-parent").unwrap();
    let options = RunOptions {
        parent_invocation: Some(parent.clone()),
        ..RunOptions::default()
    };
    let result = runner.run("child", "Hello", &(), options).await.unwrap();

    let events = observer.events();
    assert_single_invocation_identity(&events);
    match &events[0] {
        InvocationEvent::InvocationStarted {
            invocation,
            parent: observed,
            ..
        } => {
            assert_eq!(*observed, Some(parent.clone()));
            assert_ne!(*invocation, parent);
        }
        other => panic!("the first event must be InvocationStarted, got {other:?}"),
    }
    assert_ne!(result.invocation, parent);
}

#[tokio::test]
async fn legacy_handoff_is_reported_as_a_control_transfer() {
    let observer = Arc::new(RecordingInvocationObserver::default());
    let model = Arc::new(QueueModel::new([
        ModelResponse::actions(vec![ModelAction::Handoff(HandoffCall::new("to_b"))]),
        ModelResponse::final_output("done from b"),
    ]));
    let agents = [
        Agent::new("a", "A", "Delegate.", model.clone()).with_handoff(Handoff::new(
            "to_b",
            "Hand off to b",
            "b",
        )),
        Agent::new("b", "B", "Answer.", model),
    ];
    let runner = Runner::new(agents)
        .unwrap()
        .with_invocation_observer(observer.clone());

    let result = runner
        .run("a", "Hello", &(), RunOptions::default())
        .await
        .unwrap();
    let events = observer.events();
    let identity = assert_single_invocation_identity(&events);

    assert_eq!(result.final_agent, AgentId::from("b"));
    assert_eq!(result.invocation, identity);
    let transfers: Vec<&InvocationEvent> = events
        .iter()
        .filter(|event| matches!(event, InvocationEvent::ControlTransferred { .. }))
        .collect();
    assert_eq!(
        transfers.len(),
        1,
        "expected one control transfer, got {events:?}"
    );
    match transfers[0] {
        InvocationEvent::ControlTransferred { from, to, .. } => {
            assert_eq!(from.agent(), &AgentId::from("a"));
            assert_eq!(to.agent(), &AgentId::from("b"));
        }
        other => panic!("expected a control transfer, got {other:?}"),
    }
    match events.last().expect("non-empty stream") {
        InvocationEvent::InvocationCompleted {
            final_target,
            control_transfers,
            ..
        } => {
            assert_eq!(final_target.agent(), &AgentId::from("b"));
            assert_eq!(*control_transfers, 1);
        }
        other => panic!("the last event must be InvocationCompleted, got {other:?}"),
    }
}

#[tokio::test]
async fn failed_run_carries_the_identity_to_the_terminal_event() {
    let observer = Arc::new(RecordingInvocationObserver::default());
    let model = Arc::new(QueueModel::new([]));
    let runner = Runner::new([Agent::new("a", "A", "Answer.", model)])
        .unwrap()
        .with_invocation_observer(observer.clone());

    let options = RunOptions {
        invocation_id: Some(InvocationId::try_new("inv-doomed").unwrap()),
        ..RunOptions::default()
    };
    let failure = runner.run("a", "Hello", &(), options).await.unwrap_err();
    let events = observer.events();
    let identity = assert_single_invocation_identity(&events);

    assert_eq!(failure.invocation, identity);
    assert_eq!(identity.as_str(), "inv-doomed");
    match events.last().expect("non-empty stream") {
        InvocationEvent::InvocationFailed {
            target,
            kind: RunFailureKind::Model,
            ..
        } => {
            assert_eq!(target.agent(), &AgentId::from("a"));
        }
        other => panic!("the last event must be InvocationFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn both_observer_streams_stay_paired_for_one_run() {
    let invocation_observer = Arc::new(RecordingInvocationObserver::default());
    let run_observer = Arc::new(RecordingRunObserver::default());
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("done")]));
    let runner = Runner::new([Agent::new("a", "A", "Answer.", model)])
        .unwrap()
        .with_observer(run_observer.clone())
        .with_invocation_observer(invocation_observer.clone());

    let result = runner
        .run("a", "Hello", &(), RunOptions::default())
        .await
        .unwrap();

    let invocation_events = invocation_observer.events();
    let run_events = run_observer.events();
    let identity = assert_single_invocation_identity(&invocation_events);
    assert_eq!(result.invocation, identity);
    assert!(
        matches!(run_events.first(), Some(RunEvent::RunStarted { .. })),
        "the legacy stream must still open with RunStarted, got {run_events:?}"
    );
    assert_eq!(
        run_events
            .iter()
            .filter(|event| matches!(event, RunEvent::RunStarted { .. }))
            .count(),
        1
    );
    assert!(
        matches!(run_events.last(), Some(RunEvent::RunCompleted { .. })),
        "the legacy stream must still close with RunCompleted, got {run_events:?}"
    );
}

#[test]
fn invocation_contract_types_roundtrip_through_serde() {
    let id = InvocationId::try_new("inv-42").unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), "\"inv-42\"");
    assert_eq!(
        serde_json::from_str::<InvocationId>("\"inv-42\"").unwrap(),
        id
    );

    let reference = AgentRef::try_new(AgentId::from("triage"))
        .unwrap()
        .with_revision("v7")
        .unwrap();
    let json = serde_json::to_string(&reference).unwrap();
    assert_eq!(json, r#"{"agent":"triage","revision":"v7"}"#);
    assert_eq!(serde_json::from_str::<AgentRef>(&json).unwrap(), reference);

    let bare = AgentRef::try_new(AgentId::from("triage")).unwrap();
    let bare_json = serde_json::to_string(&bare).unwrap();
    assert_eq!(bare_json, r#"{"agent":"triage","revision":null}"#);
    assert_eq!(serde_json::from_str::<AgentRef>(&bare_json).unwrap(), bare);
}
