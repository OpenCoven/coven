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

struct CountingCallTool {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool<()> for CountingCallTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "add",
            "Counts executions",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": true
            }),
        )
    }

    async fn execute(&self, _arguments: Value, _context: &()) -> Result<Value, BoxError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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
struct RecordingInputGuardrail {
    seen: Mutex<Vec<String>>,
}

impl RecordingInputGuardrail {
    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl InputGuardrail<()> for RecordingInputGuardrail {
    fn name(&self) -> &str {
        "record-input"
    }

    async fn check(&self, input: &str, _context: &()) -> Result<GuardrailVerdict, BoxError> {
        self.seen.lock().unwrap().push(input.to_owned());
        Ok(GuardrailVerdict::Allow)
    }
}

struct FailingInputGuardrail;

#[async_trait]
impl InputGuardrail<()> for FailingInputGuardrail {
    fn name(&self) -> &str {
        "failing-input"
    }

    async fn check(&self, _input: &str, _context: &()) -> Result<GuardrailVerdict, BoxError> {
        Err(Box::new(io::Error::other("input guardrail exploded")) as BoxError)
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
        *failure.error,
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
        *failure.error,
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
    let repeated_call = |id: &str| {
        ModelResponse::actions(vec![ModelAction::ToolCall(ToolCall::new(
            id,
            "add",
            json!({ "left": 1, "right": 1 }),
        ))])
    };
    let model = Arc::new(QueueModel::new([
        repeated_call("call-1"),
        repeated_call("call-2"),
    ]));
    let agent = Agent::new("loop", "Loop", "Keep calling.", model).with_tool(Arc::new(AddTool));
    let runner = Runner::new([agent]).unwrap();
    let options = RunOptions {
        max_turns: 2,
        ..RunOptions::default()
    };

    let failure = runner.run("loop", "Loop.", &(), options).await.unwrap_err();

    assert!(matches!(
        *failure.error,
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
        *failure.error,
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
        *failure.error,
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
        matches!(*failure.error, RunError::UnknownStartingAgent(ref id) if id.as_str() == "missing")
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

    assert!(matches!(*failure.error, RunError::SessionUnavailable));
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

    assert!(matches!(*failure.error, RunError::ToolFailed { ref tool, .. } if tool == "explode"));
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

    assert!(matches!(*failure.error, RunError::ToolFailed { ref tool, .. } if tool == "explode"));
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

    assert!(matches!(*failure.error, RunError::ToolFailed { .. }));
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

#[tokio::test]
async fn duplicate_tool_call_ids_in_one_response_execute_no_tools() {
    let model = Arc::new(QueueModel::new([ModelResponse {
        assistant_message: Some("Adding both.".to_owned()),
        actions: vec![
            ModelAction::ToolCall(ToolCall::new("call-1", "add", json!({}))),
            ModelAction::ToolCall(ToolCall::new("call-1", "add", json!({}))),
        ],
    }]));
    let calls = Arc::new(AtomicUsize::new(0));
    let agent =
        Agent::new("worker", "Worker", "Use tools.", model).with_tool(Arc::new(CountingCallTool {
            calls: calls.clone(),
        }));
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::new([agent])
        .unwrap()
        .with_observer(observer.clone());

    let failure = runner
        .run("worker", "Add twice.", &(), RunOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(
        *failure.error,
        RunError::DuplicateToolCallId { ref call_id, .. } if call_id == "call-1"
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        failure.new_items.len(),
        2,
        "the user and assistant messages remain inspectable, got {:?}",
        failure.new_items
    );
    assert!(matches!(
        &failure.new_items[1],
        RunItem::AssistantMessage { content, .. } if content == "Adding both."
    ));
    assert!(!failure
        .new_items
        .iter()
        .any(|item| matches!(item, RunItem::ToolCall { .. })));
    assert_paired_lifecycle(&observer.events());
    assert!(observer.events().iter().any(|event| matches!(
        event,
        RunEvent::RunFailed {
            kind: RunFailureKind::InvalidResponse,
            ..
        }
    )));
}

#[tokio::test]
async fn tool_call_id_reused_across_turns_executes_only_once() {
    let repeated = || {
        ModelResponse::actions(vec![ModelAction::ToolCall(ToolCall::new(
            "call-1",
            "add",
            json!({}),
        ))])
    };
    let model = Arc::new(QueueModel::new([repeated(), repeated()]));
    let calls = Arc::new(AtomicUsize::new(0));
    let agent =
        Agent::new("worker", "Worker", "Use tools.", model).with_tool(Arc::new(CountingCallTool {
            calls: calls.clone(),
        }));
    let runner = Runner::new([agent]).unwrap();

    let failure = runner
        .run("worker", "Add twice.", &(), RunOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(
        *failure.error,
        RunError::DuplicateToolCallId { ref call_id, .. } if call_id == "call-1"
    ));
    assert_eq!(failure.turns, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        failure.new_items.len(),
        3,
        "only the first turn's unambiguous call and result are retained, got {:?}",
        failure.new_items
    );
    assert_eq!(
        failure
            .new_items
            .iter()
            .filter(|item| matches!(item, RunItem::ToolCall { .. }))
            .count(),
        1
    );
    assert_eq!(
        failure
            .new_items
            .iter()
            .filter(|item| matches!(item, RunItem::ToolResult { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn resumed_history_rejects_reused_tool_call_id() {
    let session = Arc::new(InMemorySession::default());
    session
        .append(
            "conversation-1",
            &[RunItem::ToolCall {
                agent: "worker".into(),
                call: ToolCall::new("call-1", "add", json!({})),
            }],
        )
        .await
        .unwrap();
    let model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::ToolCall(ToolCall::new("call-1", "add", json!({}))),
    ])]));
    let calls = Arc::new(AtomicUsize::new(0));
    let agent =
        Agent::new("worker", "Worker", "Use tools.", model).with_tool(Arc::new(CountingCallTool {
            calls: calls.clone(),
        }));
    let runner = Runner::new([agent]).unwrap().with_session(session.clone());

    let failure = runner
        .run(
            "worker",
            "Add again.",
            &(),
            RunOptions {
                session_id: Some("conversation-1".to_owned()),
                ..RunOptions::default()
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(
        *failure.error,
        RunError::DuplicateToolCallId { ref call_id, .. } if call_id == "call-1"
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(session.items("conversation-1").await.unwrap().len(), 1);
}

#[tokio::test]
async fn resumed_result_only_history_rejects_reused_tool_call_id() {
    let session = Arc::new(InMemorySession::default());
    session
        .append(
            "conversation-1",
            &[RunItem::ToolResult {
                agent: "worker".into(),
                call_id: "call-1".to_owned(),
                tool: "add".to_owned(),
                output: json!({ "ok": true }),
            }],
        )
        .await
        .unwrap();
    let model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::ToolCall(ToolCall::new("call-1", "add", json!({}))),
    ])]));
    let calls = Arc::new(AtomicUsize::new(0));
    let agent =
        Agent::new("worker", "Worker", "Use tools.", model).with_tool(Arc::new(CountingCallTool {
            calls: calls.clone(),
        }));
    let runner = Runner::new([agent]).unwrap().with_session(session.clone());

    let failure = runner
        .run(
            "worker",
            "Add again.",
            &(),
            RunOptions {
                session_id: Some("conversation-1".to_owned()),
                ..RunOptions::default()
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(
        *failure.error,
        RunError::DuplicateToolCallId { ref call_id, .. } if call_id == "call-1"
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(session.items("conversation-1").await.unwrap().len(), 1);
}

#[tokio::test]
async fn unique_tool_call_ids_preserve_tool_execution() {
    let model = Arc::new(QueueModel::new([
        ModelResponse::actions(vec![
            ModelAction::ToolCall(ToolCall::new("call-1", "add", json!({}))),
            ModelAction::ToolCall(ToolCall::new("call-2", "add", json!({}))),
        ]),
        ModelResponse::final_output("Done."),
    ]));
    let calls = Arc::new(AtomicUsize::new(0));
    let agent =
        Agent::new("worker", "Worker", "Use tools.", model).with_tool(Arc::new(CountingCallTool {
            calls: calls.clone(),
        }));
    let runner = Runner::new([agent]).unwrap();

    let result = runner
        .run("worker", "Add twice.", &(), RunOptions::default())
        .await
        .unwrap();

    assert_eq!(result.final_output, "Done.");
    assert_eq!(result.turns, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        result
            .new_items
            .iter()
            .filter(|item| matches!(item, RunItem::ToolResult { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn handoff_target_enforces_the_same_input_policy_as_direct_entry() {
    let direct_model = Arc::new(QueueModel::new([ModelResponse::final_output(
        "should not run",
    )]));
    let direct = Agent::new("specialist", "Specialist", "Be safe.", direct_model.clone())
        .with_input_guardrail(Arc::new(RejectInput));
    let direct_runner = Runner::new([direct]).unwrap();

    let direct_failure = direct_runner
        .run("specialist", "blocked input", &(), RunOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(
        direct_failure.error,
        RunError::GuardrailRejected {
            ref agent,
            stage: GuardrailStage::Input,
            ref reason,
            ..
        } if agent.as_str() == "specialist" && reason == "blocked by policy"
    ));
    assert_eq!(direct_model.calls.load(Ordering::SeqCst), 0);

    let triage_model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::Handoff(HandoffCall::new("to-specialist")),
    ])]));
    let specialist_model = Arc::new(QueueModel::new([ModelResponse::final_output(
        "must not run either",
    )]));
    let target_tool_calls = Arc::new(AtomicUsize::new(0));
    let triage = Agent::new("triage", "Triage", "Route the request.", triage_model).with_handoff(
        Handoff::new("to-specialist", "Use for specialist work", "specialist"),
    );
    let specialist = Agent::new(
        "specialist",
        "Specialist",
        "Handle specialist work.",
        specialist_model.clone(),
    )
    .with_input_guardrail(Arc::new(RejectInput))
    .with_tool(Arc::new(CountingCallTool {
        calls: target_tool_calls.clone(),
    }));
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::new([triage, specialist])
        .unwrap()
        .with_observer(observer.clone());

    let failure = runner
        .run("triage", "blocked input", &(), RunOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::GuardrailRejected {
            ref agent,
            stage: GuardrailStage::Input,
            ref reason,
            ..
        } if agent.as_str() == "specialist" && reason == "blocked by policy"
    ));
    assert_eq!(
        specialist_model.calls.load(Ordering::SeqCst),
        0,
        "a handoff target that rejects the input must not receive a model call"
    );
    assert_eq!(
        target_tool_calls.load(Ordering::SeqCst),
        0,
        "a handoff target that rejects the input must not execute tools"
    );
    assert_eq!(failure.turns, 1, "only the source agent's turn ran");
    assert_eq!(failure.handoffs, 1);
    assert!(matches!(
        failure.new_items.as_slice(),
        [RunItem::UserMessage { .. }, RunItem::Handoff { to, .. }] if to.as_str() == "specialist"
    ));
    let events = observer.events();
    assert_paired_lifecycle(&events);
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::GuardrailChecked {
            agent,
            stage: GuardrailStage::Input,
            allowed: false,
            ..
        } if agent.as_str() == "specialist"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::RunFailed {
            kind: RunFailureKind::InputGuardrail,
            ..
        }
    )));
}

#[tokio::test]
async fn handoff_target_input_guardrail_runs_before_the_target_model_turn() {
    let triage_model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::Handoff(HandoffCall::new("to-specialist")),
    ])]));
    let specialist_model = Arc::new(QueueModel::new([ModelResponse::final_output(
        "Handled by the specialist.",
    )]));
    let specialist_guardrail = Arc::new(RecordingInputGuardrail::default());
    let triage = Agent::new("triage", "Triage", "Route the request.", triage_model).with_handoff(
        Handoff::new("to-specialist", "Use for specialist work", "specialist"),
    );
    let specialist = Agent::new(
        "specialist",
        "Specialist",
        "Handle specialist work.",
        specialist_model,
    )
    .with_input_guardrail(specialist_guardrail.clone());
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::new([triage, specialist])
        .unwrap()
        .with_observer(observer.clone());

    let result = runner
        .run("triage", "Please route this.", &(), RunOptions::default())
        .await
        .unwrap();

    assert_eq!(result.final_agent.as_str(), "specialist");
    assert_eq!(
        specialist_guardrail.seen(),
        ["Please route this."],
        "the target's ingress policy evaluates the original user input, the same value a direct start would check"
    );

    let events = observer.events();
    let handoff_position = events
        .iter()
        .position(
            |event| matches!(event, RunEvent::Handoff { to, .. } if to.as_str() == "specialist"),
        )
        .unwrap();
    let checked_position = events
        .iter()
        .position(|event| {
            matches!(event, RunEvent::GuardrailChecked { agent, .. } if agent.as_str() == "specialist")
        })
        .unwrap();
    let target_model_position = events
        .iter()
        .position(|event| {
            matches!(event, RunEvent::ModelRequested { agent, .. } if agent.as_str() == "specialist")
        })
        .unwrap();
    assert!(
        handoff_position < checked_position && checked_position < target_model_position,
        "the target's ingress check must land between the handoff and the target's first model turn, got {events:?}"
    );
    assert!(matches!(
        &events[checked_position],
        RunEvent::GuardrailChecked {
            stage: GuardrailStage::Input,
            allowed: true,
            ..
        }
    ));
}

