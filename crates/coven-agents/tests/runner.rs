use std::{
    collections::VecDeque,
    io,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use async_trait::async_trait;
use coven_agents::{
    Agent, BoxError, ConfigError, GuardrailStage, GuardrailVerdict, Handoff, HandoffCall,
    InMemorySession, InputGuardrail, Model, ModelAction, ModelRequest, ModelResponse,
    OutputGuardrail, RunError, RunEvent, RunFailureKind, RunItem, RunObserver, RunOptions, Runner,
    SessionStore, Tool, ToolCall, ToolDefinition,
};
use serde_json::{json, Value};

#[derive(Default)]
struct QueueModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    requests: Mutex<Vec<ModelRequest>>,
    calls: AtomicUsize,
}

impl QueueModel {
    fn new(responses: impl IntoIterator<Item = ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            ..Self::default()
        }
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl Model<()> for QueueModel {
    async fn generate(
        &self,
        request: ModelRequest,
        _context: &(),
    ) -> Result<ModelResponse, BoxError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| Box::new(io::Error::other("no queued response")) as BoxError)
    }
}

struct AddTool;

#[async_trait]
impl Tool<()> for AddTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "add",
            "Add two integers",
            json!({
                "type": "object",
                "properties": {
                    "left": { "type": "integer" },
                    "right": { "type": "integer" }
                },
                "required": ["left", "right"],
                "additionalProperties": false
            }),
        )
    }

    async fn execute(&self, arguments: Value, _context: &()) -> Result<Value, BoxError> {
        let left = arguments
            .get("left")
            .and_then(Value::as_i64)
            .ok_or_else(|| Box::new(io::Error::other("missing left")) as BoxError)?;
        let right = arguments
            .get("right")
            .and_then(Value::as_i64)
            .ok_or_else(|| Box::new(io::Error::other("missing right")) as BoxError)?;
        Ok(json!({ "sum": left + right }))
    }
}

struct CountingDefinitionTool {
    definition_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool<()> for CountingDefinitionTool {
    fn definition(&self) -> ToolDefinition {
        self.definition_calls.fetch_add(1, Ordering::SeqCst);
        ToolDefinition::new(
            "counted",
            "Counts definition calls",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        )
    }

    async fn execute(&self, _arguments: Value, _context: &()) -> Result<Value, BoxError> {
        Ok(json!({ "ok": true }))
    }
}

struct RejectInput;

#[async_trait]
impl InputGuardrail<()> for RejectInput {
    fn name(&self) -> &str {
        "reject-input"
    }

    async fn check(&self, _input: &str, _context: &()) -> Result<GuardrailVerdict, BoxError> {
        Ok(GuardrailVerdict::reject("blocked by policy"))
    }
}

struct RejectOutput;

#[async_trait]
impl OutputGuardrail<()> for RejectOutput {
    fn name(&self) -> &str {
        "reject-output"
    }

    async fn check(&self, _output: &str, _context: &()) -> Result<GuardrailVerdict, BoxError> {
        Ok(GuardrailVerdict::reject("output blocked by policy"))
    }
}

#[derive(Default)]
struct RecordingObserver {
    events: Mutex<Vec<RunEvent>>,
}

impl RecordingObserver {
    fn events(&self) -> Vec<RunEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl RunObserver for RecordingObserver {
    fn on_event(&self, event: &RunEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

/// Asserts the lifecycle contract: a run opens with exactly one `RunStarted`
/// and closes with exactly one terminal event, so observers that pair a start
/// with an end never leak or orphan per-run state.
fn assert_paired_lifecycle(events: &[RunEvent]) {
    let started = events
        .iter()
        .filter(|event| matches!(event, RunEvent::RunStarted { .. }))
        .count();
    let terminal = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RunEvent::RunCompleted { .. } | RunEvent::RunFailed { .. }
            )
        })
        .count();

