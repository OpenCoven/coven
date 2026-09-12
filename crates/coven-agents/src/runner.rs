use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::{
    Agent, AgentId, AgentRef, ConfigError, GuardrailStage, GuardrailVerdict, HandoffDefinition,
    InvocationContext, InvocationEvent, InvocationEventKind, InvocationFailureKind, InvocationId,
    InvocationObserver, InvocationRequest, InvocationSource, ModelAction, ModelRequest,
    NoopInvocationObserver, NoopObserver, RunError, RunEvent, RunFailure, RunFailureKind, RunItem,
    RunObserver, SessionStore,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    pub max_turns: usize,
    pub max_handoffs: usize,
    pub session_id: Option<String>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            max_turns: 16,
            max_handoffs: 8,
            session_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunResult {
    pub invocation: InvocationContext,
    pub final_output: String,
    pub final_agent: AgentId,
    pub new_items: Vec<RunItem>,
    pub turns: usize,
    pub handoffs: usize,
}

/// Transcript and counters accumulated by a run in progress.
///
/// The run loop records into this so the caller-facing wrapper can attach the
/// partial transcript to a failure instead of dropping it with the error.
#[derive(Debug, Default)]
struct RunProgress {
    items: Vec<RunItem>,
    turns: usize,
    handoffs: usize,
}

const fn invocation_failure_kind(kind: RunFailureKind) -> InvocationFailureKind {
    match kind {
        RunFailureKind::Configuration => InvocationFailureKind::Configuration,
        RunFailureKind::Session => InvocationFailureKind::Session,
        RunFailureKind::InputGuardrail => InvocationFailureKind::InputGuardrail,
        RunFailureKind::OutputGuardrail => InvocationFailureKind::OutputGuardrail,
        RunFailureKind::Model => InvocationFailureKind::Model,
        RunFailureKind::Tool => InvocationFailureKind::Tool,
        RunFailureKind::Handoff => InvocationFailureKind::ControlTransfer,
        RunFailureKind::InvalidResponse => InvocationFailureKind::InvalidResponse,
        RunFailureKind::Limit => InvocationFailureKind::Limit,
    }
}

pub struct Runner<C>
where
    C: Sync,
{
    agents: BTreeMap<AgentId, Arc<Agent<C>>>,
    session: Option<Arc<dyn SessionStore>>,
    observer: Arc<dyn RunObserver>,
    invocation_observer: Arc<dyn InvocationObserver>,
}

impl<C> Runner<C>
where
    C: Send + Sync + 'static,
{
    pub fn new(agents: impl IntoIterator<Item = Agent<C>>) -> Result<Self, ConfigError> {
        let mut registered = BTreeMap::new();

        for agent in agents {
            let id = agent.id.clone();
            agent
                .agent_ref()
                .map_err(|source| ConfigError::InvalidAgentRef {
                    agent: id.clone(),
                    source,
                })?;
            if registered.insert(id.clone(), Arc::new(agent)).is_some() {
                return Err(ConfigError::DuplicateAgent(id));
            }
        }

        if registered.is_empty() {
            return Err(ConfigError::NoAgents);
        }

        for agent in registered.values() {
            let mut tool_names = BTreeSet::new();
            for tool in &agent.tools {
                let name = tool.definition.name.clone();
                if !tool_names.insert(name.clone()) {
                    return Err(ConfigError::DuplicateTool {
                        agent: agent.id.clone(),
                        tool: name,
                    });
                }
            }

            let mut handoff_names = BTreeSet::new();
            for handoff in &agent.handoffs {
                if !handoff_names.insert(handoff.name.clone()) {
                    return Err(ConfigError::DuplicateHandoff {
                        agent: agent.id.clone(),
                        handoff: handoff.name.clone(),
                    });
                }
                if !registered.contains_key(&handoff.target) {
                    return Err(ConfigError::UnknownHandoffTarget {
                        agent: agent.id.clone(),
                        handoff: handoff.name.clone(),
                        target: handoff.target.clone(),
                    });
                }
            }
        }

        Ok(Self {
            agents: registered,
            session: None,
            observer: Arc::new(NoopObserver),
            invocation_observer: Arc::new(NoopInvocationObserver),
        })
    }

    pub fn with_session(mut self, session: Arc<dyn SessionStore>) -> Self {
        self.session = Some(session);
        self
    }

    pub fn with_observer(mut self, observer: Arc<dyn RunObserver>) -> Self {
        self.observer = observer;
        self
    }

    pub fn with_invocation_observer(mut self, observer: Arc<dyn InvocationObserver>) -> Self {
        self.invocation_observer = observer;
        self
    }

    fn registered_agent_ref(&self, id: &AgentId) -> Option<AgentRef> {
        self.agents
            .get(id)
            .map(|agent| agent.agent_ref().expect("runner validates agent refs"))
    }

    fn observe_invocation(&self, request: &InvocationRequest, event: InvocationEventKind) {
        self.invocation_observer.on_event(&InvocationEvent::new(
            request.invocation.clone(),
            request.source.clone(),
            request.requested_target.clone(),
            event,
        ));
    }

    fn fail(
        &self,
        request: &InvocationRequest,
        agent: &AgentId,
        kind: RunFailureKind,
        error: RunError,
    ) -> RunError {
        self.fail_with_location(
            request,
            agent,
            self.registered_agent_ref(agent),
            kind,
            error,
        )
    }

    fn fail_before_execution(
        &self,
        request: &InvocationRequest,
        agent: &AgentId,
        kind: RunFailureKind,
        error: RunError,
    ) -> RunError {
        self.fail_with_location(request, agent, None, kind, error)
    }

    fn fail_with_location(
        &self,
        request: &InvocationRequest,
        agent: &AgentId,
        failed_at: Option<AgentRef>,
        kind: RunFailureKind,
        error: RunError,
    ) -> RunError {
        self.observer.on_event(&RunEvent::RunFailed {
            invocation: request.invocation.clone(),
            agent: agent.clone(),
            kind,
        });
        self.observe_invocation(
            request,
            InvocationEventKind::Failed {
                failed_at,
                kind: invocation_failure_kind(kind),
            },
        );
        error
    }

    fn fail_legacy(
        &self,
        invocation: &InvocationContext,
        agent: &AgentId,
        kind: RunFailureKind,
        error: RunError,
    ) -> RunError {
        self.observer.on_event(&RunEvent::RunFailed {
            invocation: invocation.clone(),
            agent: agent.clone(),
            kind,
        });
        error
    }

    /// Checks an agent's input guardrails against the run's original user
    /// input.
    ///
    /// This is the single ingress path shared by direct starts and handoffs.
    /// The evaluated input is always the original user input that started the
    /// run — the same bounded string a direct start would check — so entering
    /// an agent through a handoff cannot grant access that direct entry would
    /// reject. A `GuardrailChecked` event is emitted per guardrail with the
    /// owning agent's identity, and a policy rejection or guardrail
    /// implementation error fails the run before the agent's next model turn
    /// or tool execution.
    async fn check_input_guardrails(
        &self,
        request: &InvocationRequest,
        agent: &Agent<C>,
        input: &str,
        context: &C,
    ) -> Result<(), RunError> {
        for guardrail in &agent.input_guardrails {
            let verdict = guardrail.check(input, context).await.map_err(|source| {
                self.fail(
                    request,
                    &agent.id,
                    RunFailureKind::InputGuardrail,
                    RunError::GuardrailFailed {
                        agent: agent.id.clone(),
                        guardrail: guardrail.name().to_owned(),
                        stage: GuardrailStage::Input,
                        source,
                    },
                )
            })?;
            let allowed = verdict == GuardrailVerdict::Allow;
            self.observer.on_event(&RunEvent::GuardrailChecked {
                invocation: request.invocation.clone(),
                agent: agent.id.clone(),
                guardrail: guardrail.name().to_owned(),
                stage: GuardrailStage::Input,
                allowed,
            });
            if let GuardrailVerdict::Reject { reason } = verdict {
                return Err(self.fail(
                    request,
                    &agent.id,
                    RunFailureKind::InputGuardrail,
                    RunError::GuardrailRejected {
                        agent: agent.id.clone(),
                        guardrail: guardrail.name().to_owned(),
                        stage: GuardrailStage::Input,
                        reason,
                    },
                ));
            }
        }

        Ok(())
    }

    /// Runs `starting_agent` to a final output.
    ///
    /// Every run emits exactly one `RunStarted` event followed by exactly one
    /// terminal `RunCompleted` or `RunFailed` event, including when the
    /// starting agent is unregistered, so observers can pair per-run state.
    ///
    /// A failure returns [`RunFailure`], which carries the transcript the run
    /// produced before it failed. A tool that fails mid-turn does not erase the
    /// user message, assistant message, and tool calls that preceded it, so
    /// those items are handed back rather than dropped. The runner never
    /// appends a failed run's items to the session store.
    ///
    /// Input guardrails are enforced at every agent boundary: the starting
    /// agent's before its first model turn, and each handoff target's against
    /// the same original user input before that target's first model turn, so
    /// a handoff cannot reach an agent that would have rejected the input as
    /// the starting agent.
    pub async fn run(
        &self,
        starting_agent: impl Into<AgentId>,
        input: impl Into<String>,
        context: &C,
        options: RunOptions,
    ) -> Result<RunResult, RunFailure> {
        self.run_with_invocation(
            starting_agent,
            input,
            context,
            options,
            InvocationContext::root(InvocationId::new()),
        )
        .await
    }

    /// Runs with caller-provided local invocation correlation.
    ///
    /// The supplied identity is telemetry and parent/child correlation only.
    /// It does not create durable adoption, idempotency, retry, authority, or
    /// executor ownership semantics.
    pub async fn run_with_invocation(
        &self,
        starting_agent: impl Into<AgentId>,
        input: impl Into<String>,
        context: &C,
        options: RunOptions,
        invocation: InvocationContext,
    ) -> Result<RunResult, RunFailure> {
        let starting_agent = starting_agent.into();
        let requested_target = match AgentRef::new(starting_agent.as_str()) {
            Ok(requested_target) => requested_target,
            Err(source) => {
                let mut progress = RunProgress::default();
                progress.items.push(RunItem::UserMessage {
                    content: input.into(),
                });
                self.observer.on_event(&RunEvent::RunStarted {
                    invocation: invocation.clone(),
                    starting_agent: starting_agent.clone(),
                });
                let error = self.fail_legacy(
                    &invocation,
                    &starting_agent,
                    RunFailureKind::Configuration,
                    RunError::InvalidAgentRef {
                        agent: starting_agent.clone(),
                        source,
                    },
                );
                return Err(RunFailure {
                    invocation: Box::new(invocation),
                    error,
                    new_items: progress.items.into_boxed_slice(),
                    turns: 0,
                    handoffs: 0,
                });
            }
        };
        let requested_target = self
            .registered_agent_ref(requested_target.id())
            .unwrap_or(requested_target);
        self.run_invocation_inner(
            InvocationRequest::new(invocation, InvocationSource::Caller, requested_target),
            input,
            context,
            options,
            false,
        )
        .await
    }

    /// Runs one validated invocation request and emits its versioned metadata
    /// event stream alongside the compatibility [`RunEvent`] stream.
    pub async fn run_invocation(
        &self,
        request: InvocationRequest,
        input: impl Into<String>,
        context: &C,
        options: RunOptions,
    ) -> Result<RunResult, RunFailure> {
        self.run_invocation_inner(request, input, context, options, true)
            .await
    }

    async fn run_invocation_inner(
        &self,
        request: InvocationRequest,
        input: impl Into<String>,
        context: &C,
        options: RunOptions,
        require_revision: bool,
    ) -> Result<RunResult, RunFailure> {
        let mut progress = RunProgress::default();

        self.run_loop(
            &request,
            input.into(),
            context,
            options,
            require_revision,
            &mut progress,
        )
        .await
        .map_err(|error| RunFailure {
            invocation: Box::new(request.invocation.clone()),
            error,
            new_items: progress.items.into_boxed_slice(),
            turns: progress.turns,
            handoffs: progress.handoffs,
        })
    }

    async fn run_loop(
        &self,
        request: &InvocationRequest,
        input: String,
        context: &C,
        options: RunOptions,
        require_revision: bool,
        progress: &mut RunProgress,
    ) -> Result<RunResult, RunError> {
        let starting_agent = request.requested_target.id().clone();
        self.observer.on_event(&RunEvent::RunStarted {
            invocation: request.invocation.clone(),
            starting_agent: starting_agent.clone(),
        });
        self.observe_invocation(request, InvocationEventKind::Started {});

        progress.items.push(RunItem::UserMessage {
            content: input.clone(),
        });

        let mut current = self.agents.get(&starting_agent).cloned().ok_or_else(|| {
            self.fail_before_execution(
                request,
                &starting_agent,
                RunFailureKind::Configuration,
                RunError::UnknownStartingAgent(starting_agent.clone()),
            )
        })?;

        let registered_target = current.agent_ref().expect("runner validates agent refs");
        if require_revision
            && (request.requested_target.revision().is_none()
                || registered_target.revision().is_none())
        {
            return Err(self.fail_before_execution(
                request,
                &starting_agent,
                RunFailureKind::Configuration,
                RunError::AgentRevisionRequired {
                    agent: starting_agent.clone(),
                },
            ));
        }
        if request.requested_target != registered_target {
            return Err(self.fail_before_execution(
                request,
                &starting_agent,
                RunFailureKind::Configuration,
                RunError::AgentRevisionMismatch {
                    requested: Box::new(request.requested_target.clone()),
                    registered: Box::new(registered_target),
                },
            ));
        }

        self.check_input_guardrails(request, &current, &input, context)
            .await?;

        let mut model_items = match (&options.session_id, &self.session) {
            (Some(session_id), Some(session)) => {
                let mut items = session.load(session_id).await.map_err(|source| {
                    self.fail(
                        request,
                        &current.id,
                        RunFailureKind::Session,
                        RunError::SessionFailed {
                            operation: "load",
                            source,
                        },
                    )
                })?;
                items.extend(progress.items.iter().cloned());
                items
            }
            (Some(_), None) => {
                return Err(self.fail(
                    request,
                    &current.id,
                    RunFailureKind::Session,
                    RunError::SessionUnavailable,
                ));
            }
            (None, _) => progress.items.clone(),
        };
        let mut seen_call_ids: BTreeSet<String> = model_items
            .iter()
            .filter_map(|item| match item {
                RunItem::ToolCall { call, .. } => Some(call.id.clone()),
                RunItem::ToolResult { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .collect();

        for turn in 1..=options.max_turns {
            progress.turns = turn;

            self.observer.on_event(&RunEvent::ModelRequested {
                invocation: request.invocation.clone(),
                agent: current.id.clone(),
                turn,
            });

            let model_request = ModelRequest {
                agent_id: current.id.clone(),
                agent_name: current.name.clone(),
                instructions: current.instructions.clone(),
                items: model_items.clone(),
                tools: current
                    .tools
                    .iter()
                    .map(|tool| tool.definition.clone())
                    .collect(),
                handoffs: current
                    .handoffs
                    .iter()
                    .map(|handoff| HandoffDefinition {
                        name: handoff.name.clone(),
                        description: handoff.description.clone(),
                        target: handoff.target.clone(),
                    })
                    .collect(),
            };
            let response = current
                .model
                .generate(model_request, context)
                .await
                .map_err(|source| {
                    self.fail(
                        request,
                        &current.id,
                        RunFailureKind::Model,
                        RunError::ModelFailed {
                            agent: current.id.clone(),
                            source,
                        },
                    )
                })?;

            if let Some(message) = &response.assistant_message {
                let item = RunItem::AssistantMessage {
                    agent: current.id.clone(),
                    content: message.clone(),
                };
                progress.items.push(item.clone());
                model_items.push(item);
            }

            if response.actions.is_empty() {
                let output = response.assistant_message.ok_or_else(|| {
                    self.fail(
                        request,
                        &current.id,
                        RunFailureKind::InvalidResponse,
                        RunError::InvalidModelResponse {
                            agent: current.id.clone(),
                            reason: "a response without actions must contain an assistant message"
                                .to_owned(),
                        },
                    )
                })?;

                for guardrail in &current.output_guardrails {
                    let verdict = guardrail.check(&output, context).await.map_err(|source| {
                        self.fail(
                            request,
                            &current.id,
                            RunFailureKind::OutputGuardrail,
                            RunError::GuardrailFailed {
                                agent: current.id.clone(),
                                guardrail: guardrail.name().to_owned(),
                                stage: GuardrailStage::Output,
                                source,
                            },
                        )
                    })?;
                    let allowed = verdict == GuardrailVerdict::Allow;
                    self.observer.on_event(&RunEvent::GuardrailChecked {
                        invocation: request.invocation.clone(),
                        agent: current.id.clone(),
                        guardrail: guardrail.name().to_owned(),
                        stage: GuardrailStage::Output,
                        allowed,
                    });
                    if let GuardrailVerdict::Reject { reason } = verdict {
                        return Err(self.fail(
                            request,
                            &current.id,
                            RunFailureKind::OutputGuardrail,
                            RunError::GuardrailRejected {
                                agent: current.id.clone(),
                                guardrail: guardrail.name().to_owned(),
                                stage: GuardrailStage::Output,
                                reason,
                            },
                        ));
                    }
                }

                if let (Some(session_id), Some(session)) = (&options.session_id, &self.session) {
                    session
                        .append(session_id, &progress.items)
                        .await
                        .map_err(|source| {
                            self.fail(
                                request,
                                &current.id,
                                RunFailureKind::Session,
                                RunError::SessionFailed {
                                    operation: "append",
                                    source,
                                },
                            )
                        })?;
                }

                self.observer.on_event(&RunEvent::RunCompleted {
                    invocation: request.invocation.clone(),
                    final_agent: current.id.clone(),
                    turns: turn,
                    handoffs: progress.handoffs,
                });
                self.observe_invocation(
                    request,
                    InvocationEventKind::Completed {
                        final_agent: current.agent_ref().expect("runner validates agent refs"),
                        turns: turn,
                        control_transfers: progress.handoffs,
                    },
                );
                return Ok(RunResult {
                    invocation: request.invocation.clone(),
                    final_output: output,
                    final_agent: current.id.clone(),
                    new_items: std::mem::take(&mut progress.items),
                    turns: turn,
                    handoffs: progress.handoffs,
                });
            }

            let handoff_actions = response
                .actions
                .iter()
                .filter(|action| matches!(action, ModelAction::Handoff(_)))
                .count();
            if handoff_actions > 0 {
                if response.actions.len() != 1 {
                    return Err(self.fail(
                        request,
                        &current.id,
                        RunFailureKind::InvalidResponse,
                        RunError::InvalidModelResponse {
                            agent: current.id.clone(),
                            reason: "a handoff cannot be combined with other actions".to_owned(),
                        },
                    ));
                }
                progress.handoffs += 1;
                if progress.handoffs > options.max_handoffs {
                    return Err(self.fail(
                        request,
                        &current.id,
                        RunFailureKind::Limit,
                        RunError::MaxHandoffsExceeded {
                            limit: options.max_handoffs,
                        },
                    ));
                }

                let [ModelAction::Handoff(call)] = response.actions.as_slice() else {
                    return Err(self.fail(
                        request,
                        &current.id,
                        RunFailureKind::InvalidResponse,
                        RunError::InvalidModelResponse {
                            agent: current.id.clone(),
                            reason: "a handoff must be the only model action".to_owned(),
                        },
                    ));
                };
                let handoff = current
                    .handoffs
                    .iter()
                    .find(|handoff| handoff.name == call.name)
                    .ok_or_else(|| {
                        self.fail(
                            request,
                            &current.id,
                            RunFailureKind::Handoff,
                            RunError::UnknownHandoff {
                                agent: current.id.clone(),
                                handoff: call.name.clone(),
                            },
                        )
                    })?;
                let target = self.agents.get(&handoff.target).cloned().ok_or_else(|| {
                    self.fail(
                        request,
                        &current.id,
                        RunFailureKind::Configuration,
                        RunError::InvalidConfiguration {
                            reason: format!(
                                "validated handoff `{}` targets unavailable agent `{}`",
                                handoff.name, handoff.target
                            ),
                        },
                    )
                })?;
                let item = RunItem::Handoff {
                    from: current.id.clone(),
                    to: target.id.clone(),
                    name: handoff.name.clone(),
                };
                progress.items.push(item.clone());
                model_items.push(item);
                self.observer.on_event(&RunEvent::Handoff {
                    invocation: request.invocation.clone(),
                    from: current.id.clone(),
                    to: target.id.clone(),
                    name: handoff.name.clone(),
                });
                self.observe_invocation(
                    request,
                    InvocationEventKind::ControlTransferred {
                        from: current.agent_ref().expect("runner validates agent refs"),
                        to: target.agent_ref().expect("runner validates agent refs"),
                        name: handoff.name.clone(),
                    },
                );
                current = target;
                // Ingress parity: the handoff target enforces the same input
                // policy it would enforce as the starting agent, checked
                // against the original user input, before its first model turn
                // or tool execution.
                self.check_input_guardrails(request, &current, &input, context)
                    .await?;
                continue;
            }

            // Results correlate to calls by id, so reusing an id makes the
            // transcript ambiguous. Screen the whole batch before executing
            // any tool to avoid partial side effects from an invalid response.
            for action in &response.actions {
                let ModelAction::ToolCall(call) = action else {
                    continue;
                };
                if !seen_call_ids.insert(call.id.clone()) {
                    return Err(self.fail(
                        request,
                        &current.id,
                        RunFailureKind::InvalidResponse,
                        RunError::DuplicateToolCallId {
                            agent: current.id.clone(),
                            call_id: call.id.clone(),
                        },
                    ));
                }
            }

            for action in response.actions {
                let ModelAction::ToolCall(call) = action else {
                    return Err(self.fail(
                        request,
                        &current.id,
                        RunFailureKind::InvalidResponse,
                        RunError::InvalidModelResponse {
                            agent: current.id.clone(),
                            reason: "handoff action reached the tool execution path".to_owned(),
                        },
                    ));
                };
                let tool = current
                    .tools
                    .iter()
                    .find(|tool| tool.definition.name == call.name)
                    .ok_or_else(|| {
                        self.fail(
                            request,
                            &current.id,
                            RunFailureKind::Tool,
                            RunError::UnknownTool {
                                agent: current.id.clone(),
                                tool: call.name.clone(),
                            },
                        )
                    })?;
                let call_item = RunItem::ToolCall {
                    agent: current.id.clone(),
                    call: call.clone(),
                };
                progress.items.push(call_item.clone());
                model_items.push(call_item);
                self.observer.on_event(&RunEvent::ToolStarted {
                    invocation: request.invocation.clone(),
                    agent: current.id.clone(),
                    tool: call.name.clone(),
                    call_id: call.id.clone(),
                });
                let output =
                    tool.tool
                        .execute(call.arguments, context)
                        .await
                        .map_err(|source| {
                            self.fail(
                                request,
                                &current.id,
                                RunFailureKind::Tool,
                                RunError::ToolFailed {
                                    agent: current.id.clone(),
                                    tool: call.name.clone(),
                                    source,
                                },
                            )
                        })?;
                let result_item = RunItem::ToolResult {
                    agent: current.id.clone(),
                    call_id: call.id.clone(),
                    tool: call.name.clone(),
                    output,
                };
                progress.items.push(result_item.clone());
                model_items.push(result_item);
                self.observer.on_event(&RunEvent::ToolCompleted {
                    invocation: request.invocation.clone(),
                    agent: current.id.clone(),
                    tool: call.name,
                    call_id: call.id,
                });
            }
        }

        Err(self.fail(
            request,
            &current.id,
            RunFailureKind::Limit,
            RunError::MaxTurnsExceeded {
                limit: options.max_turns,
            },
        ))
    }
}
