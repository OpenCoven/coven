//! Bounded delegation: a parent invocation hands a task to a child invocation
//! with explicitly selected context and attenuated authority.
//!
//! This is the structured replacement for treating a legacy handoff as
//! delegation. A [`DelegationSpec`] names the target, a bounded task input,
//! a [`ContextManifest`] of references with provenance, the
//! [`AuthorityScope`] the child may hold, a turn/handoff budget, and the
//! result contract the parent will accept. [`PreparedDelegation::prepare`]
//! proves the spec against the parent's [`DelegationGrant`] before anything is
//! dispatched: every check fails closed, and nothing is silently narrowed.
//!
//! Identity (`AgentRef`), capability (what an agent has registered),
//! authority (what this invocation may use) and trust (the host's grant) stay
//! distinct. A grant is a host assertion; the crate proves only that a child
//! never exceeds it. No type here carries credentials: authority is a set of
//! names and references, and context travels as digests that the runner
//! verifies against what the host resolves.
//!
//! Wall-clock deadlines, cancellation, durable adoption, retries and remote
//! executors remain outside this contract (see OpenCoven/coven#804).

use std::{collections::BTreeSet, fmt};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    invocation::validate_agent_ref_component, AgentRef, AgentRefError, BoxError, InvocationContext,
    InvocationId, RunItem,
};

/// Largest task input a delegation accepts, in bytes.
pub const MAX_TASK_INPUT_BYTES: usize = 64 * 1024;
/// Most context references one manifest may carry.
pub const MAX_CONTEXT_REFS: usize = 64;
/// Most tool or resource names one authority scope may carry.
pub const MAX_AUTHORITY_ENTRIES: usize = 256;

const DIGEST_PREFIX: &str = "sha256:";
const DIGEST_HEX_LEN: usize = 64;

/// Construction-time rejection of a malformed delegation component.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DelegationSpecError {
    #[error("task input must not be empty")]
    EmptyTask,
    #[error("task input is {bytes} bytes; the limit is {MAX_TASK_INPUT_BYTES}")]
    TaskTooLarge { bytes: usize },
    #[error("digest must be `sha256:` followed by 64 lowercase hexadecimal characters")]
    InvalidDigest,
    #[error("delegation {field} is not a valid name")]
    InvalidName {
        field: &'static str,
        #[source]
        source: AgentRefError,
    },
    #[error("context manifest has {count} references; the limit is {MAX_CONTEXT_REFS}")]
    TooManyContextRefs { count: usize },
    #[error("context manifest repeats reference `{0}`")]
    DuplicateContextRef(String),
    #[error("authority {field} has {count} entries; the limit is {MAX_AUTHORITY_ENTRIES}")]
    TooManyAuthorityEntries { field: &'static str, count: usize },
    #[error("budget must allow at least one model turn")]
    ZeroTurnBudget,
    #[error("result contract must accept at least one output byte")]
    ZeroResultBudget,
}

fn validate_name(value: &str, field: &'static str) -> Result<(), DelegationSpecError> {
    validate_agent_ref_component(value, field)
        .map_err(|source| DelegationSpecError::InvalidName { field, source })
}

/// `sha256:` plus 64 lowercase hexadecimal characters, the same syntax Psyche
/// digests use on Coven's adopted routes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContentDigest(String);