#[tokio::test]
async fn multi_hop_handoff_enforces_input_policy_at_every_boundary() {
    let a_model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::Handoff(HandoffCall::new("to-b")),
    ])]));
    let b_model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::Handoff(HandoffCall::new("to-c")),
    ])]));
    let c_model = Arc::new(QueueModel::new([ModelResponse::final_output("Done by c.")]));
    let b_guardrail = Arc::new(RecordingInputGuardrail::default());
    let c_guardrail = Arc::new(RecordingInputGuardrail::default());
    let a = Agent::new("a", "A", "Route.", a_model).with_handoff(Handoff::new(
        "to-b",
        "Route to b",
        "b",
    ));
    let b = Agent::new("b", "B", "Route.", b_model)
        .with_handoff(Handoff::new("to-c", "Route to c", "c"))
        .with_input_guardrail(b_guardrail.clone());
    let c = Agent::new("c", "C", "Answer.", c_model).with_input_guardrail(c_guardrail.clone());
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::new([a, b, c])
        .unwrap()
        .with_observer(observer.clone());

    let result = runner
        .run("a", "Cross the coven.", &(), RunOptions::default())
        .await
        .unwrap();

    assert_eq!(result.final_agent.as_str(), "c");
    assert_eq!(result.handoffs, 2);
    assert_eq!(result.turns, 3);
    assert_eq!(b_guardrail.seen(), ["Cross the coven."]);
    assert_eq!(c_guardrail.seen(), ["Cross the coven."]);

    let events = observer.events();
    let b_check = events
        .iter()
        .position(|event| {
            matches!(event, RunEvent::GuardrailChecked { agent, .. } if agent.as_str() == "b")
        })
        .unwrap();
    let c_check = events
        .iter()
        .position(|event| {
            matches!(event, RunEvent::GuardrailChecked { agent, .. } if agent.as_str() == "c")
        })
        .unwrap();
    assert!(
        b_check < c_check,
        "each hop must clear its own ingress policy before the next model turn, got {events:?}"
    );
}

