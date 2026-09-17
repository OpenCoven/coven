//! Characterize legacy control transfer, not the future bounded delegation contract.

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
    Agent, BoxError, GuardrailVerdict, Handoff, HandoffCall, InMemorySession, InputGuardrail,
    Model, ModelAction, ModelRequest, ModelResponse, OutputGuardrail, RunError, RunItem,
    RunOptions, Runner, SessionStore, Tool, ToolCall, ToolDefinition,
};
use serde_json::{json, Value};

struct ScriptedModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl ScriptedModel {
    fn new(responses: impl IntoIterator<Item = ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Model<AtomicUsize> for ScriptedModel {
    async fn generate(
        &self,
        request: ModelRequest,
        _context: &AtomicUsize,
    ) -> Result<ModelResponse, BoxError> {
        self.requests.lock().unwrap().push(request);
        Ok(self.responses.lock().unwrap().pop_front().unwrap())
    }
}

struct EffectTool(&'static str);

#[async_trait]
impl Tool<AtomicUsize> for EffectTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.0, "Synthetic test effect", json!({ "type": "object" }))
    }

    async fn execute(&self, _arguments: Value, context: &AtomicUsize) -> Result<Value, BoxError> {
        context.fetch_add(1, Ordering::SeqCst);
        Ok(json!("CANARY_TOOL_OUTPUT"))
    }
}

#[derive(Default)]
struct RecordingIngress(Mutex<Vec<String>>);

#[async_trait]
impl InputGuardrail<AtomicUsize> for RecordingIngress {
    fn name(&self) -> &str {
        "record-ingress"
    }

    async fn check(
        &self,
        input: &str,
        _context: &AtomicUsize,
    ) -> Result<GuardrailVerdict, BoxError> {
        self.0.lock().unwrap().push(input.to_owned());
        Ok(GuardrailVerdict::Allow)
    }
}

#[derive(Default)]
struct RejectOutput(AtomicUsize);

#[async_trait]
impl OutputGuardrail<AtomicUsize> for RejectOutput {
    fn name(&self) -> &str {
        "reject-output"
    }

    async fn check(
        &self,
        _output: &str,
        _context: &AtomicUsize,
    ) -> Result<GuardrailVerdict, BoxError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(GuardrailVerdict::reject("synthetic rejection"))
    }
}