    assert_eq!(started, 1, "expected one RunStarted, got {events:?}");
    assert_eq!(terminal, 1, "expected one terminal event, got {events:?}");
    assert!(
        matches!(events.first(), Some(RunEvent::RunStarted { .. })),
        "RunStarted must be the first event, got {events:?}"
    );
    assert!(
        matches!(
            events.last(),
            Some(RunEvent::RunCompleted { .. } | RunEvent::RunFailed { .. })
        ),
        "a terminal event must be last, got {events:?}"
    );
}

struct FailingTool;

#[async_trait]
impl Tool<()> for FailingTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "explode",
            "Always fails",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        )
    }

    async fn execute(&self, _arguments: Value, _context: &()) -> Result<Value, BoxError> {
        Err(Box::new(io::Error::other("tool exploded")) as BoxError)
    }
}

#[tokio::test]
async fn executes_tools_and_returns_the_final_output() {
    let model = Arc::new(QueueModel::new([
        ModelResponse::actions(vec![ModelAction::ToolCall(ToolCall::new(
            "call-1",
            "add",
            json!({ "left": 2, "right": 3 }),
        ))]),
        ModelResponse::final_output("The sum is 5."),
    ]));
    let agent = Agent::new("math", "Math", "Use the calculator.", model.clone())
        .with_tool(Arc::new(AddTool));
    let runner = Runner::new([agent]).unwrap();

    let result = runner
        .run("math", "Add 2 and 3.", &(), RunOptions::default())
        .await
        .unwrap();

    assert_eq!(result.final_output, "The sum is 5.");
    assert_eq!(result.turns, 2);
    assert!(result.new_items.iter().any(|item| matches!(
        item,
        RunItem::ToolResult { output, .. } if output == &json!({ "sum": 5 })
    )));
    assert_eq!(model.requests().len(), 2);
}

#[tokio::test]
async fn caches_tool_definitions_across_turns() {
    let model = Arc::new(QueueModel::new([
        ModelResponse::actions(vec![ModelAction::ToolCall(ToolCall::new(
            "call-1",
            "counted",
            json!({}),
        ))]),
        ModelResponse::final_output("Done."),
    ]));
    let definition_calls = Arc::new(AtomicUsize::new(0));
    let agent = Agent::new("math", "Math", "Use the calculator.", model).with_tool(Arc::new(
        CountingDefinitionTool {
            definition_calls: definition_calls.clone(),
        },
    ));
    let runner = Runner::new([agent]).unwrap();

    let result = runner
        .run("math", "Run the counted tool.", &(), RunOptions::default())
        .await
        .unwrap();

    assert_eq!(result.turns, 2);
    assert_eq!(definition_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn hands_control_to_a_registered_agent() {
    let triage_model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::Handoff(HandoffCall::new("to-specialist")),
    ])]));
    let specialist_model = Arc::new(QueueModel::new([ModelResponse::final_output(
        "Handled by the specialist.",
    )]));
    let triage = Agent::new("triage", "Triage", "Route the request.", triage_model).with_handoff(
        Handoff::new("to-specialist", "Use for specialist work", "specialist"),
    );
    let specialist = Agent::new(
        "specialist",
        "Specialist",
        "Handle specialist work.",
        specialist_model,
    );
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::new([triage, specialist])
        .unwrap()
        .with_observer(observer.clone());

    let result = runner
        .run("triage", "Please route this.", &(), RunOptions::default())
        .await
        .unwrap();

    assert_eq!(result.final_agent.as_str(), "specialist");
    assert_eq!(result.handoffs, 1);
    assert!(observer.events().iter().any(|event| matches!(
        event,
        RunEvent::Handoff { from, to, name }
            if from.as_str() == "triage"
                && to.as_str() == "specialist"
                && name == "to-specialist"
    )));
}

