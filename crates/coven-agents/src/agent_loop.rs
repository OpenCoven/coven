//! One agent's behavior loop: model turns, tool proposals and dispatch, and
//! the policy checks that bound them.
//!
//! [`AgentLoop`] runs a single agent until it produces a final output,
//! requests a control transfer, or exhausts the turn budget. It does not look
//! up other agents, transfer control, load or append sessions, resolve
//! delegated context, or own invocation lifecycle. [`crate::Runner`] composes
//! those around it.

use std::{collections::BTreeSet, sync::OnceLock};

use crate::{
    Agent, AgentId, AgentRef, AuthorityScope, GuardrailStage, GuardrailVerdict, HandoffCall,
    HandoffDefinition, InvocationContext, InvocationEvent, InvocationEventKind,
    InvocationFailureKind, InvocationObserver, InvocationRequest, ModelAction, ModelRequest,
    NotStarted, ReviewOutcome, ReviewVerdict, RunError, RunEvent, RunFailureKind, RunItem,
    RunObserver, RunSignal, StopReason, ToolCall, ToolProposal,
};

/// Transcript and counters accumulated by a run in progress.
///
/// The loop records into this so the caller-facing wrapper can attach the
/// partial transcript to a failure instead of dropping it with the error.
#[derive(Debug, Default)]
pub(crate) struct RunProgress {
    pub(crate) items: Vec<RunItem>,
    pub(crate) turns: usize,
    pub(crate) handoffs: usize,
}

pub(crate) const fn invocation_failure_kind(kind: RunFailureKind) -> InvocationFailureKind {
    match kind {
        RunFailureKind::Configuration => InvocationFailureKind::Configuration,
        RunFailureKind::Session => InvocationFailureKind::Session,
        RunFailureKind::ProposalReview => InvocationFailureKind::ProposalReview,
        RunFailureKind::InputGuardrail => InvocationFailureKind::InputGuardrail,
        RunFailureKind::OutputGuardrail => InvocationFailureKind::OutputGuardrail,
        RunFailureKind::Model => InvocationFailureKind::Model,
        RunFailureKind::Tool => InvocationFailureKind::Tool,
        RunFailureKind::Handoff => InvocationFailureKind::ControlTransfer,
        RunFailureKind::InvalidResponse => InvocationFailureKind::InvalidResponse,
        RunFailureKind::Limit => InvocationFailureKind::Limit,
        RunFailureKind::Authority => InvocationFailureKind::Authority,
        RunFailureKind::Context => InvocationFailureKind::Context,
        RunFailureKind::Cancelled => InvocationFailureKind::Cancelled,
        RunFailureKind::TerminationUnknown => InvocationFailureKind::TerminationUnknown,
        RunFailureKind::RecoveryRequired => InvocationFailureKind::RecoveryRequired,
    }
}

/// Emits both observer streams for one admitted invocation request.
pub(crate) struct RunEmitter<'a> {
    observer: &'a dyn RunObserver,
    invocation_observer: &'a dyn InvocationObserver,
    request: &'a InvocationRequest,
    /// The kind of the run's one terminal failure, as emitted to observers.
    failure_kind: OnceLock<RunFailureKind>,
}

impl<'a> RunEmitter<'a> {
    pub(crate) fn new(
        observer: &'a dyn RunObserver,
        invocation_observer: &'a dyn InvocationObserver,
        request: &'a InvocationRequest,
    ) -> Self {
        Self {
            observer,
            invocation_observer,
            request,
            failure_kind: OnceLock::new(),
        }
    }

    /// The kind of the terminal failure this emitter reported, if any.
    pub(crate) fn failure_kind(&self) -> Option<RunFailureKind> {
        self.failure_kind.get().copied()
    }

    pub(crate) fn request(&self) -> &'a InvocationRequest {
        self.request
    }

    pub(crate) fn invocation(&self) -> &InvocationContext {
        &self.request.invocation
    }

    pub(crate) fn run_event(&self, event: RunEvent) {
        self.observer.on_event(&event);
    }

    pub(crate) fn invocation_event(&self, event: InvocationEventKind) {
        self.invocation_observer.on_event(&InvocationEvent::new(
            self.request.invocation.clone(),
            self.request.source.clone(),
            self.request.requested_target.clone(),
            event,
        ));
    }

    /// Emits the paired terminal failure events and returns `error`.
    /// `failed_at` is `None` when no registered agent began executing.
    pub(crate) fn fail(
        &self,
        agent: &AgentId,
        failed_at: Option<AgentRef>,
        kind: RunFailureKind,
        error: RunError,
    ) -> RunError {
        let _ = self.failure_kind.set(kind);
        self.run_event(RunEvent::RunFailed {
            invocation: self.request.invocation.clone(),
            agent: agent.clone(),
            kind,
        });
        self.invocation_event(InvocationEventKind::Failed {
            failed_at,
            kind: invocation_failure_kind(kind),
        });
        error
    }
}

