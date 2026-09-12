use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use coven_agents::{
    Agent, AgentRef, AgentRevision, BoxError, Handoff, HandoffCall, InvocationContext,
    InvocationEvent, InvocationEventKind, InvocationEventVersion, InvocationFailureKind,
    InvocationId, InvocationObserver, InvocationRequest, InvocationSource, Model, ModelAction,
    ModelRequest, ModelResponse, RunError, RunOptions, Runner,
};
use serde_json::json;

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
    fn on_event(&self, event: &InvocationEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

#[test]
fn agent_ref_round_trips_with_an_optional_revision() {
    let agent = AgentRef::with_revision("triage", "sha256:abc123").unwrap();

    assert_eq!(agent.id().as_str(), "triage");
    assert_eq!(
        agent.revision().map(AgentRevision::as_str),
        Some("sha256:abc123")
    );
    assert_eq!(
        serde_json::to_value(&agent).unwrap(),
        json!({
            "id": "triage",
            "revision": "sha256:abc123"
        })
    );
    assert_eq!(
        serde_json::from_value::<AgentRef>(serde_json::to_value(&agent).unwrap()).unwrap(),
        agent
    );

    let unrevisioned = AgentRef::new("triage").unwrap();
    assert_eq!(
        serde_json::to_value(&unrevisioned).unwrap(),
        json!({ "id": "triage" })
    );
}

#[test]
fn agent_ref_rejects_ambiguous_or_open_ended_wire_values() {
    for invalid_id in ["", " triage", "triage ", "tri age", "triage\nworker"] {
        assert!(
            AgentRef::new(invalid_id).is_err(),
            "accepted invalid agent id {invalid_id:?}"
        );
    }
    assert!(AgentRef::new("a".repeat(128)).is_err());

    for invalid_revision in ["", " latest", "latest ", "latest stable", "v1\nv2"] {
        assert!(
            AgentRef::with_revision("triage", invalid_revision).is_err(),
            "accepted invalid agent revision {invalid_revision:?}"
        );
    }
    assert!(AgentRef::with_revision("triage", "r".repeat(128)).is_err());

    assert!(serde_json::from_value::<AgentRef>(json!({
        "id": "triage",
        "unexpected": true
    }))
    .is_err());
    assert!(serde_json::from_value::<AgentRef>(json!({
        "id": "tri age"
    }))
    .is_err());
    assert!(serde_json::from_value::<AgentRef>(json!({
        "id": "triage",
        "revision": ""
    }))
    .is_err());
}

#[test]
fn canonical_invocation_event_has_a_closed_versioned_wire_shape() {
    let invocation = InvocationContext::root(
        "11111111-1111-4111-8111-111111111111"
            .parse::<InvocationId>()
            .unwrap(),
    );
    let source = AgentRef::with_revision("planner", "rev-planner").unwrap();
    let target = AgentRef::with_revision("worker", "rev-worker").unwrap();
    let event = InvocationEvent::new(
        invocation,
        InvocationSource::Agent { agent: source },
        target,
        InvocationEventKind::Started {},
    );

    let encoded = serde_json::to_value(&event).unwrap();
    assert_eq!(
        encoded,
        json!({
            "contract": "coven.agent-invocation-event.v1",
            "invocation": {
                "id": "11111111-1111-4111-8111-111111111111"
            },
            "source": {
                "type": "agent",
                "agent": {
                    "id": "planner",
                    "revision": "rev-planner"
                }
            },
            "requested_target": {
                "id": "worker",
                "revision": "rev-worker"
            },
            "event": {
                "type": "started"
            }
        })
    );
    assert_eq!(
        serde_json::from_value::<InvocationEvent>(encoded).unwrap(),
        event
    );
    assert_eq!(event.contract(), InvocationEventVersion::V1);

    assert!(serde_json::from_value::<InvocationEvent>(json!({
        "contract": "coven.agent-invocation-event.v1",
        "invocation": {
            "id": "11111111-1111-4111-8111-111111111111"
        },
        "source": { "type": "caller" },
        "requested_target": { "id": "worker" },
        "event": { "type": "started", "unexpected": true }
    }))
    .is_err());
    assert!(serde_json::from_value::<InvocationEvent>(json!({
        "contract": "coven.agent-invocation-event.v1",
        "invocation": {
            "id": "11111111-1111-4111-8111-111111111111",
            "unexpected": true
        },
        "source": { "type": "caller" },
        "requested_target": { "id": "worker" },
        "event": { "type": "started" }
    }))
    .is_err());
}

#[tokio::test]
async fn runner_emits_revisioned_canonical_lifecycle_and_control_transfer_events() {
    let triage = Agent::new(
        "triage",
        "Triage",
        "Route.",
        Arc::new(QueueModel::new([ModelResponse::actions(vec![
            ModelAction::Handoff(HandoffCall::new("to-worker")),
        ])])),
    )
    .with_revision("rev-triage".parse().unwrap())
    .with_handoff(Handoff::new("to-worker", "Route to worker", "worker"));
    let worker = Agent::new(
        "worker",
        "Worker",
        "Answer.",
        Arc::new(QueueModel::new([ModelResponse::final_output("Done.")])),
    )
    .with_revision("rev-worker".parse().unwrap());
    let observer = Arc::new(RecordingInvocationObserver::default());
    let runner = Runner::new([triage, worker])
        .unwrap()
        .with_invocation_observer(observer.clone());
    let invocation = InvocationContext::child(
        "22222222-2222-4222-8222-222222222222".parse().unwrap(),
        "11111111-1111-4111-8111-111111111111".parse().unwrap(),
    );
    let source = AgentRef::with_revision("planner", "rev-planner").unwrap();
    let target = AgentRef::with_revision("triage", "rev-triage").unwrap();

    let result = runner
        .run_invocation(
            InvocationRequest::new(
                invocation.clone(),
                InvocationSource::Agent {
                    agent: source.clone(),
                },
                target.clone(),
            ),
            "Route this.",
            &(),
            RunOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(result.final_output, "Done.");
    let events = observer.events();
    assert_eq!(events.len(), 3, "expected start, transfer, terminal");
    assert!(events.iter().all(|event| {
        event.invocation == invocation
            && event.requested_target == target
            && event.source
                == InvocationSource::Agent {
                    agent: source.clone(),
                }
    }));
    assert!(matches!(&events[0].event, InvocationEventKind::Started {}));
    assert!(matches!(
        &events[1].event,
        InvocationEventKind::ControlTransferred { from, to, name }
            if from == &AgentRef::with_revision("triage", "rev-triage").unwrap()
                && to == &AgentRef::with_revision("worker", "rev-worker").unwrap()
                && name == "to-worker"
    ));
    assert!(matches!(
        &events[2].event,
        InvocationEventKind::Completed {
            final_agent,
            turns: 2,
            control_transfers: 1
        } if final_agent == &AgentRef::with_revision("worker", "rev-worker").unwrap()
    ));
    for event in events {
        let encoded = serde_json::to_value(&event).unwrap();
        assert_eq!(
            serde_json::from_value::<InvocationEvent>(encoded).unwrap(),
            event
        );
    }
}

#[tokio::test]
async fn revision_mismatch_fails_before_execution_with_unambiguous_event_fields() {
    let agent = Agent::new(
        "worker",
        "Worker",
        "Answer.",
        Arc::new(QueueModel::new([ModelResponse::final_output("unused")])),
    )
    .with_revision("rev-current".parse().unwrap());
    let observer = Arc::new(RecordingInvocationObserver::default());
    let runner = Runner::new([agent])
        .unwrap()
        .with_invocation_observer(observer.clone());
    let target = AgentRef::with_revision("worker", "rev-stale").unwrap();

    let failure = runner
        .run_invocation(
            InvocationRequest::new(
                InvocationContext::root("33333333-3333-4333-8333-333333333333".parse().unwrap()),
                InvocationSource::Caller,
                target.clone(),
            ),
            "Do not execute.",
            &(),
            RunOptions::default(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::AgentRevisionMismatch { .. }
    ));
    let events = observer.events();
    assert_eq!(events.len(), 2, "expected start and terminal failure");
    assert_eq!(events[0].requested_target, target);
    assert!(matches!(
        &events[1].event,
        InvocationEventKind::Failed {
            failed_at: None,
            kind: InvocationFailureKind::Configuration
        }
    ));
}

#[tokio::test]
async fn explicit_invocation_requires_the_registered_revision_when_one_exists() {
    let agent = Agent::new(
        "worker",
        "Worker",
        "Answer.",
        Arc::new(QueueModel::new([ModelResponse::final_output("unused")])),
    )
    .with_revision("rev-current".parse().unwrap());
    let runner = Runner::new([agent]).unwrap();

    let failure = runner
        .run_invocation(
            InvocationRequest::new(
                InvocationContext::root("44444444-4444-4444-8444-444444444444".parse().unwrap()),
                InvocationSource::Caller,
                AgentRef::new("worker").unwrap(),
            ),
            "Do not execute.",
            &(),
            RunOptions::default(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::AgentRevisionRequired { .. }
    ));
}

#[tokio::test]
async fn explicit_invocation_rejects_an_unrevisioned_registration() {
    let agent = Agent::new(
        "worker",
        "Worker",
        "Answer.",
        Arc::new(QueueModel::new([ModelResponse::final_output("unused")])),
    );
    let runner = Runner::new([agent]).unwrap();

    let failure = runner
        .run_invocation(
            InvocationRequest::new(
                InvocationContext::root("55555555-5555-4555-8555-555555555555".parse().unwrap()),
                InvocationSource::Caller,
                AgentRef::new("worker").unwrap(),
            ),
            "Do not execute.",
            &(),
            RunOptions::default(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::AgentRevisionRequired { ref agent } if agent.as_str() == "worker"
    ));
}

#[tokio::test]
async fn compatibility_run_resolves_the_registered_revision_for_canonical_events() {
    let agent = Agent::new(
        "worker",
        "Worker",
        "Answer.",
        Arc::new(QueueModel::new([ModelResponse::final_output("Done.")])),
    )
    .with_revision("rev-current".parse().unwrap());
    let observer = Arc::new(RecordingInvocationObserver::default());
    let runner = Runner::new([agent])
        .unwrap()
        .with_invocation_observer(observer.clone());

    runner
        .run("worker", "Execute.", &(), RunOptions::default())
        .await
        .unwrap();

    let expected = AgentRef::with_revision("worker", "rev-current").unwrap();
    let events = observer.events();
    assert!(events
        .iter()
        .all(|event| event.requested_target == expected));
}

#[tokio::test]
async fn execution_failure_reports_the_exact_agent_revision_where_it_failed() {
    let agent = Agent::new("worker", "Worker", "Answer.", Arc::new(QueueModel::new([])))
        .with_revision("rev-current".parse().unwrap());
    let observer = Arc::new(RecordingInvocationObserver::default());
    let runner = Runner::new([agent])
        .unwrap()
        .with_invocation_observer(observer.clone());

    let failure = runner
        .run("worker", "Fail.", &(), RunOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(failure.error, RunError::ModelFailed { .. }));
    let events = observer.events();
    assert!(matches!(
        &events.last().unwrap().event,
        InvocationEventKind::Failed {
            failed_at: Some(agent),
            kind: InvocationFailureKind::Model
        } if agent == &AgentRef::with_revision("worker", "rev-current").unwrap()
    ));
}
