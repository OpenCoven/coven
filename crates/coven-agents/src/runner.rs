use std::{
    collections::{btree_map::Entry, BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use crate::{
    agent_loop::{AgentLoop, AgentPolicy, AgentStop, ModelHistory, RunEmitter, RunProgress},
    Agent, AgentId, AgentRef, Clock, ConfigError, ContentDigest, ContextManifest, ContextResolver,
    DelegationDigests, HandoffCall, InvocationContext, InvocationEventKind, InvocationId,
    InvocationObserver, InvocationRequest, InvocationSource, NoopInvocationObserver, NoopObserver,
    PreparedDelegation, RunControls, RunError, RunEvent, RunFailure, RunFailureKind, RunItem,
    RunObserver, RunSignal, SessionStore, SystemClock,
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

/// A completed delegated child invocation.
#[derive(Debug, Clone, PartialEq)]
pub struct DelegationResult {
    pub digests: DelegationDigests,
    pub run: RunResult,
}

/// A delegated child invocation that failed after it was admitted. The run
/// failure carries the child invocation identity and partial transcript.
#[derive(Debug)]
pub struct DelegationFailure {
    pub digests: Box<DelegationDigests>,
    pub failure: Box<RunFailure>,
}

impl std::fmt::Display for DelegationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.failure, formatter)
    }
}

impl std::error::Error for DelegationFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.failure.as_ref())
    }
}

/// What a failed delegated child means for its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DelegationDisposition {
    /// The child failed or was refused, and nothing about its outcome is
    /// ambiguous beyond what its partial transcript shows.
    Failed,
    /// The child acknowledged a stop with no tool in flight.
    Cancelled,
    /// The child, or a descendant, left an effect whose outcome is unknown.
    /// The parent must reconcile it before any retry.
    RecoveryRequired,
}

impl DelegationFailure {
    pub fn disposition(&self) -> DelegationDisposition {
        match &self.failure.error {
            error if error.leaves_unknown_effect() => DelegationDisposition::RecoveryRequired,
            RunError::Cancelled { .. } => DelegationDisposition::Cancelled,
            _ => DelegationDisposition::Failed,
        }
    }
}

type ActiveChildren = Mutex<BTreeMap<InvocationId, usize>>;

/// One running delegated child, counted against its parent until dropped.
/// Dropping the run's future releases the slot too.
struct ChildSlot<'a> {
    active: &'a ActiveChildren,
    parent: InvocationId,
}

impl Drop for ChildSlot<'_> {
    fn drop(&mut self) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Entry::Occupied(mut entry) = active.entry(self.parent) {
            *entry.get_mut() -= 1;
            if *entry.get() == 0 {
                entry.remove();
            }
        }
    }
}

/// What bounds one run beyond its options: whether the request must name an
/// exact revision, the policy every agent runs under, and explicitly selected
/// context in place of session history. The legacy entry points use the
/// unbounded default.
struct RunScope<'a, C>
where
    C: Sync,
{
    require_revision: bool,
    policy: AgentPolicy<'a>,
    context: Option<(&'a ContextManifest, &'a dyn ContextResolver<C>)>,
    controls: RunControls,
}

