//! Authority contract for future attested-memory promotion.
//!
//! This module deliberately does not expose a write command.  Import/restore
//! owns reversible copies of familiar files; promotion is a separate authority
//! operation that will create archival records only after its caller implements
//! this contract's journal and reconciliation rules.
//!
//! A promotion writer must make each journal transition durable before doing
//! the next step:
//!
//! 1. write and sync the snapshot and attestation files;
//! 2. commit SQLite rows as `pending` and tie them to the claim ULID;
//! 3. write, sync, and atomically replace the TurboVec index;
//! 4. atomically publish the manifest, then make the SQLite rows visible.
//!
//! Readers must return only a row that is visible *and* named by a valid
//! manifest.  A restart therefore cannot surface an origin row which has no
//! durable vector or manifest.  Reconciliation either rolls a complete
//! transaction forward, discards a transaction that never committed metadata,
//! or requires manual recovery when the artifacts disagree.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Version of the durable promotion-journal and manifest contract.
pub const PROMOTION_CONTRACT_VERSION: u16 = 1;

/// An immutable promotion claim. `claim_id` is the canonical claims-log key,
/// not a UI or mobile-memory identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionClaim {
    pub contract_version: u16,
    pub claim_id: String,
    pub familiar_id: String,
    pub portable_reference: String,
    pub privacy: PrivacyClassification,
    pub verification: Verification,
    pub attestation: Attestation,
    pub supersedes: Option<String>,
    /// Optional projection into the existing mobile UUID DTO boundary.
    ///
    /// A missing projection means that the promotion is not exposed to that
    /// API yet. The ULID claim is never silently substituted into UUID fields.
    pub mobile_projection: Option<MobileProjection>,
}

impl PromotionClaim {
    pub fn validate(&self) -> Result<()> {
        if self.contract_version != PROMOTION_CONTRACT_VERSION {
            bail!("unsupported promotion contract version");
        }
        validate_ulid(&self.claim_id, "claim id")?;
        validate_identifier(&self.familiar_id, "familiar id")?;
        validate_portable_reference(&self.portable_reference)?;
        self.verification.validate()?;
        self.attestation.validate()?;
        if let Some(supersedes) = &self.supersedes {
            validate_ulid(supersedes, "superseded claim id")?;
            if supersedes == &self.claim_id {
                bail!("a promotion claim cannot supersede itself");
            }
        }
        if let Some(projection) = &self.mobile_projection {
            projection.validate()?;
        }
        Ok(())
    }
}

/// The explicit privacy classification used by promotion. It is never inferred
/// from a source filename, a familiar name, or a client request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrivacyClassification {
    Public,
    Private,
    Restricted,
}

/// Verification evidence recorded with a promotion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Verification {
    Verified {
        snapshot_sha256: String,
        evidence_sha256: String,
    },
    NeedsReview {
        reason: String,
    },
}

impl Verification {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Verified {
                snapshot_sha256,
                evidence_sha256,
            } => {
                validate_sha256(snapshot_sha256, "snapshot digest")?;
                validate_sha256(evidence_sha256, "evidence digest")
            }
            Self::NeedsReview { reason } => {
                if reason.trim().is_empty() || reason.len() > 512 {
                    bail!("verification review reason must contain at most 512 characters");
                }
                Ok(())
            }
        }
    }
}

/// Metadata that binds a claim to a durable, redacted attestation artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attestation {
    pub schema: String,
    pub artifact_sha256: String,
}

impl Attestation {
    fn validate(&self) -> Result<()> {
        if self.schema.trim().is_empty() || self.schema.len() > 128 {
            bail!("attestation schema must contain at most 128 characters");
        }
        validate_sha256(&self.artifact_sha256, "attestation digest")
    }
}

/// Compatibility projection for the already-versioned mobile memory DTOs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobileProjection {
    pub memory_id: String,
    pub supersedes_memory_id: Option<String>,
}

impl MobileProjection {
    fn validate(&self) -> Result<()> {
        let memory_id = Uuid::parse_str(&self.memory_id)
            .map_err(|_| anyhow::anyhow!("mobile memory id is not a UUID"))?;
        if let Some(supersedes) = &self.supersedes_memory_id {
            let supersedes = Uuid::parse_str(supersedes)
                .map_err(|_| anyhow::anyhow!("mobile supersedes id is not a UUID"))?;
            if supersedes == memory_id {
                bail!("mobile memory id cannot supersede itself");
            }
        }
        Ok(())
    }
}

/// Persisted phases of a promotion transaction. A writer appends the phase to
/// its journal only after the phase's durability barrier has completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionJournalState {
    Prepared,
    SnapshotDurable,
    MetadataPending,
    VectorsDurable,
    ManifestPublished,
    Visible,
    Aborted,
}