#[tokio::test]
async fn legacy_handoff_inherits_history_and_host_context_but_checks_only_root_input() {
    let source_call = ToolCall::new("source-call", "source-tool", json!({}));
    let source_model = Arc::new(ScriptedModel::new([
        ModelResponse::actions(vec![ModelAction::ToolCall(source_call.clone())]),
        ModelResponse {
            assistant_message: Some("CANARY_INTERMEDIATE_MESSAGE".to_owned()),
            actions: vec![ModelAction::Handoff(HandoffCall::new("transfer"))],
        },
    ]));
    let target_model = Arc::new(ScriptedModel::new([
        ModelResponse::actions(vec![ModelAction::ToolCall(ToolCall::new(
            "target-call",
            "target-only-tool",
            json!({}),
        ))]),
        ModelResponse::final_output("done"),
    ]));
    let ingress = Arc::new(RecordingIngress::default());
    let source_output = Arc::new(RejectOutput::default());
    let source = Agent::new("source", "Source", "", source_model.clone())
        .with_tool(Arc::new(EffectTool("source-tool")))
        .with_output_guardrail(source_output.clone())
        .with_handoff(Handoff::new("transfer", "Transfer control", "target"));
    let target = Agent::new("target", "Target", "", target_model.clone())
        .with_tool(Arc::new(EffectTool("target-only-tool")))
        .with_input_guardrail(ingress.clone());
    let history = RunItem::AssistantMessage {
        agent: "previous-agent".into(),
        content: "CANARY_SESSION_HISTORY".to_owned(),
    };
    let session = Arc::new(InMemorySession::default());
    session
        .append("history", std::slice::from_ref(&history))
        .await
        .unwrap();
    let context = AtomicUsize::new(0);
    let result = Runner::new([source, target])
        .unwrap()
        .with_session(session.clone())
        .run(
            "source",
            "diagnose fixture",
            &context,
            RunOptions {
                session_id: Some("history".to_owned()),
                ..RunOptions::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(result.final_output, "done");
    assert_eq!(result.final_agent.as_str(), "target");
    assert_eq!(*ingress.0.lock().unwrap(), ["diagnose fixture"]);
    assert_eq!(source_output.0.load(Ordering::SeqCst), 0);
    assert_eq!(context.load(Ordering::SeqCst), 2);
    let requests = target_model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].items,
        vec![
            history.clone(),
            RunItem::UserMessage {
                content: "diagnose fixture".to_owned(),
            },
            RunItem::ToolCall {
                agent: "source".into(),
                call: source_call,
            },
            RunItem::ToolResult {
                agent: "source".into(),
                call_id: "source-call".to_owned(),
                tool: "source-tool".to_owned(),
                output: json!("CANARY_TOOL_OUTPUT"),
            },
            RunItem::AssistantMessage {
                agent: "source".into(),
                content: "CANARY_INTERMEDIATE_MESSAGE".to_owned(),
            },
            RunItem::Handoff {
                from: "source".into(),
                to: "target".into(),
                name: "transfer".to_owned(),
            },
        ]
    );
    assert_eq!(requests[0].tools.len(), 1);
    assert_eq!(requests[0].tools[0].name, "target-only-tool");
    assert_eq!(
        source_model.requests.lock().unwrap()[0].tools[0].name,
        "source-tool"
    );
    let mut expected_session = vec![history];
    expected_session.extend(result.new_items);
    assert_eq!(session.items("history").await.unwrap(), expected_session);
}

struct FailingAppendSession {
    append_calls: AtomicUsize,
    attempted_items: Mutex<Vec<RunItem>>,
}

#[async_trait]
impl SessionStore for FailingAppendSession {
    async fn load(&self, _session_id: &str) -> Result<Vec<RunItem>, BoxError> {
        Ok(Vec::new())
    }

    async fn append(&self, _session_id: &str, items: &[RunItem]) -> Result<(), BoxError> {
        self.append_calls.fetch_add(1, Ordering::SeqCst);
        *self.attempted_items.lock().unwrap() = items.to_vec();
        Err(Box::new(io::Error::other("synthetic append failure")))
    }
}

#[tokio::test]
async fn session_append_failure_preserves_completed_effect_without_automatic_retry() {
    let model = Arc::new(ScriptedModel::new([
        ModelResponse::actions(vec![ModelAction::ToolCall(ToolCall::new(
            "effect-call",
            "effect",
            json!({}),
        ))]),
        ModelResponse::final_output("done"),
    ]));
    let agent =
        Agent::new("agent", "Agent", "", model.clone()).with_tool(Arc::new(EffectTool("effect")));
    let session = Arc::new(FailingAppendSession {
        append_calls: AtomicUsize::new(0),
        attempted_items: Mutex::new(Vec::new()),
    });
    let context = AtomicUsize::new(0);
    let failure = Runner::new([agent])
        .unwrap()
        .with_session(session.clone())
        .run(
            "agent",
            "diagnose fixture",
            &context,
            RunOptions {
                session_id: Some("failure".to_owned()),
                ..RunOptions::default()
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(
        failure.error,
        RunError::SessionFailed {
            operation: "append",
            ..
        }
    ));
    assert_eq!(context.load(Ordering::SeqCst), 1);
    assert_eq!(model.requests.lock().unwrap().len(), 2);
    assert_eq!(session.append_calls.load(Ordering::SeqCst), 1);
    assert_eq!(failure.new_items, *session.attempted_items.lock().unwrap());
    assert!(failure.new_items.iter().any(|item| matches!(
        item,
        RunItem::ToolResult { call_id, output, .. }
            if call_id == "effect-call" && output == &json!("CANARY_TOOL_OUTPUT")
    )));
}