#[tokio::test]
async fn blocking_input_guardrail_prevents_model_execution() {
    let model = Arc::new(QueueModel::new([ModelResponse::final_output(
        "should not run",
    )]));
    let agent = Agent::new("safe", "Safe", "Be safe.", model.clone())
        .with_input_guardrail(Arc::new(RejectInput));
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::new([agent])
        .unwrap()
        .with_observer(observer.clone());

    let failure = runner
        .run("safe", "blocked input", &(), RunOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::GuardrailRejected {
            stage: GuardrailStage::Input,
            ..
        }
    ));
    assert!(
        matches!(
            failure.new_items.as_slice(),
            [RunItem::UserMessage { content }] if content == "blocked input"
        ),
        "a rejected input is still transcript, got {:?}",
        failure.new_items
    );
    assert_eq!(failure.turns, 0);
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert!(observer.events().iter().any(|event| matches!(
        event,
        RunEvent::RunFailed {
            kind: RunFailureKind::InputGuardrail,
            ..
        }
    )));
}

#[tokio::test]
async fn session_history_is_loaded_and_successful_items_are_appended() {
    let session = Arc::new(InMemorySession::default());
    session
        .append(
            "conversation-1",
            &[RunItem::UserMessage {
                content: "Earlier message".to_owned(),
            }],
        )
        .await
        .unwrap();
    let model = Arc::new(QueueModel::new([ModelResponse::final_output(
        "Current answer",
    )]));
    let agent = Agent::new("assistant", "Assistant", "Answer.", model.clone());
    let runner = Runner::new([agent]).unwrap().with_session(session.clone());
    let options = RunOptions {
        session_id: Some("conversation-1".to_owned()),
        ..RunOptions::default()
    };

    runner
        .run("assistant", "Current message", &(), options)
        .await
        .unwrap();

    let request_items = &model.requests()[0].items;
    assert_eq!(request_items.len(), 2);
    assert!(matches!(
        &request_items[0],
        RunItem::UserMessage { content } if content == "Earlier message"
    ));
    let stored = session.items("conversation-1").await.unwrap();
    assert_eq!(stored.len(), 3);
}

#[tokio::test]
async fn rejected_output_is_not_appended_to_the_session() {
    let session = Arc::new(InMemorySession::default());
    let model = Arc::new(QueueModel::new([ModelResponse::final_output(
        "blocked output",
    )]));
    let agent = Agent::new("assistant", "Assistant", "Answer.", model)
        .with_output_guardrail(Arc::new(RejectOutput));
    let runner = Runner::new([agent]).unwrap().with_session(session.clone());
    let options = RunOptions {
        session_id: Some("conversation-1".to_owned()),
        ..RunOptions::default()
    };

    let failure = runner
        .run("assistant", "Current message", &(), options)
        .await
        .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::GuardrailRejected {
            stage: GuardrailStage::Output,
            ..
        }
    ));
    assert_eq!(
        failure.new_items.len(),
        2,
        "the rejected output must stay inspectable, got {:?}",
        failure.new_items
    );
    assert!(session.items("conversation-1").await.unwrap().is_empty());
}

#[tokio::test]
async fn bounded_runner_stops_repeated_tool_turns() {
    let repeated_call = || {
        ModelResponse::actions(vec![ModelAction::ToolCall(ToolCall::new(
            "call",
            "add",
            json!({ "left": 1, "right": 1 }),
        ))])
    };
    let model = Arc::new(QueueModel::new([repeated_call(), repeated_call()]));
    let agent = Agent::new("loop", "Loop", "Keep calling.", model).with_tool(Arc::new(AddTool));
    let runner = Runner::new([agent]).unwrap();
    let options = RunOptions {
        max_turns: 2,
        ..RunOptions::default()
    };

    let failure = runner.run("loop", "Loop.", &(), options).await.unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::MaxTurnsExceeded { limit: 2 }
    ));
    assert_eq!(failure.turns, 2);
    assert_eq!(
        failure.new_items.len(),
        5,
        "an exhausted run keeps every item it produced, got {:?}",
        failure.new_items
    );
}

