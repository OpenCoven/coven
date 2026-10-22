//! Bounded delegation: attenuated authority, selected context, and a child
//! invocation that never inherits its parent's transcript.

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
    Agent, AgentRef, AuthorityError, AuthorityScope, BoxError, ContentDigest, ContextManifest,
    ContextProvenance, ContextRef, ContextResolver, DelegationBudget, DelegationGrant,
    DelegationSpec, Handoff, HandoffCall, InMemorySession, InvocationContext, InvocationEvent,
    InvocationEventKind, InvocationFailureKind, InvocationId, InvocationObserver, InvocationSource,
    Model, ModelAction, ModelRequest, ModelResponse, PreparedDelegation, ProjectRef,
    ResultContract, RunError, RunEvent, RunFailureKind, RunItem, RunObserver, RunOptions, Runner,
    SessionStore, TaskInput, Tool, ToolCall, ToolDefinition,
};
use serde_json::{json, Value};

#[derive(Default)]
struct Host {
    effects: Mutex<Vec<String>>,
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
    fn new(responses: impl IntoIterator<Item = ModelResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl Model<Host> for ScriptedModel {
    async fn generate(
        &self,
        request: ModelRequest,
        _context: &Host,
    ) -> Result<ModelResponse, BoxError> {
        self.requests.lock().unwrap().push(request);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| Box::new(io::Error::other("no scripted response")) as BoxError)
    }
}

struct EffectTool(&'static str);

#[async_trait]
impl Tool<Host> for EffectTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.0, "synthetic effect", json!({ "type": "object" }))
    }

    async fn execute(&self, _arguments: Value, context: &Host) -> Result<Value, BoxError> {
        context.effects.lock().unwrap().push(self.0.to_owned());
        Ok(json!({ "ran": self.0 }))
    }
}

/// Resolves every reference to the same scripted items and records requests.
struct ScriptedResolver {
    items: Vec<RunItem>,
    requested: Mutex<Vec<String>>,
    calls: AtomicUsize,
}