#[tokio::test]
async fn multi_hop_handoff_target_rejection_prevents_the_target_model_turn() {
    let a_model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::Handoff(HandoffCall::new("to-b")),
    ])]));
    let b_model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::Handoff(HandoffCall::new("to-c")),
    ])]));
    let c_model = Arc::new(QueueModel::new([ModelResponse::final_output(
        "must not run",
    )]));
    let a = Agent::new("a", "A", "Route.", a_model).with_handoff(Handoff::new(
        "to-b",
        "Route to b",
        "b",
    ));
    let b = Agent::new("b", "B", "Route.", b_model).with_handoff(Handoff::new(
        "to-c",
        "Route to c",
        "c",
    ));
    let c = Agent::new("c", "C", "Answer.", c_model.clone())
        .with_input_guardrail(Arc::new(RejectInput));
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::new([a, b, c])
        .unwrap()
        .with_observer(observer.clone());

    let failure = runner
        .run("a", "blocked input", &(), RunOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::GuardrailRejected {
            ref agent,
            stage: GuardrailStage::Input,
            ..
        } if agent.as_str() == "c"
    ));
    assert_eq!(
        c_model.calls.load(Ordering::SeqCst),
        0,
        "the rejecting target must not receive a model call"
    );
    assert_eq!(failure.turns, 2, "only the upstream agents' turns ran");
    assert_eq!(failure.handoffs, 2);
    assert_paired_lifecycle(&observer.events());
    assert!(observer.events().iter().any(|event| matches!(
        event,
        RunEvent::RunFailed {
            kind: RunFailureKind::InputGuardrail,
            ..
        }
    )));
}