#[tokio::test]
async fn bounded_runner_stops_repeated_handoffs() {
    let first_model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::Handoff(HandoffCall::new("to-second")),
    ])]));
    let second_model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::Handoff(HandoffCall::new("to-first")),
    ])]));
    let first = Agent::new("first", "First", "Route.", first_model).with_handoff(Handoff::new(
        "to-second",
        "Route to second",
        "second",
    ));
    let second = Agent::new("second", "Second", "Route.", second_model).with_handoff(Handoff::new(
        "to-first",
        "Route to first",
        "first",
    ));
    let runner = Runner::new([first, second]).unwrap();
    let options = RunOptions {
        max_handoffs: 1,
        ..RunOptions::default()
    };

    let failure = runner
        .run("first", "Keep routing.", &(), options)
        .await
        .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::MaxHandoffsExceeded { limit: 1 }
    ));
    assert_eq!(failure.handoffs, 2);
}

#[tokio::test]
async fn rejects_empty_model_responses() {
    let model = Arc::new(QueueModel::new([ModelResponse {
        assistant_message: None,
        actions: Vec::new(),
    }]));
    let agent = Agent::new("empty", "Empty", "Answer.", model);
    let runner = Runner::new([agent]).unwrap();

    let failure = runner
        .run("empty", "Answer.", &(), RunOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::InvalidModelResponse { .. }
    ));
}

#[test]
fn rejects_handoffs_to_unregistered_agents() {
    let model = Arc::new(QueueModel::default());
    let agent = Agent::new("triage", "Triage", "Route.", model).with_handoff(Handoff::new(
        "missing",
        "Missing target",
        "not-registered",
    ));

    let error = Runner::new([agent]).err().unwrap();

    assert_eq!(
        error,
        ConfigError::UnknownHandoffTarget {
            agent: "triage".into(),
            handoff: "missing".to_owned(),
            target: "not-registered".into(),
        }
    );
}

#[tokio::test]
async fn unknown_starting_agent_reports_a_paired_run_lifecycle() {
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("unused")]));
    let agent = Agent::new("known", "Known", "Answer.", model.clone());
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::new([agent])
        .unwrap()
        .with_observer(observer.clone());

    let failure = runner
        .run("missing", "Answer.", &(), RunOptions::default())
        .await
        .unwrap_err();

    assert!(
        matches!(failure.error, RunError::UnknownStartingAgent(ref id) if id.as_str() == "missing")
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert_paired_lifecycle(&observer.events());
}

#[tokio::test]
async fn missing_session_store_reports_a_paired_run_lifecycle() {
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("unused")]));
    let agent = Agent::new("assistant", "Assistant", "Answer.", model.clone());
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::new([agent])
        .unwrap()
        .with_observer(observer.clone());
    let options = RunOptions {
        session_id: Some("conversation-1".to_owned()),
        ..RunOptions::default()
    };

    let failure = runner
        .run("assistant", "Answer.", &(), options)
        .await
        .unwrap_err();

    assert!(matches!(failure.error, RunError::SessionUnavailable));
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert_paired_lifecycle(&observer.events());
}

#[tokio::test]
async fn tool_failure_reports_a_paired_run_lifecycle() {
    let model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::ToolCall(ToolCall::new("call-1", "explode", json!({}))),
    ])]));
    let agent =
        Agent::new("worker", "Worker", "Use tools.", model).with_tool(Arc::new(FailingTool));
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::new([agent])
        .unwrap()
        .with_observer(observer.clone());

    let failure = runner
        .run("worker", "Do it.", &(), RunOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(failure.error, RunError::ToolFailed { ref tool, .. } if tool == "explode"));
    let events = observer.events();
    assert_paired_lifecycle(&events);
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::RunFailed {
            kind: RunFailureKind::Tool,
            ..
        }
    )));
}