impl ScriptedResolver {
    fn new(items: Vec<RunItem>) -> Self {
        Self {
            items,
            requested: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ContextResolver<Host> for ScriptedResolver {
    async fn resolve(
        &self,
        reference: &ContextRef,
        _context: &Host,
    ) -> Result<Vec<RunItem>, BoxError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requested
            .lock()
            .unwrap()
            .push(reference.id().to_owned());
        Ok(self.items.clone())
    }
}

fn project() -> ProjectRef {
    ProjectRef::new("proj-a").unwrap()
}

fn parent_ref() -> AgentRef {
    AgentRef::new("planner").unwrap()
}

fn worker_ref() -> AgentRef {
    AgentRef::new("worker").unwrap()
}

fn selected_items() -> Vec<RunItem> {
    vec![RunItem::AssistantMessage {
        agent: "planner".into(),
        content: "SELECTED_CONTEXT".to_owned(),
    }]
}

fn selected_ref() -> ContextRef {
    ContextRef::new(
        "plan-summary",
        ContentDigest::of_items(&selected_items()),
        ContextProvenance::Agent {
            agent: parent_ref(),
        },
    )
    .unwrap()
}

fn root_grant(authority: AuthorityScope, context: ContextManifest) -> DelegationGrant {
    DelegationGrant {
        invocation: InvocationContext::root(InvocationId::new()),
        agent: parent_ref(),
        authority,
        context,
        budget: DelegationBudget::new(4, 2).unwrap(),
    }
}

fn full_authority() -> AuthorityScope {
    AuthorityScope::new(project())
        .with_tool("read")
        .unwrap()
        .with_tool("write")
        .unwrap()
        .with_resource("repo:a")
        .unwrap()
        .with_delegation()
}

fn spec(authority: AuthorityScope, context: ContextManifest) -> DelegationSpec {
    DelegationSpec {
        target: worker_ref(),
        task: TaskInput::new("summarize the plan").unwrap(),
        context,
        authority,
        budget: DelegationBudget::new(2, 1).unwrap(),
        result: ResultContract::new(64).unwrap(),
    }
}

#[test]
fn delegation_spec_has_a_closed_wire_shape_of_names_and_digests_only() {
    let manifest = ContextManifest::new([selected_ref()]).unwrap();
    let spec = spec(
        AuthorityScope::new(project())
            .with_tool("read")
            .unwrap()
            .with_resource("repo:a")
            .unwrap(),
        manifest.clone(),
    );

    let encoded = serde_json::to_value(&spec).unwrap();
    assert_eq!(
        encoded,
        json!({
            "target": { "id": "worker" },
            "task": "summarize the plan",
            "context": [{
                "id": "plan-summary",
                "digest": selected_ref().digest().as_str(),
                "provenance": { "type": "agent", "agent": { "id": "planner" } },
            }],
            "authority": {
                "project": "proj-a",
                "tools": ["read"],
                "resources": ["repo:a"],
                "may_delegate": false,
            },
            "budget": { "max_turns": 2, "max_handoffs": 1 },
            "result": { "max_output_bytes": 64 },
        })
    );
    assert_eq!(
        serde_json::from_value::<DelegationSpec>(encoded.clone()).unwrap(),
        spec
    );
    assert_eq!(
        serde_json::from_value::<DelegationSpec>(encoded)
            .unwrap()
            .digest(),
        spec.digest()
    );

    // Validation applies on the wire, not only through constructors.
    let mut widened = serde_json::to_value(&spec).unwrap();
    widened["authority"]["credential"] = json!("tok_secret");
    assert!(serde_json::from_value::<DelegationSpec>(widened).is_err());
    let mut oversized = serde_json::to_value(&spec).unwrap();
    oversized["task"] = json!("");
    assert!(serde_json::from_value::<DelegationSpec>(oversized).is_err());
    let mut bad_digest = serde_json::to_value(&spec).unwrap();
    bad_digest["context"][0]["digest"] = json!("sha256:nothex");
    assert!(serde_json::from_value::<DelegationSpec>(bad_digest).is_err());
    let mut bad_budget = serde_json::to_value(&spec).unwrap();
    bad_budget["budget"]["max_turns"] = json!(0);
    assert!(serde_json::from_value::<DelegationSpec>(bad_budget).is_err());
}

#[test]
fn prepare_refuses_every_widening_before_dispatch() {
    let manifest = ContextManifest::new([selected_ref()]).unwrap();
    let grant = root_grant(full_authority(), manifest.clone());

    let cross_project = spec(
        AuthorityScope::new(ProjectRef::new("proj-b").unwrap()),
        ContextManifest::default(),
    );
    assert!(matches!(
        PreparedDelegation::prepare(&grant, cross_project).unwrap_err(),
        AuthorityError::CrossProject { .. }
    ));

    let unauthorized_tool = spec(
        AuthorityScope::new(project()).with_tool("deploy").unwrap(),
        ContextManifest::default(),
    );
    assert_eq!(
        PreparedDelegation::prepare(&grant, unauthorized_tool).unwrap_err(),
        AuthorityError::UnauthorizedTool {
            tool: "deploy".into()
        }
    );

    let unauthorized_resource = spec(
        AuthorityScope::new(project())
            .with_resource("repo:b")
            .unwrap(),
        ContextManifest::default(),
    );
    assert_eq!(
        PreparedDelegation::prepare(&grant, unauthorized_resource).unwrap_err(),
        AuthorityError::UnauthorizedResource {
            resource: "repo:b".into()
        }
    );

    let mut over_budget = spec(AuthorityScope::new(project()), ContextManifest::default());
    over_budget.budget = DelegationBudget::new(5, 0).unwrap();
    assert_eq!(
        PreparedDelegation::prepare(&grant, over_budget).unwrap_err(),
        AuthorityError::BudgetExceeded {
            field: "max_turns",
            parent: 4,
            requested: 5,
        }
    );

    // Same id, different digest: not the reference the parent holds.
    let forged = ContextRef::new(
        "plan-summary",
        ContentDigest::of_bytes(b"something else"),
        ContextProvenance::Agent {
            agent: parent_ref(),
        },
    )
    .unwrap();
    let leaked = spec(
        AuthorityScope::new(project()),
        ContextManifest::new([forged]).unwrap(),
    );
    assert_eq!(
        PreparedDelegation::prepare(&grant, leaked).unwrap_err(),
        AuthorityError::ContextLeak {
            reference: "plan-summary".into()
        }
    );
    let unknown = ContextRef::new(
        "private-notes",
        ContentDigest::of_bytes(b"notes"),
        ContextProvenance::Caller,
    )
    .unwrap();
    let leaked = spec(
        AuthorityScope::new(project()),
        ContextManifest::new([unknown]).unwrap(),
    );
    assert_eq!(
        PreparedDelegation::prepare(&grant, leaked).unwrap_err(),
        AuthorityError::ContextLeak {
            reference: "private-notes".into()
        }
    );

    // The subset is accepted exactly as requested, never widened or narrowed.
    let narrowed = spec(
        AuthorityScope::new(project()).with_tool("read").unwrap(),
        manifest,
    );
    let prepared = PreparedDelegation::prepare(&grant, narrowed.clone()).unwrap();
    assert_eq!(prepared.spec(), &narrowed);
    assert_eq!(prepared.invocation().parent_id, Some(grant.invocation.id));
    assert_ne!(prepared.invocation().id, grant.invocation.id);
    assert_eq!(prepared.source(), &parent_ref());
    assert_eq!(prepared.digests().spec, narrowed.digest());
    assert_eq!(prepared.digests().authority, narrowed.authority.digest());
    assert_eq!(prepared.digests().context, narrowed.context.digest());
}

#[test]
fn redelegation_fails_closed_unless_the_parent_was_granted_it() {
    let grant = root_grant(full_authority(), ContextManifest::default());

    // A child that did not ask for delegation cannot delegate further.
    let leaf = PreparedDelegation::prepare(
        &grant,
        spec(
            AuthorityScope::new(project()).with_tool("read").unwrap(),
            ContextManifest::default(),
        ),
    )
    .unwrap();
    let child_grant = leaf.child_grant();
    assert_eq!(child_grant.invocation, *leaf.invocation());
    assert_eq!(child_grant.agent, worker_ref());
    assert!(!child_grant.authority.may_delegate());
    assert_eq!(
        PreparedDelegation::prepare(
            &child_grant,
            spec(AuthorityScope::new(project()), ContextManifest::default())
        )
        .unwrap_err(),
        AuthorityError::RedelegationDenied
    );

    // A parent without the right cannot confer it.
    let ungranted = root_grant(
        AuthorityScope::new(project()).with_tool("read").unwrap(),
        ContextManifest::default(),
    );
    assert_eq!(
        PreparedDelegation::prepare(
            &ungranted,
            spec(AuthorityScope::new(project()), ContextManifest::default())
        )
        .unwrap_err(),
        AuthorityError::RedelegationDenied
    );

    // Granted delegation passes down, and the grandchild is bounded by the
    // child's attenuated authority and budget, not the root's.
    let delegating_child = PreparedDelegation::prepare(
        &grant,
        spec(
            AuthorityScope::new(project())
                .with_tool("read")
                .unwrap()
                .with_delegation(),
            ContextManifest::default(),
        ),
    )
    .unwrap();
    let child_grant = delegating_child.child_grant();
    let mut grandchild = spec(
        AuthorityScope::new(project()).with_tool("write").unwrap(),
        ContextManifest::default(),
    );
    grandchild.budget = DelegationBudget::new(1, 0).unwrap();
    assert_eq!(
        PreparedDelegation::prepare(&child_grant, grandchild.clone()).unwrap_err(),
        AuthorityError::UnauthorizedTool {
            tool: "write".into()
        }
    );
    grandchild.authority = AuthorityScope::new(project()).with_tool("read").unwrap();
    grandchild.budget = DelegationBudget::new(3, 0).unwrap();
    assert_eq!(
        PreparedDelegation::prepare(&child_grant, grandchild.clone()).unwrap_err(),
        AuthorityError::BudgetExceeded {
            field: "max_turns",
            parent: 2,
            requested: 3,
        }
    );
    grandchild.budget = DelegationBudget::new(1, 0).unwrap();
    let prepared = PreparedDelegation::prepare(&child_grant, grandchild).unwrap();
    assert_eq!(
        prepared.invocation().parent_id,
        Some(delegating_child.invocation().id)
    );
}

#[tokio::test]
async fn delegated_child_sees_selected_context_and_task_but_never_the_parent_transcript() {
    let parent_model = ScriptedModel::new([ModelResponse::final_output("parent done")]);
    let child_model = ScriptedModel::new([ModelResponse::final_output("child done")]);
    let events = Arc::new(Events::default());
    let session = Arc::new(InMemorySession::default());
    let runner = Runner::new([
        Agent::new("planner", "Planner", "plan", parent_model.clone())
            .with_tool(Arc::new(EffectTool("read")))
            .with_tool(Arc::new(EffectTool("write"))),
        Agent::new("worker", "Worker", "work", child_model.clone())
            .with_tool(Arc::new(EffectTool("read")))
            .with_tool(Arc::new(EffectTool("write"))),
    ])
    .unwrap()
    .with_session(session.clone())
    .with_observer(events.clone())
    .with_invocation_observer(events.clone());
    let host = Host::default();

    // Build up a parent transcript in a session that the child must not see.
    session
        .append(
            "parent-session",
            &[RunItem::UserMessage {
                content: "PARENT_HISTORY_SECRET".to_owned(),
            }],
        )
        .await
        .unwrap();
    let parent = runner
        .run(
            "planner",
            "PARENT_INPUT_SECRET",
            &host,
            RunOptions {
                session_id: Some("parent-session".to_owned()),
                ..RunOptions::default()
            },
        )
        .await
        .unwrap();

    let manifest = ContextManifest::new([selected_ref()]).unwrap();
    let grant = DelegationGrant {
        invocation: parent.invocation.clone(),
        agent: parent_ref(),
        authority: full_authority(),
        context: manifest.clone(),
        budget: DelegationBudget::new(4, 2).unwrap(),
    };
    let prepared = PreparedDelegation::prepare(
        &grant,
        spec(
            AuthorityScope::new(project()).with_tool("read").unwrap(),
            manifest,
        ),
    )
    .unwrap();
    let child_invocation = prepared.invocation().clone();
    let expected_digests = prepared.digests().clone();
    let resolver = ScriptedResolver::new(selected_items());

    let result = runner
        .run_delegation(prepared, &resolver, &host)
        .await
        .unwrap();

    assert_eq!(result.digests, expected_digests);
    assert_eq!(result.run.invocation, child_invocation);
    assert_eq!(result.run.invocation.parent_id, Some(parent.invocation.id));
    assert_eq!(result.run.final_output, "child done");
    assert_eq!(
        result.run.new_items,
        vec![
            RunItem::UserMessage {
                content: "summarize the plan".to_owned()
            },
            RunItem::AssistantMessage {
                agent: "worker".into(),
                content: "child done".to_owned()
            },
        ],
        "resolved context is model input, not transcript the child produced"
    );
    assert_eq!(
        resolver.requested.lock().unwrap().as_slice(),
        ["plan-summary"]
    );

    let requests = child_model.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        request.items,
        vec![
            RunItem::AssistantMessage {
                agent: "planner".into(),
                content: "SELECTED_CONTEXT".to_owned()
            },
            RunItem::UserMessage {
                content: "summarize the plan".to_owned()
            },
        ]
    );
    let serialized = serde_json::to_string(request).unwrap();
    assert!(!serialized.contains("PARENT_HISTORY_SECRET"));
    assert!(!serialized.contains("PARENT_INPUT_SECRET"));
    assert_eq!(
        request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["read"],
        "only tools inside the attenuated authority are advertised"
    );

    // The child did not touch the parent's session.
    assert_eq!(
        session.items("parent-session").await.unwrap().len(),
        3,
        "history + parent input + parent output only"
    );

    let invocations = events.invocations.lock().unwrap().clone();
    let child_events: Vec<_> = invocations
        .iter()
        .filter(|event| event.invocation == child_invocation)
        .collect();
    assert_eq!(child_events.len(), 2);
    for event in &child_events {
        assert_eq!(
            serde_json::to_value(event).unwrap()["delegation"],
            serde_json::to_value(&expected_digests).unwrap()
        );
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(
            serde_json::from_value::<InvocationEvent>(value.clone()).unwrap(),
            **event
        );
        let mut invalid = value;
        invalid["delegation"]["authority"] = json!("not-a-digest");
        assert!(serde_json::from_value::<InvocationEvent>(invalid).is_err());
        let encoded = serde_json::to_string(event).unwrap();
        assert!(!encoded.contains("summarize the plan"));
        assert!(!encoded.contains("SELECTED_CONTEXT"));
    }
    for event in invocations
        .iter()
        .filter(|event| event.invocation != child_invocation)
    {
        assert!(serde_json::to_value(event)
            .unwrap()
            .get("delegation")
            .is_none());
    }
    assert_eq!(
        child_events[0].source,
        InvocationSource::Agent {
            agent: parent_ref()
        }
    );
    assert_eq!(child_events[0].requested_target, worker_ref());
    assert_eq!(child_events[0].event, InvocationEventKind::Started {});
    assert_eq!(
        child_events[1].event,
        InvocationEventKind::Completed {
            final_agent: worker_ref(),
            turns: 1,
            control_transfers: 0,
        }
    );
}

#[tokio::test]
async fn child_tool_call_outside_attenuated_authority_fails_closed_without_dispatch() {
    let child_model = ScriptedModel::new([ModelResponse::actions(vec![ModelAction::ToolCall(
        ToolCall::new("call-1", "write", json!({})),
    )])]);
    let events = Arc::new(Events::default());
    let runner = Runner::new([
        Agent::new("planner", "Planner", "plan", ScriptedModel::new([])),
        Agent::new("worker", "Worker", "work", child_model)
            .with_tool(Arc::new(EffectTool("read")))
            .with_tool(Arc::new(EffectTool("write"))),
    ])
    .unwrap()
    .with_observer(events.clone())
    .with_invocation_observer(events.clone());
    let host = Host::default();

    let grant = root_grant(full_authority(), ContextManifest::default());
    let prepared = PreparedDelegation::prepare(
        &grant,
        spec(
            AuthorityScope::new(project()).with_tool("read").unwrap(),
            ContextManifest::default(),
        ),
    )
    .unwrap();
    let child_invocation = prepared.invocation().clone();
    let resolver = ScriptedResolver::new(Vec::new());

    let failure = runner
        .run_delegation(prepared, &resolver, &host)
        .await
        .unwrap_err();

    assert!(matches!(
        failure.failure.error,
        RunError::UnauthorizedTool { ref agent, ref tool } if agent.as_str() == "worker" && tool == "write"
    ));
    assert_eq!(*failure.failure.invocation, child_invocation);
    assert!(
        host.effects.lock().unwrap().is_empty(),
        "nothing dispatched"
    );
    assert_eq!(
        failure.failure.new_items.as_ref(),
        [RunItem::UserMessage {
            content: "summarize the plan".to_owned()
        }],
        "the refused proposal is not recorded as a tool call"
    );

    let runs = events.runs.lock().unwrap().clone();
    assert!(!runs
        .iter()
        .any(|event| matches!(event, RunEvent::ToolStarted { .. })));
    assert!(matches!(
        runs.last().unwrap(),
        RunEvent::RunFailed {
            kind: RunFailureKind::Authority,
            ..
        }
    ));
    let invocations = events.invocations.lock().unwrap().clone();
    assert_eq!(invocations.len(), 2);
    for event in &invocations {
        assert_eq!(
            serde_json::to_value(event).unwrap()["delegation"],
            serde_json::to_value(&failure.digests).unwrap()
        );
    }
    assert_eq!(
        invocations[1].event,
        InvocationEventKind::Failed {
            failed_at: Some(worker_ref()),
            kind: InvocationFailureKind::Authority,
        }
    );
}

#[tokio::test]
async fn authority_binds_the_whole_child_invocation_across_control_transfer() {
    let worker_model = ScriptedModel::new([ModelResponse::actions(vec![ModelAction::Handoff(
        HandoffCall::new("escalate"),
    )])]);
    let helper_model = ScriptedModel::new([ModelResponse::actions(vec![ModelAction::ToolCall(
        ToolCall::new("call-1", "write", json!({})),
    )])]);
    let events = Arc::new(Events::default());
    let runner = Runner::new([
        Agent::new("planner", "Planner", "plan", ScriptedModel::new([])),
        Agent::new("worker", "Worker", "work", worker_model).with_handoff(Handoff::new(
            "escalate",
            "escalate to helper",
            "helper",
        )),
        Agent::new("helper", "Helper", "help", helper_model.clone())
            .with_tool(Arc::new(EffectTool("read")))
            .with_tool(Arc::new(EffectTool("write"))),
    ])
    .unwrap()
    .with_invocation_observer(events.clone());
    let host = Host::default();

    let grant = root_grant(full_authority(), ContextManifest::default());
    let prepared = PreparedDelegation::prepare(
        &grant,
        spec(
            AuthorityScope::new(project()).with_tool("read").unwrap(),
            ContextManifest::default(),
        ),
    )
    .unwrap();
    let resolver = ScriptedResolver::new(Vec::new());

    let failure = runner
        .run_delegation(prepared, &resolver, &host)
        .await
        .unwrap_err();

    assert_eq!(failure.failure.handoffs, 1);
    assert!(matches!(
        failure.failure.error,
        RunError::UnauthorizedTool { ref agent, ref tool } if agent.as_str() == "helper" && tool == "write"
    ));
    assert!(host.effects.lock().unwrap().is_empty());
    let advertised: Vec<String> = helper_model.requests()[0]
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect();
    assert_eq!(advertised, ["read"]);
    let events = events.invocations.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(events
        .iter()
        .any(|event| matches!(event.event, InvocationEventKind::ControlTransferred { .. })));
    for event in events.iter() {
        assert_eq!(
            serde_json::to_value(event).unwrap()["delegation"],
            serde_json::to_value(&failure.digests).unwrap()
        );
    }
}

#[tokio::test]
async fn context_digest_mismatch_fails_before_any_model_turn() {
    let child_model = ScriptedModel::new([ModelResponse::final_output("never")]);
    let events = Arc::new(Events::default());
    let runner = Runner::new([
        Agent::new("planner", "Planner", "plan", ScriptedModel::new([])),
        Agent::new("worker", "Worker", "work", child_model.clone()),
    ])
    .unwrap()
    .with_observer(events.clone())
    .with_invocation_observer(events.clone());
    let host = Host::default();

    let manifest = ContextManifest::new([selected_ref()]).unwrap();
    let grant = root_grant(full_authority(), manifest.clone());
    let prepared =
        PreparedDelegation::prepare(&grant, spec(AuthorityScope::new(project()), manifest))
            .unwrap();
    let resolver = ScriptedResolver::new(vec![RunItem::AssistantMessage {
        agent: "planner".into(),
        content: "SUBSTITUTED_CONTEXT".to_owned(),
    }]);

    let failure = runner
        .run_delegation(prepared, &resolver, &host)
        .await
        .unwrap_err();

    match &failure.failure.error {
        RunError::ContextDigestMismatch {
            reference,
            expected,
            actual,
        } => {
            assert_eq!(reference, "plan-summary");
            assert_eq!(expected, selected_ref().digest());
            assert_eq!(actual, &ContentDigest::of_items(&resolver.items));
        }
        other => panic!("unexpected error {other:?}"),
    }
    assert!(child_model.requests().is_empty(), "no model turn ran");
    assert_eq!(failure.failure.turns, 0);
    let runs = events.runs.lock().unwrap().clone();
    assert!(matches!(
        runs.last().unwrap(),
        RunEvent::RunFailed {
            kind: RunFailureKind::Context,
            ..
        }
    ));
    let invocations = events.invocations.lock().unwrap().clone();
    assert_eq!(invocations.len(), 2);
    for event in &invocations {
        assert_eq!(
            serde_json::to_value(event).unwrap()["delegation"],
            serde_json::to_value(&failure.digests).unwrap()
        );
    }
    assert_eq!(
        invocations[1].event,
        InvocationEventKind::Failed {
            failed_at: Some(worker_ref()),
            kind: InvocationFailureKind::Context,
        }
    );
}

#[tokio::test]
async fn resolver_error_fails_closed_with_context_kind() {
    struct FailingResolver;

    #[async_trait]
    impl ContextResolver<Host> for FailingResolver {
        async fn resolve(
            &self,
            _reference: &ContextRef,
            _context: &Host,
        ) -> Result<Vec<RunItem>, BoxError> {
            Err(Box::new(io::Error::other("store offline")))
        }
    }

    let child_model = ScriptedModel::new([ModelResponse::final_output("never")]);
    let runner = Runner::new([
        Agent::new("planner", "Planner", "plan", ScriptedModel::new([])),
        Agent::new("worker", "Worker", "work", child_model.clone()),
    ])
    .unwrap();
    let host = Host::default();
    let manifest = ContextManifest::new([selected_ref()]).unwrap();
    let grant = root_grant(full_authority(), manifest.clone());
    let prepared =
        PreparedDelegation::prepare(&grant, spec(AuthorityScope::new(project()), manifest))
            .unwrap();

    let failure = runner
        .run_delegation(prepared, &FailingResolver, &host)
        .await
        .unwrap_err();

    assert!(matches!(
        failure.failure.error,
        RunError::ContextFailed { ref reference, .. } if reference == "plan-summary"
    ));
    assert!(child_model.requests().is_empty());
}

#[tokio::test]
async fn oversized_child_output_fails_the_result_contract_instead_of_completing() {
    let child_model = ScriptedModel::new([ModelResponse::final_output("x".repeat(65))]);
    let events = Arc::new(Events::default());
    let runner = Runner::new([
        Agent::new("planner", "Planner", "plan", ScriptedModel::new([])),
        Agent::new("worker", "Worker", "work", child_model),
    ])
    .unwrap()
    .with_observer(events.clone())
    .with_invocation_observer(events.clone());
    let host = Host::default();

    let grant = root_grant(full_authority(), ContextManifest::default());
    let prepared = PreparedDelegation::prepare(
        &grant,
        spec(AuthorityScope::new(project()), ContextManifest::default()),
    )
    .unwrap();
    let resolver = ScriptedResolver::new(Vec::new());

    let failure = runner
        .run_delegation(prepared, &resolver, &host)
        .await
        .unwrap_err();

    assert!(matches!(
        failure.failure.error,
        RunError::OutputTooLarge {
            limit: 64,
            bytes: 65
        }
    ));
    let invocations = events.invocations.lock().unwrap().clone();
    assert!(!invocations
        .iter()
        .any(|event| matches!(event.event, InvocationEventKind::Completed { .. })));
    assert_eq!(
        invocations.last().unwrap().event,
        InvocationEventKind::Failed {
            failed_at: Some(worker_ref()),
            kind: InvocationFailureKind::Limit,
        }
    );
}

#[tokio::test]
async fn delegation_budget_bounds_the_child_run() {
    let child_model = ScriptedModel::new([
        ModelResponse::actions(vec![ModelAction::ToolCall(ToolCall::new(
            "call-1",
            "read",
            json!({}),
        ))]),
        ModelResponse::actions(vec![ModelAction::ToolCall(ToolCall::new(
            "call-2",
            "read",
            json!({}),
        ))]),
        ModelResponse::final_output("late"),
    ]);
    let runner = Runner::new([
        Agent::new("planner", "Planner", "plan", ScriptedModel::new([])),
        Agent::new("worker", "Worker", "work", child_model).with_tool(Arc::new(EffectTool("read"))),
    ])
    .unwrap();
    let host = Host::default();

    let grant = root_grant(full_authority(), ContextManifest::default());
    let prepared = PreparedDelegation::prepare(
        &grant,
        spec(
            AuthorityScope::new(project()).with_tool("read").unwrap(),
            ContextManifest::default(),
        ),
    )
    .unwrap();
    let resolver = ScriptedResolver::new(Vec::new());

    let failure = runner
        .run_delegation(prepared, &resolver, &host)
        .await
        .unwrap_err();

    assert!(matches!(
        failure.failure.error,
        RunError::MaxTurnsExceeded { limit: 2 }
    ));
    assert_eq!(failure.failure.turns, 2);
    assert_eq!(host.effects.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn delegation_target_revision_must_match_registration() {
    let runner = Runner::new([
        Agent::new("planner", "Planner", "plan", ScriptedModel::new([])),
        Agent::new(
            "worker",
            "Worker",
            "work",
            ScriptedModel::new([ModelResponse::final_output("ok")]),
        )
        .with_revision("rev-1".parse().unwrap()),
    ])
    .unwrap();
    let host = Host::default();
    let grant = root_grant(full_authority(), ContextManifest::default());
    let resolver = ScriptedResolver::new(Vec::new());

    let mut stale = spec(AuthorityScope::new(project()), ContextManifest::default());
    stale.target = AgentRef::with_revision("worker", "rev-0").unwrap();
    let prepared = PreparedDelegation::prepare(&grant, stale).unwrap();
    let failure = runner
        .run_delegation(prepared, &resolver, &host)
        .await
        .unwrap_err();
    assert!(matches!(
        failure.failure.error,
        RunError::AgentRevisionMismatch { .. }
    ));

    let prepared = PreparedDelegation::prepare(
        &grant,
        spec(AuthorityScope::new(project()), ContextManifest::default()),
    )
    .unwrap();
    let result = runner
        .run_delegation(prepared, &resolver, &host)
        .await
        .unwrap();
    assert_eq!(result.run.final_output, "ok");
}

#[tokio::test]
async fn legacy_entry_points_remain_unbounded_by_delegation_scope() {
    // A plain run still advertises every registered tool and keeps session
    // history: delegation bounds apply only to delegated invocations.
    let model = ScriptedModel::new([ModelResponse::final_output("legacy")]);
    let runner = Runner::new([Agent::new("planner", "Planner", "plan", model.clone())
        .with_tool(Arc::new(EffectTool("read")))
        .with_tool(Arc::new(EffectTool("write")))])
    .unwrap();
    let host = Host::default();

    let result = runner
        .run("planner", "hello", &host, RunOptions::default())
        .await
        .unwrap();

    assert_eq!(result.final_output, "legacy");
    let advertised: Vec<String> = model.requests()[0]
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect();
    assert_eq!(advertised, ["read", "write"]);
}