#[tokio::test]
async fn handoff_target_guardrail_error_is_distinguishable_from_a_rejection() {
    let triage_model = Arc::new(QueueModel::new([ModelResponse::actions(vec![
        ModelAction::Handoff(HandoffCall::new("to-specialist")),
    ])]));
    let specialist_model = Arc::new(QueueModel::new([ModelResponse::final_output("unused")]));
    let triage = Agent::new("triage", "Triage", "Route the request.", triage_model).with_handoff(
        Handoff::new("to-specialist", "Use for specialist work", "specialist"),
    );
    let specialist = Agent::new(
        "specialist",
        "Specialist",
        "Handle specialist work.",
        specialist_model.clone(),
    )
    .with_input_guardrail(Arc::new(FailingInputGuardrail));
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::new([triage, specialist])
        .unwrap()
        .with_observer(observer.clone());

    let failure = runner
        .run("triage", "Any input.", &(), RunOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::GuardrailFailed {
            ref agent,
            ref guardrail,
            stage: GuardrailStage::Input,
            ..
        } if agent.as_str() == "specialist" && guardrail == "failing-input"
    ));
    assert_eq!(
        failure.to_string(),
        "input guardrail `failing-input` for agent `specialist` failed",
        "an implementation error must not read as a policy rejection"
    );
    assert_eq!(
        specialist_model.calls.load(Ordering::SeqCst),
        0,
        "a guardrail implementation error must still stop the target model turn"
    );
    assert_paired_lifecycle(&observer.events());
}