#[tokio::test]
async fn successful_run_reports_a_paired_run_lifecycle() {
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("Done.")]));
    let agent = Agent::new("assistant", "Assistant", "Answer.", model);
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::new([agent])
        .unwrap()
        .with_observer(observer.clone());

    let result = runner
        .run("assistant", "Answer.", &(), RunOptions::default())
        .await
        .unwrap();

    assert_eq!(result.final_output, "Done.");
    assert_paired_lifecycle(&observer.events());
}

#[tokio::test]
async fn tool_failure_returns_the_partial_transcript() {
    let session = Arc::new(InMemorySession::default());
    let model = Arc::new(QueueModel::new([ModelResponse {
        assistant_message: Some("Working on it.".to_owned()),
        actions: vec![
            ModelAction::ToolCall(ToolCall::new(
                "call-1",
                "add",
                json!({ "left": 1, "right": 2 }),
            )),
            ModelAction::ToolCall(ToolCall::new("call-2", "explode", json!({}))),
        ],
    }]));
    let agent = Agent::new("worker", "Worker", "Use tools.", model)
        .with_tool(Arc::new(AddTool))
        .with_tool(Arc::new(FailingTool));
    let runner = Runner::new([agent]).unwrap().with_session(session.clone());
    let options = RunOptions {
        session_id: Some("conversation-1".to_owned()),
        ..RunOptions::default()
    };

    let failure = runner
        .run("worker", "Add then explode.", &(), options)
        .await
        .unwrap_err();

    assert!(matches!(failure.error, RunError::ToolFailed { ref tool, .. } if tool == "explode"));
    assert_eq!(
        failure.to_string(),
        "tool `explode` for agent `worker` failed"
    );
    assert_eq!(failure.turns, 1);
    assert_eq!(failure.handoffs, 0);

    let items = &failure.new_items;
    assert_eq!(
        items.len(),
        5,
        "the work performed before the failure must survive, got {items:?}"
    );
    assert!(matches!(
        &items[0],
        RunItem::UserMessage { content } if content == "Add then explode."
    ));
    assert!(matches!(
        &items[1],
        RunItem::AssistantMessage { content, .. } if content == "Working on it."
    ));
    assert!(matches!(
        &items[2],
        RunItem::ToolCall { call, .. } if call.name == "add"
    ));
    assert!(matches!(
        &items[3],
        RunItem::ToolResult { tool, .. } if tool == "add"
    ));
    assert!(matches!(
        &items[4],
        RunItem::ToolCall { call, .. } if call.name == "explode"
    ));

    assert!(
        session.items("conversation-1").await.unwrap().is_empty(),
        "a failed run is returned to the caller, never written as durable history"
    );
}

#[tokio::test]
async fn tool_failure_after_a_handoff_returns_the_whole_run_transcript() {
    let first_model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::Handoff(HandoffCall::new("to-worker")),
    ])]));
    let worker_model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::ToolCall(ToolCall::new("call-1", "explode", json!({}))),
    ])]));
    let triage = Agent::new("triage", "Triage", "Route.", first_model).with_handoff(Handoff::new(
        "to-worker",
        "Route to worker",
        "worker",
    ));
    let worker =
        Agent::new("worker", "Worker", "Use tools.", worker_model).with_tool(Arc::new(FailingTool));
    let runner = Runner::new([triage, worker]).unwrap();

    let failure = runner
        .run("triage", "Do it.", &(), RunOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(failure.error, RunError::ToolFailed { .. }));
    assert_eq!(failure.turns, 2);
    assert_eq!(failure.handoffs, 1);

    let items = &failure.new_items;
    assert_eq!(
        items.len(),
        3,
        "expected the full cross-agent transcript, got {items:?}"
    );
    assert!(matches!(&items[0], RunItem::UserMessage { .. }));
    assert!(matches!(
        &items[1],
        RunItem::Handoff { from, to, .. } if from.as_str() == "triage" && to.as_str() == "worker"
    ));
    assert!(matches!(
        &items[2],
        RunItem::ToolCall { agent, call } if agent.as_str() == "worker" && call.name == "explode"
    ));
}
