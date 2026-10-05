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
    InvocationContext, InvocationEvent, InvocationEventKind, InvocationFailureKind,
    InvocationObserver, Model, ModelAction, ModelRequest, ModelResponse, OutputGuardrail,
    ProposalReview, ReviewVerdict, RunError, RunEvent, RunFailureKind, RunItem, RunObserver,
    RunOptions, Runner, SessionStore, Tool, ToolCall, ToolDefinition, ToolProposal,
};
use serde_json::{json, Value};

#[derive(Default)]
struct HostContext {
    effects: AtomicUsize,
    seen: Mutex<Vec<(String, usize)>>,
}

impl HostContext {
    fn record(&self, seam: impl Into<String>) {
        self.seen
            .lock()
            .unwrap()
            .push((seam.into(), std::ptr::from_ref(self) as usize));
    }
}

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
impl Model<HostContext> for ScriptedModel {
    async fn generate(
        &self,
        request: ModelRequest,
        context: &HostContext,
    ) -> Result<ModelResponse, BoxError> {
        context.record(format!("model/{}", request.agent_id));
        self.requests.lock().unwrap().push(request);
        Ok(self.responses.lock().unwrap().pop_front().unwrap())
    }
}

struct EffectTool(&'static str);

#[async_trait]
impl Tool<HostContext> for EffectTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.0, "Synthetic test effect", json!({ "type": "object" }))
    }

    async fn execute(&self, _arguments: Value, context: &HostContext) -> Result<Value, BoxError> {
        context.record(format!("tool/{}", self.0));
        context.effects.fetch_add(1, Ordering::SeqCst);
        Ok(json!("CANARY_TOOL_OUTPUT"))
    }
}

#[derive(Default)]
struct RecordingIngress(Mutex<Vec<String>>);

#[async_trait]
impl InputGuardrail<HostContext> for RecordingIngress {
    fn name(&self) -> &str {
        "record-ingress"
    }

    async fn check(
        &self,
        input: &str,
        context: &HostContext,
    ) -> Result<GuardrailVerdict, BoxError> {
        context.record("input");
        self.0.lock().unwrap().push(input.to_owned());
        Ok(GuardrailVerdict::Allow)
    }
}

#[derive(Default)]
struct RejectOutput(AtomicUsize);

#[async_trait]
impl OutputGuardrail<HostContext> for RejectOutput {
    fn name(&self) -> &str {
        "reject-output"
    }

    async fn check(
        &self,
        _output: &str,
        context: &HostContext,
    ) -> Result<GuardrailVerdict, BoxError> {
        context.record("output/reject");
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(GuardrailVerdict::reject("synthetic rejection"))
    }
}

#[derive(Default)]
struct RecordingOutput(Mutex<Vec<String>>);

#[async_trait]
impl OutputGuardrail<HostContext> for RecordingOutput {
    fn name(&self) -> &str {
        "record-output"
    }

    async fn check(
        &self,
        output: &str,
        context: &HostContext,
    ) -> Result<GuardrailVerdict, BoxError> {
        context.record("output");
        self.0.lock().unwrap().push(output.to_owned());
        Ok(GuardrailVerdict::Allow)
    }
}

struct ContextReview;

#[async_trait]
impl ProposalReview<HostContext> for ContextReview {
    fn name(&self) -> &str {
        "context-review"
    }

    async fn review(
        &self,
        _proposal: &ToolProposal<'_>,
        context: &HostContext,
    ) -> Result<ReviewVerdict, BoxError> {
        context.record("review");
        Ok(ReviewVerdict::Permit)
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
    let target_output = Arc::new(RecordingOutput::default());
    let source = Agent::new("source", "Source", "", source_model.clone())
        .with_tool(Arc::new(EffectTool("source-tool")))
        .with_output_guardrail(source_output.clone())
        .with_handoff(Handoff::new("transfer", "Transfer control", "target"));
    let target = Agent::new("target", "Target", "", target_model.clone())
        .with_tool(Arc::new(EffectTool("target-only-tool")))
        .with_input_guardrail(ingress.clone())
        .with_output_guardrail(target_output.clone())
        .with_proposal_review(Arc::new(ContextReview));
    let history = RunItem::AssistantMessage {
        agent: "previous-agent".into(),
        content: "CANARY_SESSION_HISTORY".to_owned(),
    };
    let session = Arc::new(InMemorySession::default());
    session
        .append("history", std::slice::from_ref(&history))
        .await
        .unwrap();
    let context = HostContext::default();
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
    assert_eq!(*target_output.0.lock().unwrap(), ["done"]);
    {
        let seen = context.seen.lock().unwrap();
        assert_eq!(
            seen.iter()
                .map(|(seam, _)| seam.as_str())
                .collect::<Vec<_>>(),
            [
                "model/source",
                "tool/source-tool",
                "model/source",
                "input",
                "model/target",
                "review",
                "tool/target-only-tool",
                "model/target",
                "output"
            ]
        );
        assert!(seen
            .iter()
            .all(|(_, address)| *address == std::ptr::from_ref(&context) as usize));
    }
    assert_eq!(context.effects.load(Ordering::SeqCst), 2);
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
    let context = HostContext::default();
    let events = Arc::new(Events::default());
    let invocation = InvocationContext::child(
        "88888888-8888-4888-8888-888888888888".parse().unwrap(),
        "99999999-9999-4999-8999-999999999999".parse().unwrap(),
    );
    let failure = Runner::new([agent])
        .unwrap()
        .with_session(session.clone())
        .with_observer(events.clone())
        .with_invocation_observer(events.clone())
        .run_with_invocation(
            "agent",
            "diagnose fixture",
            &context,
            RunOptions {
                session_id: Some("failure".to_owned()),
                ..RunOptions::default()
            },
            invocation.clone(),
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
    assert_eq!(context.effects.load(Ordering::SeqCst), 1);
    assert_eq!(model.requests.lock().unwrap().len(), 2);
    assert_eq!(session.append_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        failure.new_items.as_ref(),
        session.attempted_items.lock().unwrap().as_slice()
    );
    assert_eq!(*failure.invocation, invocation);
    assert_eq!((failure.turns, failure.handoffs), (2, 0));
    let runs = events.runs.lock().unwrap();
    assert!(runs.iter().all(|event| event.invocation() == &invocation));
    assert_eq!(
        runs.iter()
            .filter(|e| matches!(e, RunEvent::RunStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        runs.iter()
            .filter(|e| matches!(
                e,
                RunEvent::RunCompleted { .. } | RunEvent::RunFailed { .. }
            ))
            .count(),
        1
    );
    assert!(matches!(
        runs.last(),
        Some(RunEvent::RunFailed {
            kind: RunFailureKind::Session,
            ..
        })
    ));
    let canonical = events.invocations.lock().unwrap();
    assert_eq!(canonical.len(), 2);
    assert!(canonical.iter().all(|e| e.invocation == invocation));
    assert!(matches!(
        canonical[0].event,
        InvocationEventKind::Started {}
    ));
    assert!(matches!(&canonical[1].event, InvocationEventKind::Failed {
        failed_at: Some(agent), kind: InvocationFailureKind::Session
    } if agent.id().as_str() == "agent"));
    assert!(failure.new_items.iter().any(|item| matches!(
        item,
        RunItem::ToolResult { call_id, output, .. }
            if call_id == "effect-call" && output == &json!("CANARY_TOOL_OUTPUT")
    )));
}