#[tokio::test]
async fn failing_input_guardrail_is_distinguishable_from_a_rejection() {
    let model = Arc::new(QueueModel::new([ModelResponse::final_output("unused")]));
    let agent = Agent::new("safe", "Safe", "Be safe.", model.clone())
        .with_input_guardrail(Arc::new(FailingInputGuardrail));
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::new([agent])
        .unwrap()
        .with_observer(observer.clone());

    let failure = runner
        .run("safe", "Any input.", &(), RunOptions::default())
        .await
        .unwrap_err();

    assert_eq!(
        failure.to_string(),
        "input guardrail `failing-input` for agent `safe` failed"
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert_paired_lifecycle(&observer.events());
    assert!(observer.events().iter().any(|event| matches!(
        event,
        RunEvent::RunFailed {
            kind: RunFailureKind::InputGuardrail,
            ..
        }
    )));
}

#[tokio::test]
async fn handoff_cannot_be_combined_with_tool_calls() {
    let model = Arc::new(QueueModel::new([ModelResponse {
        assistant_message: Some("Routing and calculating.".to_owned()),
        actions: vec![
            ModelAction::Handoff(HandoffCall::new("to-worker")),
            ModelAction::ToolCall(ToolCall::new(
                "call-1",
                "add",
                json!({ "left": 1, "right": 2 }),
            )),
        ],
    }]));
    let calls = Arc::new(AtomicUsize::new(0));
    let worker_model = Arc::new(QueueModel::new([ModelResponse::final_output("unused")]));
    let triage = Agent::new("triage", "Triage", "Route.", model).with_handoff(Handoff::new(
        "to-worker",
        "Route to worker",
        "worker",
    ));
    let worker = Agent::new("worker", "Worker", "Use tools.", worker_model).with_tool(Arc::new(
        CountingCallTool {
            calls: calls.clone(),
        },
    ));
    let runner = Runner::new([triage, worker]).unwrap();

    let failure = runner
        .run("triage", "Route and add.", &(), RunOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::InvalidModelResponse { ref reason, .. } if reason == "a handoff cannot be combined with other actions"
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