impl ContentDigest {
    pub fn parse(value: impl Into<String>) -> Result<Self, DelegationSpecError> {
        let value = value.into();
        let hex = value
            .strip_prefix(DIGEST_PREFIX)
            .ok_or(DelegationSpecError::InvalidDigest)?;
        if hex.len() != DIGEST_HEX_LEN
            || !hex
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(DelegationSpecError::InvalidDigest);
        }
        Ok(Self(value))
    }

    /// Digest of raw bytes.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        let hex: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Self(format!("{DIGEST_PREFIX}{hex}"))
    }

    /// Digest of the canonical JSON encoding of `items`. Both the host that
    /// issues a [`ContextRef`] and the runner that verifies it compute this
    /// over the same resolved items.
    pub fn of_items(items: &[RunItem]) -> Self {
        Self::of_bytes(&serde_json::to_vec(items).expect("run items serialize"))
    }

    fn of_value<T: Serialize>(value: &T) -> Self {
        Self::of_bytes(&serde_json::to_vec(value).expect("delegation contract serializes"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ContentDigest {
    type Error = DelegationSpecError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<ContentDigest> for String {
    fn from(value: ContentDigest) -> Self {
        value.0
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Opaque project identity. Coven compares it for equality only; a child may
/// never cross into a project its parent does not hold.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProjectRef(String);

impl ProjectRef {
    pub fn new(value: impl Into<String>) -> Result<Self, DelegationSpecError> {
        let value = value.into();
        validate_name(&value, "project")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProjectRef {
    type Error = DelegationSpecError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ProjectRef> for String {
    fn from(value: ProjectRef) -> Self {
        value.0
    }
}

impl fmt::Display for ProjectRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Non-empty, size-bounded task input for a child invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TaskInput(String);

impl TaskInput {
    pub fn new(value: impl Into<String>) -> Result<Self, DelegationSpecError> {
        let value = value.into();
        if value.is_empty() {
            return Err(DelegationSpecError::EmptyTask);
        }
        if value.len() > MAX_TASK_INPUT_BYTES {
            return Err(DelegationSpecError::TaskTooLarge { bytes: value.len() });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TaskInput {
    type Error = DelegationSpecError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<TaskInput> for String {
    fn from(value: TaskInput) -> Self {
        value.0
    }
}

/// Where a context reference came from. Recorded for provenance; the leak
/// check compares whole references, so provenance cannot be rewritten to
/// smuggle a reference the parent never held.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextProvenance {
    /// Supplied by the caller that started the root invocation.
    Caller,
    /// Produced by a specific invocation.
    Invocation { invocation: InvocationId },
    /// Produced by a specific agent.
    Agent { agent: AgentRef },
}

/// A reference to bounded context: an id, the digest of the items it resolves
/// to, and provenance. It carries no payload.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawContextRef")]
pub struct ContextRef {
    id: String,
    digest: ContentDigest,
    provenance: ContextProvenance,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawContextRef {
    id: String,
    digest: ContentDigest,
    provenance: ContextProvenance,
}

impl ContextRef {
    pub fn new(
        id: impl Into<String>,
        digest: ContentDigest,
        provenance: ContextProvenance,
    ) -> Result<Self, DelegationSpecError> {
        let id = id.into();
        validate_name(&id, "context reference id")?;
        Ok(Self {
            id,
            digest,
            provenance,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn digest(&self) -> &ContentDigest {
        &self.digest
    }

    pub fn provenance(&self) -> &ContextProvenance {
        &self.provenance
    }
}

impl TryFrom<RawContextRef> for ContextRef {
    type Error = DelegationSpecError;

    fn try_from(raw: RawContextRef) -> Result<Self, Self::Error> {
        Self::new(raw.id, raw.digest, raw.provenance)
    }
}

/// Ordered, bounded, duplicate-free selection of context references.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(try_from = "Vec<ContextRef>", into = "Vec<ContextRef>")]
pub struct ContextManifest {
    refs: Vec<ContextRef>,
}

impl ContextManifest {
    pub fn new(refs: impl IntoIterator<Item = ContextRef>) -> Result<Self, DelegationSpecError> {
        let refs: Vec<ContextRef> = refs.into_iter().collect();
        if refs.len() > MAX_CONTEXT_REFS {
            return Err(DelegationSpecError::TooManyContextRefs { count: refs.len() });
        }
        let mut ids = BTreeSet::new();
        for reference in &refs {
            if !ids.insert(reference.id.as_str()) {
                return Err(DelegationSpecError::DuplicateContextRef(
                    reference.id.clone(),
                ));
            }
        }
        Ok(Self { refs })
    }

    pub fn refs(&self) -> &[ContextRef] {
        &self.refs
    }

    pub fn is_empty(&self) -> bool {
        self.refs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.refs.len()
    }

    /// Digest over the manifest's references, not their payloads.
    pub fn digest(&self) -> ContentDigest {
        ContentDigest::of_value(&self.refs)
    }

    /// Every reference in `self` must appear in `parent` exactly, digest and
    /// provenance included.
    fn check_subset_of(&self, parent: &ContextManifest) -> Result<(), AuthorityError> {
        for reference in &self.refs {
            if !parent.refs.contains(reference) {
                return Err(AuthorityError::ContextLeak {
                    reference: reference.id.clone(),
                });
            }
        }
        Ok(())
    }
}

impl TryFrom<Vec<ContextRef>> for ContextManifest {
    type Error = DelegationSpecError;

    fn try_from(refs: Vec<ContextRef>) -> Result<Self, Self::Error> {
        Self::new(refs)
    }
}

impl From<ContextManifest> for Vec<ContextRef> {
    fn from(value: ContextManifest) -> Self {
        value.refs
    }
}

fn validate_authority_entries(
    entries: &BTreeSet<String>,
    field: &'static str,
) -> Result<(), DelegationSpecError> {
    if entries.len() > MAX_AUTHORITY_ENTRIES {
        return Err(DelegationSpecError::TooManyAuthorityEntries {
            field,
            count: entries.len(),
        });
    }
    for entry in entries {
        validate_name(entry, field)?;
    }
    Ok(())
}

/// What one invocation may use: a project, named tools, opaque resource
/// references, and whether it may delegate further. Authority names things;
/// it never carries the credentials that back them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawAuthorityScope")]
pub struct AuthorityScope {
    project: ProjectRef,
    tools: BTreeSet<String>,
    resources: BTreeSet<String>,
    may_delegate: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAuthorityScope {
    project: ProjectRef,
    tools: BTreeSet<String>,
    resources: BTreeSet<String>,
    may_delegate: bool,
}

impl AuthorityScope {
    /// Authority over `project` with no tools, no resources, and no right to
    /// delegate.
    pub fn new(project: ProjectRef) -> Self {
        Self {
            project,
            tools: BTreeSet::new(),
            resources: BTreeSet::new(),
            may_delegate: false,
        }
    }

    pub fn with_tool(mut self, tool: impl Into<String>) -> Result<Self, DelegationSpecError> {
        let tool = tool.into();
        validate_name(&tool, "tool")?;
        self.tools.insert(tool);
        validate_authority_entries(&self.tools, "tools")?;
        Ok(self)
    }

    pub fn with_resource(
        mut self,
        resource: impl Into<String>,
    ) -> Result<Self, DelegationSpecError> {
        let resource = resource.into();
        validate_name(&resource, "resource")?;
        self.resources.insert(resource);
        validate_authority_entries(&self.resources, "resources")?;
        Ok(self)
    }

    /// Permits the holder to delegate to children.
    pub fn with_delegation(mut self) -> Self {
        self.may_delegate = true;
        self
    }

    pub fn project(&self) -> &ProjectRef {
        &self.project
    }

    pub fn tools(&self) -> &BTreeSet<String> {
        &self.tools
    }

    pub fn resources(&self) -> &BTreeSet<String> {
        &self.resources
    }

    pub fn may_delegate(&self) -> bool {
        self.may_delegate
    }

    pub fn permits_tool(&self, tool: &str) -> bool {
        self.tools.contains(tool)
    }

    pub fn permits_resource(&self, resource: &str) -> bool {
        self.resources.contains(resource)
    }

    pub fn digest(&self) -> ContentDigest {
        ContentDigest::of_value(self)
    }

    /// Proves `requested ⊆ self`. Any widening fails; nothing is narrowed on
    /// the requester's behalf, so a caller cannot ask for more and receive
    /// less without noticing.
    pub fn attenuate(&self, requested: &AuthorityScope) -> Result<AuthorityScope, AuthorityError> {
        if requested.project != self.project {
            return Err(AuthorityError::CrossProject {
                parent: self.project.clone(),
                requested: requested.project.clone(),
            });
        }
        if let Some(tool) = requested.tools.difference(&self.tools).next() {
            return Err(AuthorityError::UnauthorizedTool { tool: tool.clone() });
        }
        if let Some(resource) = requested.resources.difference(&self.resources).next() {
            return Err(AuthorityError::UnauthorizedResource {
                resource: resource.clone(),
            });
        }
        if requested.may_delegate && !self.may_delegate {
            return Err(AuthorityError::RedelegationDenied);
        }
        Ok(requested.clone())
    }
}

impl TryFrom<RawAuthorityScope> for AuthorityScope {
    type Error = DelegationSpecError;

    fn try_from(raw: RawAuthorityScope) -> Result<Self, Self::Error> {
        validate_authority_entries(&raw.tools, "tools")?;
        validate_authority_entries(&raw.resources, "resources")?;
        Ok(Self {
            project: raw.project,
            tools: raw.tools,
            resources: raw.resources,
            may_delegate: raw.may_delegate,
        })
    }
}

/// Model-turn and control-transfer budget for one invocation. Wall-clock
/// deadlines and cancellation are a separate, later contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawDelegationBudget")]
pub struct DelegationBudget {
    max_turns: usize,
    max_handoffs: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDelegationBudget {
    max_turns: usize,
    max_handoffs: usize,
}

impl DelegationBudget {
    pub fn new(max_turns: usize, max_handoffs: usize) -> Result<Self, DelegationSpecError> {
        if max_turns == 0 {
            return Err(DelegationSpecError::ZeroTurnBudget);
        }
        Ok(Self {
            max_turns,
            max_handoffs,
        })
    }

    pub fn max_turns(&self) -> usize {
        self.max_turns
    }

    pub fn max_handoffs(&self) -> usize {
        self.max_handoffs
    }

    fn check_within(&self, parent: &DelegationBudget) -> Result<(), AuthorityError> {
        if self.max_turns > parent.max_turns {
            return Err(AuthorityError::BudgetExceeded {
                field: "max_turns",
                parent: parent.max_turns,
                requested: self.max_turns,
            });
        }
        if self.max_handoffs > parent.max_handoffs {
            return Err(AuthorityError::BudgetExceeded {
                field: "max_handoffs",
                parent: parent.max_handoffs,
                requested: self.max_handoffs,
            });
        }
        Ok(())
    }
}

impl TryFrom<RawDelegationBudget> for DelegationBudget {
    type Error = DelegationSpecError;

    fn try_from(raw: RawDelegationBudget) -> Result<Self, Self::Error> {
        Self::new(raw.max_turns, raw.max_handoffs)
    }
}

/// What the parent accepts back: a final text output no larger than the
/// stated bound. A child whose output exceeds it fails rather than completing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawResultContract")]
pub struct ResultContract {
    max_output_bytes: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResultContract {
    max_output_bytes: usize,
}

impl ResultContract {
    pub fn new(max_output_bytes: usize) -> Result<Self, DelegationSpecError> {
        if max_output_bytes == 0 {
            return Err(DelegationSpecError::ZeroResultBudget);
        }
        Ok(Self { max_output_bytes })
    }

    pub fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }
}

impl TryFrom<RawResultContract> for ResultContract {
    type Error = DelegationSpecError;

    fn try_from(raw: RawResultContract) -> Result<Self, Self::Error> {
        Self::new(raw.max_output_bytes)
    }
}

/// The child invocation contract a parent proposes. Every component is
/// validated at construction; the shape is closed on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegationSpec {
    pub target: AgentRef,
    pub task: TaskInput,
    pub context: ContextManifest,
    pub authority: AuthorityScope,
    pub budget: DelegationBudget,
    pub result: ResultContract,
}

impl DelegationSpec {
    /// Digest of the whole proposed contract, comparable to Psyche's
    /// `delegationDigest`.
    pub fn digest(&self) -> ContentDigest {
        ContentDigest::of_value(self)
    }
}

/// Why a delegation was refused before dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum AuthorityError {
    #[error("delegation would cross from project `{parent}` into `{requested}`")]
    CrossProject {
        parent: ProjectRef,
        requested: ProjectRef,
    },
    #[error("parent authority does not include tool `{tool}`")]
    UnauthorizedTool { tool: String },
    #[error("parent authority does not include resource `{resource}`")]
    UnauthorizedResource { resource: String },
    #[error("parent authority does not permit delegation")]
    RedelegationDenied,
    #[error("child {field} of {requested} exceeds the parent budget of {parent}")]
    BudgetExceeded {
        field: &'static str,
        parent: usize,
        requested: usize,
    },
    #[error("context reference `{reference}` is not held by the parent")]
    ContextLeak { reference: String },
}

/// What a delegating invocation holds. A root grant is a trusted host
/// assertion; the crate proves only that every descendant stays within it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationGrant {
    pub invocation: InvocationContext,
    pub agent: AgentRef,
    pub authority: AuthorityScope,
    pub context: ContextManifest,
    pub budget: DelegationBudget,
}

/// Digests that identify what a child was given, suitable for logs and traces
/// in place of payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegationDigests {
    pub spec: ContentDigest,
    pub authority: ContentDigest,
    pub context: ContentDigest,
}

/// A delegation whose spec has been proven against its parent's grant and
/// bound to a fresh child invocation identity. Consumed by exactly one run.
///
/// It is deliberately not `Clone`: a copy would let the same proof, and the
/// same child invocation id, run twice. Prepare again to get a new child.
///
/// ```compile_fail
/// fn assert_clone<T: Clone>() {}
/// assert_clone::<coven_agents::PreparedDelegation>();
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct PreparedDelegation {
    invocation: InvocationContext,
    source: AgentRef,
    spec: DelegationSpec,
    digests: DelegationDigests,
}

impl PreparedDelegation {
    /// Proves `spec` against `grant`, in this order: the parent may delegate
    /// at all; same project; tools ⊆; resources ⊆; no delegation widening;
    /// budget ≤; every context reference is one the parent holds. The first
    /// failure is returned and nothing is dispatched.
    pub fn prepare(grant: &DelegationGrant, spec: DelegationSpec) -> Result<Self, AuthorityError> {
        if !grant.authority.may_delegate() {
            return Err(AuthorityError::RedelegationDenied);
        }
        let authority = grant.authority.attenuate(&spec.authority)?;
        spec.budget.check_within(&grant.budget)?;
        spec.context.check_subset_of(&grant.context)?;

        let digests = DelegationDigests {
            spec: spec.digest(),
            authority: authority.digest(),
            context: spec.context.digest(),
        };
        Ok(Self {
            invocation: InvocationContext::child(InvocationId::new(), grant.invocation.id),
            source: grant.agent.clone(),
            spec: DelegationSpec { authority, ..spec },
            digests,
        })
    }

    pub fn invocation(&self) -> &InvocationContext {
        &self.invocation
    }

    pub fn source(&self) -> &AgentRef {
        &self.source
    }

    pub fn spec(&self) -> &DelegationSpec {
        &self.spec
    }

    pub fn authority(&self) -> &AuthorityScope {
        &self.spec.authority
    }

    pub fn digests(&self) -> &DelegationDigests {
        &self.digests
    }

    /// The grant the child would hold if it delegated further: its own
    /// invocation, its attenuated authority, its selected context and its
    /// budget. Without `may_delegate` in that authority, preparing against it
    /// fails with [`AuthorityError::RedelegationDenied`].
    pub fn child_grant(&self) -> DelegationGrant {
        DelegationGrant {
            invocation: self.invocation.clone(),
            agent: self.spec.target.clone(),
            authority: self.spec.authority.clone(),
            context: self.spec.context.clone(),
            budget: self.spec.budget,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        InvocationContext,
        AgentRef,
        DelegationSpec,
        DelegationDigests,
    ) {
        (self.invocation, self.source, self.spec, self.digests)
    }
}

/// Resolves a context reference to the items it names. The runner verifies
/// the resolved items against the reference digest before any model turn, so
/// a resolver cannot substitute content for what the parent selected.
#[async_trait]
pub trait ContextResolver<C>: Send + Sync
where
    C: Sync,
{
    async fn resolve(&self, reference: &ContextRef, context: &C) -> Result<Vec<RunItem>, BoxError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectRef {
        ProjectRef::new("proj-a").unwrap()
    }

    #[test]
    fn digest_syntax_matches_psyche_rules() {
        let digest = ContentDigest::of_bytes(b"hello");
        assert_eq!(digest.as_str().len(), DIGEST_PREFIX.len() + DIGEST_HEX_LEN);
        assert_eq!(ContentDigest::parse(digest.as_str()).unwrap(), digest);
        for invalid in [
            "",
            "sha256:",
            "sha1:0000",
            &format!("sha256:{}", "A".repeat(64)),
            &format!("sha256:{}", "0".repeat(63)),
            &format!("SHA256:{}", "0".repeat(64)),
        ] {
            assert!(
                ContentDigest::parse(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn attenuation_fails_closed_on_every_widening() {
        let parent = AuthorityScope::new(project())
            .with_tool("read")
            .unwrap()
            .with_resource("repo:a")
            .unwrap()
            .with_delegation();

        assert_eq!(
            parent
                .attenuate(&AuthorityScope::new(ProjectRef::new("proj-b").unwrap()))
                .unwrap_err(),
            AuthorityError::CrossProject {
                parent: project(),
                requested: ProjectRef::new("proj-b").unwrap(),
            }
        );
        assert_eq!(
            parent
                .attenuate(&AuthorityScope::new(project()).with_tool("write").unwrap())
                .unwrap_err(),
            AuthorityError::UnauthorizedTool {
                tool: "write".into()
            }
        );
        assert_eq!(
            parent
                .attenuate(
                    &AuthorityScope::new(project())
                        .with_resource("repo:b")
                        .unwrap()
                )
                .unwrap_err(),
            AuthorityError::UnauthorizedResource {
                resource: "repo:b".into()
            }
        );

        let leaf = AuthorityScope::new(project()).with_tool("read").unwrap();
        assert_eq!(
            leaf.attenuate(&AuthorityScope::new(project()).with_delegation())
                .unwrap_err(),
            AuthorityError::RedelegationDenied
        );

        let narrowed = parent
            .attenuate(&AuthorityScope::new(project()).with_tool("read").unwrap())
            .unwrap();
        assert!(narrowed.permits_tool("read"));
        assert!(!narrowed.permits_resource("repo:a"));
        assert!(!narrowed.may_delegate());
    }

    #[test]
    fn components_reject_malformed_and_oversized_values() {
        assert_eq!(
            TaskInput::new("").unwrap_err(),
            DelegationSpecError::EmptyTask
        );
        assert_eq!(
            TaskInput::new("x".repeat(MAX_TASK_INPUT_BYTES + 1)).unwrap_err(),
            DelegationSpecError::TaskTooLarge {
                bytes: MAX_TASK_INPUT_BYTES + 1
            }
        );
        assert!(TaskInput::new("x".repeat(MAX_TASK_INPUT_BYTES)).is_ok());
        assert!(ProjectRef::new("two words").is_err());
        assert!(AuthorityScope::new(project()).with_tool("").is_err());
        assert_eq!(
            DelegationBudget::new(0, 0).unwrap_err(),
            DelegationSpecError::ZeroTurnBudget
        );
        assert_eq!(
            ResultContract::new(0).unwrap_err(),
            DelegationSpecError::ZeroResultBudget
        );

        let digest = ContentDigest::of_bytes(b"ctx");
        let reference = |id: &str| ContextRef::new(id, digest.clone(), ContextProvenance::Caller);
        assert!(reference("has space").is_err());
        assert_eq!(
            ContextManifest::new([reference("a").unwrap(), reference("a").unwrap()]).unwrap_err(),
            DelegationSpecError::DuplicateContextRef("a".into())
        );
        let too_many = (0..=MAX_CONTEXT_REFS).map(|index| reference(&format!("r{index}")).unwrap());
        assert_eq!(
            ContextManifest::new(too_many).unwrap_err(),
            DelegationSpecError::TooManyContextRefs {
                count: MAX_CONTEXT_REFS + 1
            }
        );
    }
}