impl PromotionJournalState {
    /// Returns whether an append-only journal can advance from `self` to
    /// `next`. `Aborted` is terminal; a failed transaction gets a new claim.
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Prepared, Self::SnapshotDurable | Self::Aborted)
                | (Self::SnapshotDurable, Self::MetadataPending | Self::Aborted)
                | (Self::MetadataPending, Self::VectorsDurable | Self::Aborted)
                | (
                    Self::VectorsDurable,
                    Self::ManifestPublished | Self::Aborted
                )
                | (Self::ManifestPublished, Self::Visible | Self::Aborted)
        )
    }
}

/// Result required from restart reconciliation. The decision is deliberately
/// conservative: the implementation must verify every named artifact before
/// it can roll a transaction forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconciliationAction {
    DiscardUncommitted,
    RollForward,
    ManualRecovery,
}

/// Determines the only safe restart action from the durable journal state and
/// the independently observed artifacts.
pub fn reconcile_promotion(
    state: PromotionJournalState,
    snapshot_and_attestation_valid: bool,
    metadata_row_present: bool,
    vectors_valid: bool,
    manifest_valid: bool,
) -> ReconciliationAction {
    match state {
        PromotionJournalState::Prepared => ReconciliationAction::DiscardUncommitted,
        PromotionJournalState::SnapshotDurable => {
            if snapshot_and_attestation_valid
                && !metadata_row_present
                && !vectors_valid
                && !manifest_valid
            {
                ReconciliationAction::DiscardUncommitted
            } else {
                ReconciliationAction::ManualRecovery
            }
        }
        PromotionJournalState::MetadataPending | PromotionJournalState::VectorsDurable => {
            if snapshot_and_attestation_valid && metadata_row_present && vectors_valid {
                ReconciliationAction::RollForward
            } else {
                ReconciliationAction::ManualRecovery
            }
        }
        PromotionJournalState::ManifestPublished => {
            if snapshot_and_attestation_valid
                && metadata_row_present
                && vectors_valid
                && manifest_valid
            {
                ReconciliationAction::RollForward
            } else {
                ReconciliationAction::ManualRecovery
            }
        }
        PromotionJournalState::Visible => {
            if snapshot_and_attestation_valid
                && metadata_row_present
                && vectors_valid
                && manifest_valid
            {
                ReconciliationAction::RollForward
            } else {
                ReconciliationAction::ManualRecovery
            }
        }
        PromotionJournalState::Aborted => ReconciliationAction::DiscardUncommitted,
    }
}

/// Validates a reference that can be preserved across machines and runtimes.
pub fn validate_portable_reference(reference: &str) -> Result<()> {
    if reference.trim().is_empty() {
        bail!("portable reference must not be empty");
    }
    if reference.starts_with("agent:") {
        bail!("runtime-specific session keys are not portable");
    }
    if is_absolute_or_drive_qualified_on_any_supported_platform(reference) {
        bail!("absolute or drive-relative paths are not portable");
    }
    if reference.split(['/', '\\']).any(|segment| segment == "..") {
        bail!("parent traversal is not allowed in portable references");
    }
    if let Some(session) = reference.strip_prefix("session://") {
        let mut parts = session.split('/');
        if parts.next().is_none_or(str::is_empty)
            || parts.next().is_none_or(str::is_empty)
            || parts.next().is_none_or(str::is_empty)
            || parts.next().is_some()
        {
            bail!("session reference must use session://<familiar>/<date>/<slug>");
        }
    }
    Ok(())
}

fn is_absolute_or_drive_qualified_on_any_supported_platform(value: &str) -> bool {
    if value.starts_with('/') || value.starts_with('\\') {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn validate_ulid(value: &str, field: &str) -> Result<()> {
    if value.len() != 26
        || !matches!(value.as_bytes().first(), Some(b'0'..=b'7'))
        || !value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'A'..=b'H' | b'J'..=b'K' | b'M'..=b'N' | b'P'..=b'T' | b'V'..=b'Z'))
    {
        bail!("{field} must be a canonical uppercase ULID");
    }
    Ok(())
}

fn validate_identifier(value: &str, field: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("{field} must use only ASCII letters, digits, '-' or '_'");
    }
    Ok(())
}

