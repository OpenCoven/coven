//! Privacy-minimized projection of a validated Automations Runtime Authority binding.
//!
//! The full `AutomationExecutionBinding` is Coven-owned authority evidence. External
//! execution consumers do not need its nonce, authorization proof references,
//! approval evidence, memory/context projection identifiers, or authentication
//! signature material merely to prove which principal/familiar/runtime envelope
//! Coven admitted. This module exposes the smallest immutable projection needed to
//! correlate an external execution with that Coven-owned decision.
//!
//! Construction takes an already validated companion-profile extension. It does
//! not perform authorization and must never be used as an alternate authority
//! path: dispatch remains gated by `validate_authority_profile` and the trusted
//! Runtime Authority adapter.

use serde::{Deserialize, Serialize};

use super::contract::authority::{
    AuthorityCapabilitySet, AuthorityOpaqueIdentifier, AuthorityPrivacy, AuthorityProducer,
    AuthorityProfile, AuthorityRisk, AuthorityRuntimeBinding, AuthorityTimestamp,
    AutomationAuthorityExtension, AutomationExecutionBinding,
};
use super::contract::types::{DigestValue, PrincipalId};

/// Bounded, immutable authority evidence suitable for an external execution
/// consumer or reconciliation surface.
///
/// Intentionally omitted from this projection:
///
/// - authorization proof references, request nonce, and consumption-store state;
/// - approval IDs/evidence and consumption event material;
/// - context and memory projection identifiers;
/// - Threads decision/protected-surface internals;
/// - authentication key/proof/signature material.
///
/// Consumers receive correlation and granted authority, never the material
/// from which authority could be replayed, broadened, or independently inferred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationAuthorityConsumerProjection {
    pub profile: AuthorityProfile,
    pub binding_id: AuthorityOpaqueIdentifier,
    pub binding_digest: DigestValue,
    pub principal_id: PrincipalId,
    pub familiar_root_id: AuthorityOpaqueIdentifier,
    pub identity_revision_id: AuthorityOpaqueIdentifier,
    pub project_id: AuthorityOpaqueIdentifier,
    pub workspace_id: AuthorityOpaqueIdentifier,
    pub granted_capabilities: AuthorityCapabilitySet,
    pub risk: AuthorityRisk,
    pub runtime: AuthorityRuntimeBinding,
    pub decision_timestamp: AuthorityTimestamp,
    pub producer: AuthorityProducer,
    pub privacy: AuthorityPrivacy,
}

impl AutomationAuthorityConsumerProjection {
    /// Project an already validated Runtime Authority extension for a bounded
    /// external consumer. This function deliberately performs no fallback from
    /// malformed or absent authority evidence; callers must only reach it after
    /// companion-profile validation has succeeded.
    #[must_use]
    pub fn from_validated(extension: &AutomationAuthorityExtension) -> Self {
        Self::from_binding(&extension.execution_binding)
    }

    #[must_use]
    fn from_binding(binding: &AutomationExecutionBinding) -> Self {
        Self {
            profile: binding.profile,
            binding_id: binding.binding_id.clone(),
            binding_digest: binding.integrity.clone(),
            principal_id: binding.principal.principal_id.clone(),
            familiar_root_id: binding.familiar.familiar_root_id.clone(),
            identity_revision_id: binding.familiar.identity_revision_id.clone(),
            project_id: binding.context_projection.project_id.clone(),
            workspace_id: binding.context_projection.workspace_id.clone(),
            granted_capabilities: binding.capabilities.granted.clone(),
            risk: binding.risk.clone(),
            runtime: binding.runtime.clone(),
            decision_timestamp: binding.decision_timestamp.clone(),
            producer: binding.producer.clone(),
            privacy: binding.privacy.clone(),
        }
    }
}