/// Authority and result bounds that apply to every agent in one invocation.
/// The legacy entry points use the unbounded default.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AgentPolicy<'a> {
    pub(crate) authority: Option<&'a AuthorityScope>,
    pub(crate) max_output_bytes: Option<usize>,
}

impl AgentPolicy<'_> {
    fn permits_tool(&self, tool: &str) -> bool {
        self.authority
            .is_none_or(|authority| authority.permits_tool(tool))
    }
}

/// Items the model sees ahead of this run's own items, plus every tool call
/// id already used, so ids stay unique across the whole model transcript.
pub(crate) struct ModelHistory {
    prior: Vec<RunItem>,
    seen_call_ids: BTreeSet<String>,
}

impl ModelHistory {
    /// `prior` is loaded session history or resolved delegated context;
    /// `progress` is what this run has recorded so far.
    pub(crate) fn new(prior: Vec<RunItem>, progress: &RunProgress) -> Self {
        let seen_call_ids = prior
            .iter()
            .chain(&progress.items)
            .filter_map(|item| match item {
                RunItem::ToolCall { call, .. } => Some(call.id.clone()),
                RunItem::ToolResult { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .collect();
        Self {
            prior,
            seen_call_ids,
        }
    }

    fn model_items(&self, progress: &RunProgress) -> Vec<RunItem> {
        self.prior.iter().chain(&progress.items).cloned().collect()
    }
}

/// Why one agent's loop stopped without failing.
pub(crate) enum AgentStop {
    /// The agent produced output that passed the result contract and its
    /// output guardrails.
    Final(String),
    /// The agent asked to transfer control. The caller validates and
    /// performs the transfer.
    ControlTransfer(HandoffCall),
}

/// Runs one agent's model/tool loop under an invocation's policy, turn
/// budget, and stop signal. The budget is shared by every agent in the
/// invocation, so it is read from [`RunProgress::turns`] rather than reset
/// per agent.
pub(crate) struct AgentLoop<'a> {
    emitter: &'a RunEmitter<'a>,
    policy: AgentPolicy<'a>,
    max_turns: usize,
    signal: &'a RunSignal,
}

impl<'a> AgentLoop<'a> {
    pub(crate) fn new(
        emitter: &'a RunEmitter<'a>,
        policy: AgentPolicy<'a>,
        max_turns: usize,
        signal: &'a RunSignal,
    ) -> Self {
        Self {
            emitter,
            policy,
            max_turns,
            signal,
        }
    }

    /// Records that the runner observed a stop request.
    fn observe_stop<C: Sync>(&self, agent: &Agent<C>, reason: StopReason) {
        self.emitter.run_event(RunEvent::CancellationRequested {
            invocation: self.emitter.invocation().clone(),
            agent: agent.id.clone(),
            reason,
        });
        self.emitter
            .invocation_event(InvocationEventKind::CancellationRequested { reason });
    }

    /// Acknowledges a stop with no tool in flight.
    fn acknowledge_stop<C: Sync>(&self, agent: &Agent<C>, reason: StopReason) -> RunError {
        self.observe_stop(agent, reason);
        self.fail(
            agent,
            RunFailureKind::Cancelled,
            RunError::Cancelled {
                agent: agent.id.clone(),
                reason,
            },
        )
    }

    /// Stops cleanly if a stop has been requested. Called only where no tool
    /// is in flight.
    fn checkpoint<C: Sync>(&self, agent: &Agent<C>) -> Result<(), RunError> {
        match self.signal.stop_reason() {
            Some(reason) => Err(self.acknowledge_stop(agent, reason)),
            None => Ok(()),
        }
    }

    fn fail<C: Sync>(&self, agent: &Agent<C>, kind: RunFailureKind, error: RunError) -> RunError {
        self.emitter.fail(
            &agent.id,
            Some(agent.agent_ref().expect("runner validates agent refs")),
            kind,
            error,
        )
    }

    /// Checks an agent's input guardrails against the invocation's original
    /// input.
    ///
    /// This is the single ingress path shared by direct starts, delegated
    /// starts, and control transfers. The evaluated input is always the
    /// original input that started the invocation, so entering an agent
    /// through a transfer cannot grant access that direct entry would
    /// reject. A `GuardrailChecked` event is emitted per guardrail with the
    /// owning agent's identity, and a policy rejection or guardrail
    /// implementation error fails the run before the agent's next model turn
    /// or tool execution.
    pub(crate) async fn check_ingress<C: Sync>(
        &self,
        agent: &Agent<C>,
        input: &str,
        context: &C,
    ) -> Result<(), RunError> {
        for guardrail in &agent.input_guardrails {
            let verdict = guardrail.check(input, context).await.map_err(|source| {
                self.fail(
                    agent,
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
            self.emitter.run_event(RunEvent::GuardrailChecked {
                invocation: self.emitter.invocation().clone(),
                agent: agent.id.clone(),
                guardrail: guardrail.name().to_owned(),
                stage: GuardrailStage::Input,
                allowed,
            });
            if let GuardrailVerdict::Reject { reason } = verdict {
                return Err(self.fail(
                    agent,
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

    /// Applies the result contract, then the agent's output guardrails, to a
    /// candidate final output.
    async fn check_output<C: Sync>(
        &self,
        agent: &Agent<C>,
        output: &str,
        context: &C,
    ) -> Result<(), RunError> {
        if let Some(limit) = self.policy.max_output_bytes {
            if output.len() > limit {
                return Err(self.fail(
                    agent,
                    RunFailureKind::Limit,
                    RunError::OutputTooLarge {
                        limit,
                        bytes: output.len(),
                    },
                ));
            }
        }

        for guardrail in &agent.output_guardrails {
            let verdict = guardrail.check(output, context).await.map_err(|source| {
                self.fail(
                    agent,
                    RunFailureKind::OutputGuardrail,
                    RunError::GuardrailFailed {
                        agent: agent.id.clone(),
                        guardrail: guardrail.name().to_owned(),
                        stage: GuardrailStage::Output,
                        source,
                    },
                )
            })?;
            let allowed = verdict == GuardrailVerdict::Allow;
            self.emitter.run_event(RunEvent::GuardrailChecked {
                invocation: self.emitter.invocation().clone(),
                agent: agent.id.clone(),
                guardrail: guardrail.name().to_owned(),
                stage: GuardrailStage::Output,
                allowed,
            });
            if let GuardrailVerdict::Reject { reason } = verdict {
                return Err(self.fail(
                    agent,
                    RunFailureKind::OutputGuardrail,
                    RunError::GuardrailRejected {
                        agent: agent.id.clone(),
                        guardrail: guardrail.name().to_owned(),
                        stage: GuardrailStage::Output,
                        reason,
                    },
                ));
            }
        }

        Ok(())
    }

    /// Consults the agent's reviewers in registration order and returns the
    /// first non-permit verdict with the reviewer that produced it. `None`
    /// means every reviewer permitted the call, or none is registered.
    ///
    /// One `ProposalReviewed` event is emitted per reviewer consulted;
    /// reviewers after the deciding one are not called. A reviewer
    /// implementation error records an `Unavailable` review outcome, then
    /// fails the run.
    async fn review_tool_proposal<C: Sync>(
        &self,
        agent: &Agent<C>,
        call: &ToolCall,
        context: &C,
    ) -> Result<Option<(String, ReviewVerdict)>, RunError> {
        let proposal = ToolProposal {
            call_id: &call.id,
            tool: &call.name,
            arguments: &call.arguments,
        };
        for review in &agent.proposal_reviews {
            let verdict = match review.review(&proposal, context).await {
                Ok(verdict) => verdict,
                Err(source) => {
                    self.emitter.run_event(RunEvent::ProposalReviewed {
                        invocation: self.emitter.invocation().clone(),
                        agent: agent.id.clone(),
                        reviewer: review.name().to_owned(),
                        tool: call.name.clone(),
                        call_id: call.id.clone(),
                        verdict: ReviewOutcome::Unavailable,
                    });
                    return Err(self.fail(
                        agent,
                        RunFailureKind::ProposalReview,
                        RunError::ProposalReviewFailed {
                            agent: agent.id.clone(),
                            reviewer: review.name().to_owned(),
                            source,
                        },
                    ));
                }
            };
            self.emitter.run_event(RunEvent::ProposalReviewed {
                invocation: self.emitter.invocation().clone(),
                agent: agent.id.clone(),
                reviewer: review.name().to_owned(),
                tool: call.name.clone(),
                call_id: call.id.clone(),
                verdict: verdict.outcome(),
            });
            if verdict != ReviewVerdict::Permit {
                return Ok(Some((review.name().to_owned(), verdict)));
            }
        }
        Ok(None)
    }

    /// Runs `agent` until it produces a final output, requests a control
    /// transfer, or the invocation's turn budget runs out.
    ///
    /// Every item the agent produces is recorded in `progress` as it
    /// happens, so a failure keeps the partial transcript. A response with
    /// tool calls is screened for duplicate call ids before any tool runs;
    /// each call is then checked against the invocation's authority,
    /// reviewed, and dispatched in order. Review is per call: an earlier
    /// permitted call can execute before a later call fails.
    pub(crate) async fn run<C: Sync>(
        &self,
        agent: &Agent<C>,
        history: &mut ModelHistory,
        progress: &mut RunProgress,
        context: &C,
    ) -> Result<AgentStop, RunError> {
        while progress.turns < self.max_turns {
            self.checkpoint(agent)?;
            progress.turns += 1;
            let turn = progress.turns;

            self.emitter.run_event(RunEvent::ModelRequested {
                invocation: self.emitter.invocation().clone(),
                agent: agent.id.clone(),
                turn,
            });

            let model_request = ModelRequest {
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                instructions: agent.instructions.clone(),
                items: history.model_items(progress),
                tools: agent
                    .tools
                    .iter()
                    .filter(|tool| self.policy.permits_tool(&tool.definition.name))
                    .map(|tool| tool.definition.clone())
                    .collect(),
                handoffs: agent
                    .handoffs
                    .iter()
                    .map(|handoff| HandoffDefinition {
                        name: handoff.name.clone(),
                        description: handoff.description.clone(),
                        target: handoff.target.clone(),
                    })
                    .collect(),
            };
            let response = agent
                .model
                .generate_with_signal(model_request, context, self.signal)
                .await;
            // A response that arrives after a stop is discarded: none of its
            // actions run, whether or not the model call succeeded.
            if let Some(reason) = self.signal.stop_reason() {
                return Err(self.acknowledge_stop(agent, reason));
            }
            let response = response.map_err(|source| {
                self.fail(
                    agent,
                    RunFailureKind::Model,
                    RunError::ModelFailed {
                        agent: agent.id.clone(),
                        source,
                    },
                )
            })?;

            if let Some(message) = &response.assistant_message {
                progress.items.push(RunItem::AssistantMessage {
                    agent: agent.id.clone(),
                    content: message.clone(),
                });
            }

            if response.actions.is_empty() {
                let output = response.assistant_message.ok_or_else(|| {
                    self.fail(
                        agent,
                        RunFailureKind::InvalidResponse,
                        RunError::InvalidModelResponse {
                            agent: agent.id.clone(),
                            reason: "a response without actions must contain an assistant message"
                                .to_owned(),
                        },
                    )
                })?;
                self.check_output(agent, &output, context).await?;
                return Ok(AgentStop::Final(output));
            }

            let handoff_actions = response
                .actions
                .iter()
                .filter(|action| matches!(action, ModelAction::Handoff(_)))
                .count();
            if handoff_actions > 0 {
                if response.actions.len() != 1 {
                    return Err(self.fail(
                        agent,
                        RunFailureKind::InvalidResponse,
                        RunError::InvalidModelResponse {
                            agent: agent.id.clone(),
                            reason: "a handoff cannot be combined with other actions".to_owned(),
                        },
                    ));
                }
                let Some(ModelAction::Handoff(call)) = response.actions.into_iter().next() else {
                    return Err(self.fail(
                        agent,
                        RunFailureKind::InvalidResponse,
                        RunError::InvalidModelResponse {
                            agent: agent.id.clone(),
                            reason: "a handoff must be the only model action".to_owned(),
                        },
                    ));
                };
                return Ok(AgentStop::ControlTransfer(call));
            }

            // Results correlate to calls by id, so reusing an id makes the
            // transcript ambiguous. Screen the whole batch before executing
            // any tool to avoid partial side effects from an invalid response.
            for action in &response.actions {
                let ModelAction::ToolCall(call) = action else {
                    continue;
                };
                if !history.seen_call_ids.insert(call.id.clone()) {
                    return Err(self.fail(
                        agent,
                        RunFailureKind::InvalidResponse,
                        RunError::DuplicateToolCallId {
                            agent: agent.id.clone(),
                            call_id: call.id.clone(),
                        },
                    ));
                }
            }

            for action in response.actions {
                let ModelAction::ToolCall(call) = action else {
                    return Err(self.fail(
                        agent,
                        RunFailureKind::InvalidResponse,
                        RunError::InvalidModelResponse {
                            agent: agent.id.clone(),
                            reason: "handoff action reached the tool execution path".to_owned(),
                        },
                    ));
                };
                self.checkpoint(agent)?;
                if !self.policy.permits_tool(&call.name) {
                    return Err(self.fail(
                        agent,
                        RunFailureKind::Authority,
                        RunError::UnauthorizedTool {
                            agent: agent.id.clone(),
                            tool: call.name.clone(),
                        },
                    ));
                }
                let tool = agent
                    .tools
                    .iter()
                    .find(|tool| tool.definition.name == call.name)
                    .ok_or_else(|| {
                        self.fail(
                            agent,
                            RunFailureKind::Tool,
                            RunError::UnknownTool {
                                agent: agent.id.clone(),
                                tool: call.name.clone(),
                            },
                        )
                    })?;
                // Review the resolved call before anything is dispatched. The
                // proposal stays in the transcript either way; only a permit
                // reaches the tool.
                let review = self.review_tool_proposal(agent, &call, context).await?;
                if review.is_none() {
                    // Last point at which a stop leaves nothing in flight.
                    self.checkpoint(agent)?;
                }
                progress.items.push(RunItem::ToolCall {
                    agent: agent.id.clone(),
                    call: call.clone(),
                });
                if let Some((reviewer, verdict)) = review {
                    progress.items.push(RunItem::ToolResult {
                        agent: agent.id.clone(),
                        call_id: call.id.clone(),
                        tool: call.name.clone(),
                        output: serde_json::json!({
                            "executed": false,
                            "review": {
                                "reviewer": reviewer,
                                "verdict": verdict.outcome().as_str(),
                                "reason": verdict.reason(),
                            },
                        }),
                    });
                    continue;
                }
                self.emitter.run_event(RunEvent::ToolStarted {
                    invocation: self.emitter.invocation().clone(),
                    agent: agent.id.clone(),
                    tool: call.name.clone(),
                    call_id: call.id.clone(),
                });
                let output = match tool
                    .tool
                    .execute_with_signal(call.arguments, context, self.signal)
                    .await
                {
                    Ok(output) => output,
                    Err(source) => {
                        // A descendant whose effect is unknown outranks every
                        // other reading of this failure, stop or no stop.
                        if crate::error::chain_leaves_unknown_effect(source.as_ref()) {
                            if let Some(reason) = self.signal.stop_reason() {
                                self.observe_stop(agent, reason);
                            }
                            return Err(self.fail(
                                agent,
                                RunFailureKind::RecoveryRequired,
                                RunError::RecoveryRequired {
                                    agent: agent.id.clone(),
                                    call_id: call.id.clone(),
                                    source,
                                },
                            ));
                        }
                        return Err(match self.signal.stop_reason() {
                            // The tool proved it produced nothing.
                            Some(reason) if source.downcast_ref::<NotStarted>().is_some() => {
                                self.acknowledge_stop(agent, reason)
                            }
                            // A stop arrived while the tool ran and it failed
                            // without that proof: the effect is unknown.
                            Some(reason) => {
                                self.observe_stop(agent, reason);
                                self.fail(
                                    agent,
                                    RunFailureKind::TerminationUnknown,
                                    RunError::TerminationUnknown {
                                        agent: agent.id.clone(),
                                        call_id: call.id.clone(),
                                        reason,
                                        source,
                                    },
                                )
                            }
                            None => self.fail(
                                agent,
                                RunFailureKind::Tool,
                                RunError::ToolFailed {
                                    agent: agent.id.clone(),
                                    tool: call.name.clone(),
                                    source,
                                },
                            ),
                        });
                    }
                };
                progress.items.push(RunItem::ToolResult {
                    agent: agent.id.clone(),
                    call_id: call.id.clone(),
                    tool: call.name.clone(),
                    output,
                });
                self.emitter.run_event(RunEvent::ToolCompleted {
                    invocation: self.emitter.invocation().clone(),
                    agent: agent.id.clone(),
                    tool: call.name,
                    call_id: call.id,
                });
            }
        }

        Err(self.fail(
            agent,
            RunFailureKind::Limit,
            RunError::MaxTurnsExceeded {
                limit: self.max_turns,
            },
        ))
    }
}