fn validate_sha256(value: &str, field: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{field} must be a SHA-256 hex digest");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        reconcile_promotion, validate_portable_reference, Attestation, MobileProjection,
        PrivacyClassification, PromotionClaim, PromotionJournalState, ReconciliationAction,
        Verification, PROMOTION_CONTRACT_VERSION,
    };

    const CLAIM_ID: &str = "01J4K9R81M6D92TX6D4E7R31VZ";
    const SUPERSEDES_ID: &str = "01J4K9R81M6D92TX6D4E7R31W0";
    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn claim() -> PromotionClaim {
        PromotionClaim {
            contract_version: PROMOTION_CONTRACT_VERSION,
            claim_id: CLAIM_ID.to_owned(),
            familiar_id: "sage".to_owned(),
            portable_reference: "session://sage/2026-08-04/promoted-summary".to_owned(),
            privacy: PrivacyClassification::Private,
            verification: Verification::Verified {
                snapshot_sha256: DIGEST.to_owned(),
                evidence_sha256: DIGEST.to_owned(),
            },
            attestation: Attestation {
                schema: "coven.attestation.v1".to_owned(),
                artifact_sha256: DIGEST.to_owned(),
            },
            supersedes: Some(SUPERSEDES_ID.to_owned()),
            mobile_projection: Some(MobileProjection {
                memory_id: "00000000-0000-0000-0000-000000000001".to_owned(),
                supersedes_memory_id: Some("00000000-0000-0000-0000-000000000002".to_owned()),
            }),
        }
    }

    #[test]
    fn portable_reference_accepts_relative_path() {
        validate_portable_reference("memory/example.md").expect("relative path should pass");
    }

    #[test]
    fn portable_reference_rejects_parent_traversal_with_both_separator_styles() {
        for reference in ["../example.md", r"..\example.md"] {
            let error = validate_portable_reference(reference).expect_err("parent traversal");
            assert!(error.to_string().contains("traversal"));
        }
    }

    #[test]
    fn portable_reference_rejects_non_relative_paths_on_every_supported_platform() {
        for reference in [
            "/absolute/example.md",
            r"C:\absolute\example.md",
            r"C:drive-relative\example.md",
        ] {
            let error = validate_portable_reference(reference).expect_err("non-relative path");
            assert!(error.to_string().contains("paths are not portable"));
        }
    }

    #[test]
    fn portable_reference_requires_a_complete_session_class() {
        validate_portable_reference("session://sage/2026-08-04/promoted-summary")
            .expect("complete session class");
        assert!(validate_portable_reference("session://sage/2026-08-04").is_err());
    }

    #[test]
    fn portable_reference_rejects_runtime_session_key() {
        let reference = ["agent", "example", "webchat", "direct", "123456789"].join(":");
        let error = validate_portable_reference(&reference).expect_err("runtime session key");
        assert!(error.to_string().contains("runtime-specific"));
    }

    #[test]
    fn claim_requires_canonical_claim_log_identity_and_mobile_uuid_projection() {
        claim().validate().expect("valid promotion claim");

        let mut invalid_ulid = claim();
        invalid_ulid.claim_id.make_ascii_lowercase();
        assert!(invalid_ulid.validate().is_err());

        let mut overflowing_ulid = claim();
        overflowing_ulid.claim_id.replace_range(..1, "Z");
        assert!(overflowing_ulid.validate().is_err());

        let mut invalid_mobile = claim();
        invalid_mobile.mobile_projection.as_mut().unwrap().memory_id = CLAIM_ID.to_owned();
        assert!(invalid_mobile.validate().is_err());
    }

    #[test]
    fn claim_rejects_self_supersession_and_incomplete_verification() {
        let mut self_superseding = claim();
        self_superseding.supersedes = Some(CLAIM_ID.to_owned());
        assert!(self_superseding.validate().is_err());

        let mut incomplete_verification = claim();
        incomplete_verification.verification = Verification::Verified {
            snapshot_sha256: "bad".to_owned(),
            evidence_sha256: DIGEST.to_owned(),
        };
        assert!(incomplete_verification.validate().is_err());
    }

    #[test]
    fn journal_is_append_only_and_rolls_forward_completed_artifacts() {
        assert!(PromotionJournalState::Prepared
            .can_transition_to(PromotionJournalState::SnapshotDurable));
        assert!(!PromotionJournalState::Prepared.can_transition_to(PromotionJournalState::Visible));
        assert!(!PromotionJournalState::Aborted.can_transition_to(PromotionJournalState::Prepared));

        assert_eq!(
            reconcile_promotion(
                PromotionJournalState::VectorsDurable,
                true,
                true,
                true,
                true
            ),
            ReconciliationAction::RollForward
        );
        assert_eq!(
            reconcile_promotion(
                PromotionJournalState::VectorsDurable,
                true,
                true,
                false,
                false
            ),
            ReconciliationAction::ManualRecovery
        );
        assert_eq!(
            reconcile_promotion(PromotionJournalState::Prepared, false, false, false, false),
            ReconciliationAction::DiscardUncommitted
        );
    }
}
