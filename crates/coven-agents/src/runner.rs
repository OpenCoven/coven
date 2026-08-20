use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::{
    Agent, AgentId, ConfigError, GuardrailStage, GuardrailVerdict, HandoffDefinition, ModelAction,
    ModelRequest, NoopObserver, RunError, RunEvent, RunFailure, RunFailureKind, RunItem,
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

pub struct Runner<C>
where
    C: Sync,
{
    agents: BTreeMap<AgentId, Arc<Agent<C>>>,
    session: Option<Arc<dyn SessionStore>>,
    observer: Arc<dyn RunObserver>,
}

impl<C> Runner<C>
where
    C: Send + Sync + 'static,
{
    pub fn new(agents: impl IntoIterator<Item = Agent<C>>) -> Result<Self, ConfigError> {
        let mut registered = BTreeMap::new();

        for agent in agents {
            let id = agent.id.clone();
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

    fn fail(&self, agent: &AgentId, kind: RunFailureKind, error: RunError) -> RunError {
        self.observer.on_event(&RunEvent::RunFailed {
            agent: agent.clone(),
            kind,
        });
        error
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
    pub async fn run(
        &self,
        starting_agent: impl Into<AgentId>,
        input: impl Into<String>,
        context: &C,
        options: RunOptions,
    ) -> Result<RunResult, RunFailure> {
        let mut progress = RunProgress::default();

        self.run_loop(
            starting_agent.into(),
            input.into(),
            context,
            options,
            &mut progress,
        )
        .await
        .map_err(|error| RunFailure {
            error,
            new_items: progress.items,
            turns: progress.turns,
            handoffs: progress.handoffs,
        })
    }

    async fn run_loop(
        &self,
        starting_agent: AgentId,
        input: String,
        context: &C,
        options: RunOptions,
        progress: &mut RunProgress,
    ) -> Result<RunResult, RunError> {
        self.observer.on_event(&RunEvent::RunStarted {
            starting_agent: starting_agent.clone(),
        });

        progress.items.push(RunItem::UserMessage {
            content: input.clone(),
        });

        let mut current = self.agents.get(&starting_agent).cloned().ok_or_else(|| {
            self.fail(
                &starting_agent,
                RunFailureKind::Configuration,
                RunError::UnknownStartingAgent(starting_agent.clone()),
            )
        })?;

        for guardrail in &current.input_guardrails {
            let verdict = guardrail.check(&input, context).await.map_err(|source| {
                self.fail(
                    &current.id,
                    RunFailureKind::InputGuardrail,
                    RunError::GuardrailFailed {
                        agent: current.id.clone(),
                        guardrail: guardrail.name().to_owned(),
                        stage: GuardrailStage::Input,
                        source,
                    },
                )
            })?;
            let allowed = verdict == GuardrailVerdict::Allow;
            self.observer.on_event(&RunEvent::GuardrailChecked {
                agent: current.id.clone(),
                guardrail: guardrail.name().to_owned(),
                stage: GuardrailStage::Input,
                allowed,
            });
            if let GuardrailVerdict::Reject { reason } = verdict {
                return Err(self.fail(
                    &current.id,
                    RunFailureKind::InputGuardrail,
                    RunError::GuardrailRejected {
                        agent: current.id.clone(),
                        guardrail: guardrail.name().to_owned(),
                        stage: GuardrailStage::Input,
                        reason,
                    },
                ));
            }
        }

        let mut model_items = match (&options.session_id, &self.session) {
            (Some(session_id), Some(session)) => {
                let mut items = session.load(session_id).await.map_err(|source| {
                    self.fail(
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
                _ => None,
            })
            .collect();

        for turn in 1..=options.max_turns {
            progress.turns = turn;

            self.observer.on_event(&RunEvent::ModelRequested {
                agent: current.id.clone(),
                turn,
            });

            let request = ModelRequest {
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
                .generate(request, context)
                .await
                .map_err(|source| {
                    self.fail(
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
                        agent: current.id.clone(),
                        guardrail: guardrail.name().to_owned(),
                        stage: GuardrailStage::Output,
                        allowed,
                    });
                    if let GuardrailVerdict::Reject { reason } = verdict {
                        return Err(self.fail(
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
                    final_agent: current.id.clone(),
                    turns: turn,
                    handoffs: progress.handoffs,
                });
                return Ok(RunResult {
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
                        &current.id,
                        RunFailureKind::Limit,
                        RunError::MaxHandoffsExceeded {
                            limit: options.max_handoffs,
                        },
                    ));
                }

                let [ModelAction::Handoff(call)] = response.actions.as_slice() else {
                    return Err(self.fail(
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
                    from: current.id.clone(),
                    to: target.id.clone(),
                    name: handoff.name.clone(),
                });
                current = target;
                continue;
            }

            // A tool result correlates to its call by id, so a reused id makes
            // the transcript ambiguous. The whole response is screened before
            // any tool runs: rejecting mid-batch would leave the side effects
            // of the earlier calls behind with no way to correlate them.
            for action in &response.actions {
                let ModelAction::ToolCall(call) = action else {
                    continue;
                };
                if !seen_call_ids.insert(call.id.clone()) {
                    return Err(self.fail(
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
                    agent: current.id.clone(),
                    tool: call.name,
                    call_id: call.id,
                });
            }
        }

        Err(self.fail(
            &current.id,
            RunFailureKind::Limit,
            RunError::MaxTurnsExceeded {
                limit: options.max_turns,
            },
        ))
    }
}