impl<C> Default for RunScope<'_, C>
where
    C: Sync,
{
    fn default() -> Self {
        Self {
            require_revision: false,
            policy: AgentPolicy::default(),
            context: None,
            controls: RunControls::default(),
        }
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
    clock: Arc<dyn Clock>,
    child_limit: Option<usize>,
    active_children: ActiveChildren,
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

            let mut review_names = BTreeSet::new();
            for review in &agent.proposal_reviews {
                let name = review.name().to_owned();
                if !review_names.insert(name.clone()) {
                    return Err(ConfigError::DuplicateProposalReview {
                        agent: agent.id.clone(),
                        name,
                    });
                }
            }
        }

        Ok(Self {
            agents: registered,
            session: None,
            observer: Arc::new(NoopObserver),
            invocation_observer: Arc::new(NoopInvocationObserver),
            clock: Arc::new(SystemClock::default()),
            child_limit: None,
            active_children: Mutex::default(),
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

    /// Replaces the monotonic clock used for run deadlines.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Allows at most `limit` delegated children of the same parent
    /// invocation to run on this runner at once. A delegation beyond the
    /// limit is refused before its target runs. Unlimited by default.
    pub fn with_child_limit(mut self, limit: usize) -> Self {
        self.child_limit = Some(limit);
        self
    }

    fn acquire_child_slot(
        &self,
        parent: Option<InvocationId>,
    ) -> Result<Option<ChildSlot<'_>>, RunError> {
        let (Some(limit), Some(parent)) = (self.child_limit, parent) else {
            return Ok(None);
        };
        let mut active = self
            .active_children
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let running = active.get(&parent).copied().unwrap_or(0);
        if running >= limit {
            return Err(RunError::ChildLimitExceeded { parent, limit });
        }
        active.insert(parent, running + 1);
        Ok(Some(ChildSlot {
            active: &self.active_children,
            parent,
        }))
    }

    fn registered_agent_ref(&self, id: &AgentId) -> Option<AgentRef> {
        self.agents
            .get(id)
            .map(|agent| agent.agent_ref().expect("runner validates agent refs"))
    }

    /// Fails at a registered agent and records it as the failure location.
    fn fail(
        &self,
        emitter: &RunEmitter<'_>,
        agent: &AgentId,
        kind: RunFailureKind,
        error: RunError,
    ) -> RunError {
        emitter.fail(agent, self.registered_agent_ref(agent), kind, error)
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

    /// Runs `starting_agent` to a final output.
    ///
    /// Every run emits exactly one `RunStarted` event followed by exactly one
    /// terminal `RunCompleted` or `RunFailed` event, including when the
    /// starting agent is unregistered, so observers can pair per-run state.
    ///
    /// A failure returns [`RunFailure`], which carries the transcript the run
    /// produced before it failed. A tool that fails mid-turn does not erase the
    /// user message, assistant message, and tool calls that preceded it, so
    /// those items are handed back rather than dropped. Execution or policy
    /// failure prevents session append. After final output passes policy, an
    /// append error also returns a failure; it does not prove the store wrote
    /// nothing or undo earlier tool effects.
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
            RunScope::default(),
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
        self.run_invocation_inner(
            request,
            input,
            context,
            options,
            RunScope {
                require_revision: true,
                ..RunScope::default()
            },
        )
        .await
    }

    /// Runs one validated invocation request under caller-supplied
    /// cancellation and deadline controls.
    ///
    /// The runner checks for a stop before each model turn, after each model
    /// response, and before each tool dispatch. A stop observed there ends
    /// the run with [`RunFailureKind::Cancelled`]; completed tool effects
    /// remain in the partial transcript. A tool that fails after a stop
    /// arrived while it ran ends the run with
    /// [`RunFailureKind::TerminationUnknown`] unless it returns
    /// [`crate::NotStarted`]. Once the final output passes policy, the run
    /// completes. Nothing is retried.
    pub async fn run_invocation_controlled(
        &self,
        request: InvocationRequest,
        input: impl Into<String>,
        context: &C,
        options: RunOptions,
        controls: RunControls,
    ) -> Result<RunResult, RunFailure> {
        self.run_invocation_inner(
            request,
            input,
            context,
            options,
            RunScope {
                require_revision: true,
                controls,
                ..RunScope::default()
            },
        )
        .await
    }

    /// Runs one prepared delegation as a child invocation of its parent.
    ///
    /// The child starts from the task input and the context the parent
    /// selected, never from a session or the parent's transcript. Every
    /// reference in the manifest is resolved through `resolver` and verified
    /// against its digest before the first model turn; a mismatch fails the
    /// invocation with [`RunFailureKind::Context`]. Tools outside the
    /// attenuated authority are neither advertised to the model nor
    /// executed; a request for one fails with [`RunFailureKind::Authority`].
    /// A final output larger than the result contract fails with
    /// [`RunFailureKind::Limit`] instead of completing.
    ///
    /// The prepared delegation is consumed, so one proof runs at most once.
    /// This is local execution only: it does not adopt, persist, retry, or
    /// place the child anywhere.
    pub async fn run_delegation(
        &self,
        delegation: PreparedDelegation,
        resolver: &dyn ContextResolver<C>,
        context: &C,
    ) -> Result<DelegationResult, DelegationFailure> {
        self.run_delegation_controlled(delegation, resolver, context, RunControls::default())
            .await
    }

    /// Runs one prepared delegation under controls derived from its parent,
    /// usually [`RunSignal::child_controls`].
    ///
    /// The child stops when the parent's token is cancelled or its own is.
    /// Its deadline is the earlier of `controls`' timeout and the spec's
    /// budget timeout. With [`Runner::with_child_limit`], a delegation beyond
    /// the parent's limit is refused with [`RunFailureKind::Limit`] before
    /// its target runs. [`DelegationFailure::disposition`] tells the parent
    /// whether the child failed, was cancelled, or requires recovery.
    pub async fn run_delegation_controlled(
        &self,
        delegation: PreparedDelegation,
        resolver: &dyn ContextResolver<C>,
        context: &C,
        controls: RunControls,
    ) -> Result<DelegationResult, DelegationFailure> {
        let (invocation, source, spec, digests) = delegation.into_parts();
        let requested_target = match spec.target.revision() {
            Some(_) => spec.target.clone(),
            None => self
                .registered_agent_ref(spec.target.id())
                .unwrap_or_else(|| spec.target.clone()),
        };
        let request = InvocationRequest::new(
            invocation,
            InvocationSource::Agent { agent: source },
            requested_target,
        );

        let _slot = match self.acquire_child_slot(request.invocation.parent_id) {
            Ok(slot) => slot,
            Err(error) => {
                return Err(DelegationFailure {
                    digests: Box::new(digests),
                    failure: Box::new(self.refuse_before_start(
                        &request,
                        spec.task.as_str(),
                        error,
                    )),
                });
            }
        };

        let timeout = match (controls.timeout(), spec.budget.timeout()) {
            (Some(parent), Some(own)) => Some(parent.min(own)),
            (parent, own) => parent.or(own),
        };
        let mut child_controls = RunControls::new();
        if let Some(token) = controls.cancellation() {
            child_controls = child_controls.with_cancellation(token.clone());
        }
        if let Some(timeout) = timeout {
            child_controls = child_controls.with_timeout(timeout);
        }

        let options = RunOptions {
            max_turns: spec.budget.max_turns(),
            max_handoffs: spec.budget.max_handoffs(),
            session_id: None,
        };
        let scope = RunScope {
            require_revision: false,
            policy: AgentPolicy {
                authority: Some(&spec.authority),
                max_output_bytes: Some(spec.result.max_output_bytes()),
            },
            context: Some((&spec.context, resolver)),
            controls: child_controls,
        };
        match self
            .run_invocation_inner(request, spec.task.as_str(), context, options, scope)
            .await
        {
            Ok(run) => Ok(DelegationResult { digests, run }),
            Err(failure) => Err(DelegationFailure {
                digests: Box::new(digests),
                failure: Box::new(failure),
            }),
        }
    }

    /// Refuses an admitted request before any agent runs, with the same
    /// paired lifecycle events as any other pre-execution failure.
    fn refuse_before_start(
        &self,
        request: &InvocationRequest,
        input: &str,
        error: RunError,
    ) -> RunFailure {
        let emitter = RunEmitter::new(
            self.observer.as_ref(),
            self.invocation_observer.as_ref(),
            request,
        );
        let target = request.requested_target.id().clone();
        emitter.run_event(RunEvent::RunStarted {
            invocation: request.invocation.clone(),
            starting_agent: target.clone(),
        });
        emitter.invocation_event(InvocationEventKind::Started {});
        let error = emitter.fail(&target, None, RunFailureKind::Limit, error);
        RunFailure {
            invocation: Box::new(request.invocation.clone()),
            error,
            new_items: vec![RunItem::UserMessage {
                content: input.to_owned(),
            }]
            .into_boxed_slice(),
            turns: 0,
            handoffs: 0,
        }
    }

    async fn run_invocation_inner(
        &self,
        request: InvocationRequest,
        input: impl Into<String>,
        context: &C,
        options: RunOptions,
        scope: RunScope<'_, C>,
    ) -> Result<RunResult, RunFailure> {
        let mut progress = RunProgress::default();
        let emitter = RunEmitter::new(
            self.observer.as_ref(),
            self.invocation_observer.as_ref(),
            &request,
        );

        let outcome = self
            .run_loop(
                &emitter,
                input.into(),
                context,
                options,
                scope,
                &mut progress,
            )
            .await;
        outcome.map_err(|error| {
            // Every failure inside the loop is emitted before it returns, and
            // its kind is a function of the error, so `RunFailure::kind`
            // always reports what observers saw.
            debug_assert_eq!(emitter.failure_kind(), Some(error.failure_kind()));
            RunFailure {
                invocation: Box::new(request.invocation.clone()),
                error,
                new_items: progress.items.into_boxed_slice(),
                turns: progress.turns,
                handoffs: progress.handoffs,
            }
        })
    }

    /// Resolves the selected context in manifest order and verifies each
    /// reference's digest before any of it reaches a model.
    async fn resolve_context(
        &self,
        emitter: &RunEmitter<'_>,
        agent: &AgentId,
        manifest: &ContextManifest,
        resolver: &dyn ContextResolver<C>,
        context: &C,
    ) -> Result<Vec<RunItem>, RunError> {
        let mut items = Vec::new();
        for reference in manifest.refs() {
            let resolved = resolver
                .resolve(reference, context)
                .await
                .map_err(|source| {
                    self.fail(
                        emitter,
                        agent,
                        RunFailureKind::Context,
                        RunError::ContextFailed {
                            reference: reference.id().to_owned(),
                            source,
                        },
                    )
                })?;
            let actual = ContentDigest::of_items(&resolved);
            if &actual != reference.digest() {
                return Err(self.fail(
                    emitter,
                    agent,
                    RunFailureKind::Context,
                    RunError::ContextDigestMismatch {
                        reference: reference.id().to_owned(),
                        expected: reference.digest().clone(),
                        actual,
                    },
                ));
            }
            items.extend(resolved);
        }
        Ok(items)
    }

    /// Validates and performs a legacy control transfer within one invocation:
    /// counts it against the transfer budget, resolves the named handoff on
    /// the current agent, and records the transfer. The target's ingress is
    /// checked by the caller before it runs.
    fn transfer_control(
        &self,
        emitter: &RunEmitter<'_>,
        current: &Agent<C>,
        call: &HandoffCall,
        max_handoffs: usize,
        progress: &mut RunProgress,
    ) -> Result<Arc<Agent<C>>, RunError> {
        progress.handoffs += 1;
        if progress.handoffs > max_handoffs {
            return Err(self.fail(
                emitter,
                &current.id,
                RunFailureKind::Limit,
                RunError::MaxHandoffsExceeded {
                    limit: max_handoffs,
                },
            ));
        }

        let handoff = current
            .handoffs
            .iter()
            .find(|handoff| handoff.name == call.name)
            .ok_or_else(|| {
                self.fail(
                    emitter,
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
                emitter,
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

        progress.items.push(RunItem::Handoff {
            from: current.id.clone(),
            to: target.id.clone(),
            name: handoff.name.clone(),
        });
        emitter.run_event(RunEvent::Handoff {
            invocation: emitter.invocation().clone(),
            from: current.id.clone(),
            to: target.id.clone(),
            name: handoff.name.clone(),
        });
        emitter.invocation_event(InvocationEventKind::ControlTransferred {
            from: current.agent_ref().expect("runner validates agent refs"),
            to: target.agent_ref().expect("runner validates agent refs"),
            name: handoff.name.clone(),
        });
        Ok(target)
    }

    async fn run_loop(
        &self,
        emitter: &RunEmitter<'_>,
        input: String,
        context: &C,
        options: RunOptions,
        scope: RunScope<'_, C>,
        progress: &mut RunProgress,
    ) -> Result<RunResult, RunError> {
        let request = emitter.request();
        let signal = RunSignal::new(scope.controls, Arc::clone(&self.clock));
        let starting_agent = request.requested_target.id().clone();
        emitter.run_event(RunEvent::RunStarted {
            invocation: request.invocation.clone(),
            starting_agent: starting_agent.clone(),
        });
        emitter.invocation_event(InvocationEventKind::Started {});

        progress.items.push(RunItem::UserMessage {
            content: input.clone(),
        });

        let mut current = self.agents.get(&starting_agent).cloned().ok_or_else(|| {
            emitter.fail(
                &starting_agent,
                None,
                RunFailureKind::Configuration,
                RunError::UnknownStartingAgent(starting_agent.clone()),
            )
        })?;

        let registered_target = current.agent_ref().expect("runner validates agent refs");
        if scope.require_revision
            && (request.requested_target.revision().is_none()
                || registered_target.revision().is_none())
        {
            return Err(emitter.fail(
                &starting_agent,
                None,
                RunFailureKind::Configuration,
                RunError::AgentRevisionRequired {
                    agent: starting_agent.clone(),
                },
            ));
        }
        if request.requested_target != registered_target {
            return Err(emitter.fail(
                &starting_agent,
                None,
                RunFailureKind::Configuration,
                RunError::AgentRevisionMismatch {
                    requested: Box::new(request.requested_target.clone()),
                    registered: Box::new(registered_target),
                },
            ));
        }

        let agent_loop = AgentLoop::new(emitter, scope.policy, options.max_turns, &signal);
        agent_loop.check_ingress(&current, &input, context).await?;

        let prior = match (&options.session_id, &self.session) {
            _ if scope.context.is_some() => {
                let (manifest, resolver) = scope.context.expect("checked above");
                self.resolve_context(emitter, &current.id, manifest, resolver, context)
                    .await?
            }
            (Some(session_id), Some(session)) => {
                session.load(session_id).await.map_err(|source| {
                    self.fail(
                        emitter,
                        &current.id,
                        RunFailureKind::Session,
                        RunError::SessionFailed {
                            operation: "load",
                            source,
                        },
                    )
                })?
            }
            (Some(_), None) => {
                return Err(self.fail(
                    emitter,
                    &current.id,
                    RunFailureKind::Session,
                    RunError::SessionUnavailable,
                ));
            }
            (None, _) => Vec::new(),
        };
        let mut history = ModelHistory::new(prior, progress);

        loop {
            match agent_loop
                .run(&current, &mut history, progress, context)
                .await?
            {
                AgentStop::Final(output) => {
                    if let (Some(session_id), Some(session)) = (&options.session_id, &self.session)
                    {
                        session
                            .append(session_id, &progress.items)
                            .await
                            .map_err(|source| {
                                self.fail(
                                    emitter,
                                    &current.id,
                                    RunFailureKind::Session,
                                    RunError::SessionFailed {
                                        operation: "append",
                                        source,
                                    },
                                )
                            })?;
                    }

                    emitter.run_event(RunEvent::RunCompleted {
                        invocation: request.invocation.clone(),
                        final_agent: current.id.clone(),
                        turns: progress.turns,
                        handoffs: progress.handoffs,
                    });
                    emitter.invocation_event(InvocationEventKind::Completed {
                        final_agent: current.agent_ref().expect("runner validates agent refs"),
                        turns: progress.turns,
                        control_transfers: progress.handoffs,
                    });
                    return Ok(RunResult {
                        invocation: request.invocation.clone(),
                        final_output: output,
                        final_agent: current.id.clone(),
                        new_items: std::mem::take(&mut progress.items),
                        turns: progress.turns,
                        handoffs: progress.handoffs,
                    });
                }
                AgentStop::ControlTransfer(call) => {
                    current = self.transfer_control(
                        emitter,
                        &current,
                        &call,
                        options.max_handoffs,
                        progress,
                    )?;
                    // Ingress parity: the target enforces the same input
                    // policy it would enforce as the starting agent, checked
                    // against the original input, before its first model turn
                    // or tool execution.
                    agent_loop.check_ingress(&current, &input, context).await?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AuthorityScope;

    fn assert_send<T: Send>(_: &T) {}

    struct NoContext;

    #[async_trait::async_trait]
    impl ContextResolver<()> for NoContext {
        async fn resolve(
            &self,
            _reference: &crate::ContextRef,
            _context: &(),
        ) -> Result<Vec<RunItem>, crate::BoxError> {
            Ok(Vec::new())
        }
    }

    /// Hosts spawn runs onto multi-threaded executors, so every public entry
    /// point must stay `Send` for a `Send + Sync` host context.
    #[test]
    fn public_run_futures_are_send() {
        let runner: Runner<()> = Runner {
            agents: BTreeMap::new(),
            session: None,
            observer: Arc::new(NoopObserver),
            invocation_observer: Arc::new(NoopInvocationObserver),
            clock: Arc::new(SystemClock::default()),
            child_limit: None,
            active_children: Mutex::default(),
        };
        let request = InvocationRequest::new(
            InvocationContext::root(InvocationId::new()),
            InvocationSource::Caller,
            AgentRef::new("agent").unwrap(),
        );
        let grant = crate::DelegationGrant {
            invocation: InvocationContext::root(InvocationId::new()),
            agent: AgentRef::new("parent").unwrap(),
            authority: AuthorityScope::new(crate::ProjectRef::new("p").unwrap()).with_delegation(),
            context: ContextManifest::default(),
            budget: crate::DelegationBudget::new(1, 0).unwrap(),
        };
        let prepared = PreparedDelegation::prepare(
            &grant,
            crate::DelegationSpec {
                target: AgentRef::new("agent").unwrap(),
                task: crate::TaskInput::new("task").unwrap(),
                context: ContextManifest::default(),
                authority: AuthorityScope::new(crate::ProjectRef::new("p").unwrap()),
                budget: crate::DelegationBudget::new(1, 0).unwrap(),
                result: crate::ResultContract::new(1).unwrap(),
            },
        )
        .unwrap();

        assert_send(&runner.run("agent", "input", &(), RunOptions::default()));
        assert_send(&runner.run_with_invocation(
            "agent",
            "input",
            &(),
            RunOptions::default(),
            InvocationContext::root(InvocationId::new()),
        ));
        assert_send(&runner.run_invocation(request, "input", &(), RunOptions::default()));
        assert_send(&runner.run_invocation_controlled(
            InvocationRequest::new(
                InvocationContext::root(InvocationId::new()),
                InvocationSource::Caller,
                AgentRef::new("agent").unwrap(),
            ),
            "input",
            &(),
            RunOptions::default(),
            RunControls::new().with_cancellation(crate::CancellationToken::new()),
        ));
        assert_send(&runner.run_delegation(prepared, &NoContext, &()));
    }
}
