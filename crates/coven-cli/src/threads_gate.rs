//! The coven-threads validator call site — Phase 2 of the authority-boundary
//! gate layer (`OpenCoven/coven-threads`, `specs/PHASE-0-DESIGN.md` §5, §6).
//!
//! The daemon already validates *who* (Ward Gate 1) and *where a write really
//! lands* (Gate 2 path materialization). What it did not validate is *what the
//! target file's authority state permits*: whether the protected surface has
//! drifted out from under its recorded authority since the principal last
//! blessed it. `coven-threads-core` fills that gap with a typed weave of
//! threads (authority relationships `surface → writer`) whose strands commit
//! to surface content.
//!
//! This module is the **only** bridge between the daemon and the gate crate:
//!
//! - it persists per-surface content baselines (`ward_manifest` table in
//!   `coven.sqlite3` — daemon-owned state, same single store as the audit log),
//! - it weaves the familiar's protected surfaces into a `Weave` on each
//!   request, fraying any thread whose surface drifted from baseline,
//! - it calls `coven_threads_core::validate_fail_closed` per protected target
//!   (fail-closed on unknown surface/writer/channel *and* on validator panic,
//!   RFC-0001 §5.4 Gate 4),
//! - it appends one `ward_audit` row per verdict (RFC-0001 §5.6; append-only
//!   enforced by triggers in the schema itself),
//! - on `DegradeToProposal` it stages the whole proposal, as a unit, at
//!   `~/.coven/pending/` for the principal — nothing is written to the
//!   protected surface.
//!
//! The gate runs *before* [`crate::ward::Ward::apply`]; the Ward's own
//! all-or-nothing apply remains the final materialized-diff boundary. Both
//! layers fail closed; neither can be skipped on the daemon's only
//! arbitrary-file write path into familiar homes (`POST /familiars/{id}/edits`).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::ambient_authority;
#[cfg(windows)]
use cap_std::fs::MetadataExt;
#[cfg(unix)]
use cap_std::fs::OpenOptionsExt;
use cap_std::fs::{Dir, OpenOptions};
use coven_threads_core as threads;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

use crate::ward;

/// The channels every protected-surface thread must hold under. `Mutation` is
/// the channel this endpoint exercises; `Forced` and `Serialization` are woven
/// now so compaction (WARD-C1–C6) and export (C7) lanes gate against the same
/// threads when they land.
const PROTECTED_CHANNELS: [threads::Channel; 3] = [
    threads::Channel::Forced,
    threads::Channel::Serialization,
    threads::Channel::Mutation,
];

/// Serialization-contract tag committed by every `SerializationMarker` strand
/// until the Phase 3 portability format defines the real contract hash.
const SERIALIZATION_CONTRACT: &[u8] = b"coven-threads:serialization-contract:v0.1.0";
const SERIALIZATION_FORMAT_VERSION: &str = "0.1.0";
const MAX_SURFACE_BYTES: u64 = crate::ward::WARD_FILE_CONTENT_MAX_BYTES;
pub(crate) const SCHEDULED_SUBMISSION_RECOVERY_PREFIX: &str = "scheduled-proposal-submission:";

/// What the gate decided about a proposal, as a unit.
#[derive(Debug)]
pub enum GateOutcome {
    /// Every protected target holds: proceed to `Ward::apply`.
    Permitted,
    /// At least one thread frayed (§5): the whole proposal is staged at
    /// `~/.coven/pending/`; nothing may be written.
    Staged {
        /// Where the pending proposal was staged.
        pending_path: PathBuf,
        /// The staged proposal id.
        proposal_id: String,
    },
    /// At least one verdict rejected: the proposal is refused as a unit.
    Rejected,
}

/// The gate's full report for one proposal.
#[derive(Debug)]
pub struct GateReport {
    /// Per-target verdicts in request order: `(resolved surface, verdict)`.
    pub verdicts: Vec<(String, threads::Verdict)>,
    /// The unit outcome.
    pub outcome: GateOutcome,
}

impl GateReport {
    /// JSON for API payloads (`threadsGate` field). Purely descriptive — the
    /// daemon acts on [`GateOutcome`], never on this rendering.
    pub fn to_json(&self) -> Value {
        let verdicts: Vec<Value> = self
            .verdicts
            .iter()
            .map(|(surface, verdict)| {
                json!({
                    "surface": surface,
                    "verdict": serde_json::to_value(verdict).unwrap_or(Value::Null),
                })
            })
            .collect();
        let outcome = match &self.outcome {
            GateOutcome::Permitted => json!({ "kind": "permitted" }),
            GateOutcome::Staged {
                pending_path,
                proposal_id,
            } => json!({
                "kind": "staged",
                "pendingPath": pending_path.display().to_string(),
                "proposalId": proposal_id,
            }),
            GateOutcome::Rejected => json!({ "kind": "rejected" }),
        };
        json!({ "verdicts": verdicts, "outcome": outcome })
    }
}

pub(crate) struct StagedCoherenceProposal {
    pub pending_path: PathBuf,
    pub proposal_id: String,
    pub scheduled: Option<crate::proposal_scheduler::ScheduledProposal>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ScheduledSubmissionRecovery {
    proposal_id: String,
    familiar_id: String,
    weave_hash: Vec<u8>,
    body_sha256: String,
}

impl ScheduledSubmissionRecovery {
    fn new(proposal_id: &str, familiar_id: &str, weave_hash: &[u8], body: &[u8]) -> Result<Self> {
        anyhow::ensure!(
            weave_hash.len() == 32,
            "scheduled proposal weave hash is not a SHA-256 digest"
        );
        Ok(Self {
            proposal_id: proposal_id.to_string(),
            familiar_id: familiar_id.to_string(),
            weave_hash: weave_hash.to_vec(),
            body_sha256: sha256_hex(body),
        })
    }

    pub(crate) fn proposal_id(&self) -> &str {
        &self.proposal_id
    }

    pub(crate) fn familiar_id(&self) -> &str {
        &self.familiar_id
    }

    pub(crate) fn weave_hash(&self) -> &[u8] {
        &self.weave_hash
    }

    pub(crate) fn matches_body(&self, body: &[u8]) -> bool {
        self.body_sha256 == sha256_hex(body)
    }

    fn purpose(&self) -> Result<String> {
        Ok(format!(
            "{SCHEDULED_SUBMISSION_RECOVERY_PREFIX}{}",
            serde_json::to_string(self)
                .context("serializing scheduled submission recovery authority")?
        ))
    }
}

pub(crate) fn parse_scheduled_submission_recovery(
    purpose: &str,
) -> Result<Option<ScheduledSubmissionRecovery>> {
    let Some(encoded) = purpose.strip_prefix(SCHEDULED_SUBMISSION_RECOVERY_PREFIX) else {
        return Ok(None);
    };
    let recovery: ScheduledSubmissionRecovery =
        serde_json::from_str(encoded).context("parsing scheduled submission recovery authority")?;
    uuid::Uuid::parse_str(&recovery.proposal_id)
        .context("scheduled submission recovery proposal id is invalid")?;
    anyhow::ensure!(
        !recovery.familiar_id.is_empty(),
        "scheduled submission recovery familiar id is empty"
    );
    anyhow::ensure!(
        recovery.weave_hash.len() == 32,
        "scheduled submission recovery weave hash is invalid"
    );
    anyhow::ensure!(
        recovery.body_sha256.len() == 64
            && recovery
                .body_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "scheduled submission recovery body digest is invalid"
    );
    Ok(Some(recovery))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScheduledPublicationFailure {
    MissingRegionEvidence,
    UnclassifiedSurface { surface: String },
    UnboundRegion { region: String },
    InvalidClassification { reason: String },
}

impl ScheduledPublicationFailure {
    fn reason(&self) -> &'static str {
        match self {
            Self::MissingRegionEvidence => "proposal-region-evidence-missing",
            Self::UnclassifiedSurface { .. } => "proposal-surface-evidence-missing",
            Self::UnboundRegion { .. } => "proposal-region-unbound",
            Self::InvalidClassification { .. } => "proposal-classification-invalid",
        }
    }

    pub(crate) fn details(&self) -> Value {
        match self {
            Self::MissingRegionEvidence => json!({
                "why": self.reason(),
            }),
            Self::UnclassifiedSurface { surface } => json!({
                "why": self.reason(),
                "surface": surface,
            }),
            Self::UnboundRegion { region } => json!({
                "why": self.reason(),
                "region": region,
            }),
            Self::InvalidClassification { reason } => json!({
                "why": self.reason(),
                "reason": reason,
            }),
        }
    }
}

#[derive(Debug)]
struct ScheduledPublicationError {
    failure: ScheduledPublicationFailure,
}

impl std::fmt::Display for ScheduledPublicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.failure.reason())
    }
}

impl std::error::Error for ScheduledPublicationError {}

fn scheduled_publication_error(failure: ScheduledPublicationFailure) -> anyhow::Error {
    ScheduledPublicationError { failure }.into()
}

pub(crate) fn scheduled_publication_failure(
    error: &anyhow::Error,
) -> Option<&ScheduledPublicationFailure> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<ScheduledPublicationError>()
            .map(|typed| &typed.failure)
    })
}

/// Schema for the gate's daemon-owned state inside `coven.sqlite3`: the
/// per-familiar content-baseline manifest. Applied idempotently by
/// `store::open_store` alongside `coven_threads_core::WARD_AUDIT_SCHEMA_SQL`.
pub const WARD_MANIFEST_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS ward_manifest (
    familiar_id  TEXT NOT NULL,
    surface      TEXT NOT NULL,
    manifest_id  TEXT NOT NULL,
    entry_hash   BLOB NOT NULL,
    updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (familiar_id, surface)
);
";

/// Everything the gate needs to adjudicate one proposal.
pub struct GateRequest<'a> {
    /// The coven home (owns `pending/` and the store).
    pub coven_home: &'a Path,
    /// Human-readable familiar id (`familiars.toml` key).
    pub familiar_id: &'a str,
    /// The familiar workspace (home of the protected surfaces).
    pub workspace: &'a Path,
    /// The familiar's Ward configuration.
    pub config: &'a ward::WardConfig,
    /// The proposal's edits, in request order.
    pub edits: &'a [ward::FileEdit],
    /// Gate-2 *resolved* home-relative paths of the proposal's unblocked
    /// Tier-0 targets. Blocked targets are already refused by the Ward
    /// downstream. Empty leaves structural authority to the Ward tiers;
    /// configured identity predicates still constrain the complete candidate.
    pub gated_targets: &'a [String],
    /// The proposal's authorization.
    pub authorization: &'a ward::Authorization,
}

/// The daemon's current weave state for one familiar, as used by the validator.
pub(crate) struct WeaveState {
    pub familiar_uuid: threads::FamiliarId,
    pub weave: threads::Weave,
    pub baseline_snapshot: BTreeMap<String, Option<Vec<u8>>>,
}

/// Gate a proposal's protected targets through the coven-threads weave.
pub fn gate_protected_edits(conn: &Connection, req: &GateRequest<'_>) -> Result<GateReport> {
    let GateRequest {
        coven_home,
        familiar_id,
        workspace,
        config,
        edits,
        gated_targets,
        authorization,
    } = *req;
    ward::validate_file_edit_budget(edits)?;
    let identity_context = crate::ward_identity::candidate_identity_context(
        coven_home,
        familiar_id,
        workspace,
        config,
        edits,
        authorization,
        None,
    );
    let identity_rejection =
        crate::ward_identity::candidate_rejection(config, identity_context.as_ref())?;
    if gated_targets.is_empty() && identity_rejection.is_none() {
        return Ok(GateReport {
            verdicts: Vec::new(),
            outcome: GateOutcome::Permitted,
        });
    }

    let request_writer = match &authorization.principal_signature_fingerprint {
        Some(fp) => threads::WriterId::new(format!("principal:{fp}")),
        None => threads::WriterId::new("client:unsigned"),
    };
    let now = crate::threads_clock::now(coven_home)?;
    let state = build_weave_state_at(
        conn,
        familiar_id,
        workspace,
        config,
        gated_targets,
        identity_rejection.is_none(),
        now,
    )?;
    let familiar_uuid = state.familiar_uuid;
    let weave = state.weave;
    if let Some(verdict) = identity_rejection {
        let mut verdicts = Vec::with_capacity(edits.len());
        for edit in edits {
            let request = threads::MutationRequest {
                surface: threads::SurfaceId::new(edit.target.clone()),
                writer: request_writer.clone(),
                channel: threads::Channel::Mutation,
                identity_context: identity_context.clone(),
            };
            append_audit_row(
                conn,
                familiar_id,
                &familiar_uuid,
                weave.weave_hash(),
                &request,
                &verdict,
                now,
            )?;
            verdicts.push((edit.target.clone(), verdict.clone()));
        }
        return Ok(GateReport {
            verdicts,
            outcome: GateOutcome::Rejected,
        });
    }

    // Validate every gated target; audit every verdict (RFC-0001 §5.6).
    let mut verdicts = Vec::with_capacity(gated_targets.len());
    let mut degraded: Option<(threads::ThreadId, threads::FrayOrSnap)> = None;
    let mut rejected = false;
    for target in gated_targets {
        let request = threads::MutationRequest {
            surface: threads::SurfaceId::new(target.clone()),
            writer: request_writer.clone(),
            channel: threads::Channel::Mutation,
            identity_context: identity_context.clone(),
        };
        let verdict = threads::validate_fail_closed(&weave, &request);
        append_audit_row(
            conn,
            familiar_id,
            &familiar_uuid,
            weave.weave_hash(),
            &request,
            &verdict,
            now,
        )?;
        match &verdict {
            threads::Verdict::Reject { .. } => rejected = true,
            threads::Verdict::DegradeToProposal { thread, fray } => {
                degraded.get_or_insert((*thread, fray.clone()));
            }
            threads::Verdict::Permit { .. } => {}
        }
        verdicts.push((target.clone(), verdict));
    }

    // Unit semantics (§5 + the Ward's own all-or-nothing rule): any Reject
    // refuses the proposal; otherwise any fray stages the whole proposal.
    let outcome = if rejected {
        GateOutcome::Rejected
    } else if let Some((thread_id, fray)) = degraded {
        let pending = pending_proposal(
            &familiar_uuid,
            &request_writer,
            &StagingLane {
                thread_id,
                fray,
                review_kind: None,
            },
            edits,
            now,
        );
        let (pending_path, proposal_id) = stage_legacy_pending_proposal(
            coven_home,
            pending,
            None,
            edits,
            StagingProbeContext {
                familiar_id,
                workspace,
                config,
                authorization,
            },
        )?;
        GateOutcome::Staged {
            pending_path,
            proposal_id,
        }
    } else {
        GateOutcome::Permitted
    };

    Ok(GateReport { verdicts, outcome })
}

/// Build the daemon's authoritative weave view for a familiar.
///
/// `bootstrap_missing_baselines` is true only on the mutation path, preserving
/// the existing first-sight semantics. Read/decision replays observe missing
/// baselines without mutating the store.
pub(crate) fn build_weave_state(
    conn: &Connection,
    familiar_id: &str,
    workspace: &Path,
    config: &ward::WardConfig,
    extra_targets: &[String],
    bootstrap_missing_baselines: bool,
) -> Result<WeaveState> {
    build_weave_state_at(
        conn,
        familiar_id,
        workspace,
        config,
        extra_targets,
        bootstrap_missing_baselines,
        time::OffsetDateTime::now_utc(),
    )
}

pub(crate) fn build_weave_state_at(
    conn: &Connection,
    familiar_id: &str,
    workspace: &Path,
    config: &ward::WardConfig,
    extra_targets: &[String],
    bootstrap_missing_baselines: bool,
    now: time::OffsetDateTime,
) -> Result<WeaveState> {
    build_weave_state_for_writer_at(
        conn,
        familiar_id,
        workspace,
        config,
        extra_targets,
        bootstrap_missing_baselines,
        now,
        None,
    )
}

// All surfaces share one captured time alongside the existing writer-specific weave inputs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_weave_state_for_writer_at(
    conn: &Connection,
    familiar_id: &str,
    workspace: &Path,
    config: &ward::WardConfig,
    extra_targets: &[String],
    bootstrap_missing_baselines: bool,
    now: time::OffsetDateTime,
    writer: Option<&threads::WriterId>,
) -> Result<WeaveState> {
    let familiar_uuid = familiar_weave_id(familiar_id);
    let principal_writer =
        threads::WriterId::new(format!("principal:{}", config.principal_key_fingerprint));
    // A pending principal label cannot rebind a rotated Ward principal.
    let review_writer = writer
        .filter(|writer| !writer.as_str().starts_with("principal:"))
        .unwrap_or(&principal_writer);

    // Weave one thread per protected surface: the literal (non-glob) tier-0
    // declarations plus any resolved protected targets being replayed.
    let mut surfaces: Vec<String> = config
        .protected_surface
        .iter()
        .filter(|entry| !entry.contains(['*', '?', '[']))
        .cloned()
        .collect();
    for target in extra_targets {
        if !surfaces.contains(target) {
            surfaces.push(target.clone());
        }
    }
    surfaces.sort();

    let manifest_id = load_or_create_manifest_id(conn, familiar_id)?;
    let mut woven = Vec::with_capacity(surfaces.len());
    let mut baseline_snapshot = BTreeMap::new();
    for surface in &surfaces {
        let surface_id = threads::SurfaceId::new(surface.clone());
        let disk = read_surface(workspace, surface)?;
        let current_hash = threads::manifest_entry_hash(&surface_id, &disk);

        let baseline = load_baseline(conn, familiar_id, surface)?;
        let validated_baseline = match &baseline {
            Some(recorded) => Some(recorded.clone()),
            None if bootstrap_missing_baselines => Some(current_hash.to_vec()),
            None => None,
        };
        let (entry_hash, drifted) = match baseline {
            Some(recorded) => {
                let drifted = recorded.as_slice() != current_hash.as_slice();
                (recorded, drifted)
            }
            None if bootstrap_missing_baselines => {
                // First sight: bootstrap the baseline from current content.
                // Observation, not authority — recording a baseline grants
                // nothing; it only makes future drift detectable.
                store_baseline(conn, familiar_id, surface, &manifest_id, &current_hash)?;
                (current_hash.to_vec(), false)
            }
            None => (current_hash.to_vec(), false),
        };
        baseline_snapshot.insert(surface.clone(), validated_baseline);

        let mut thread = threads::Thread {
            id: threads::ThreadId::new(),
            surface: surface_id.clone(),
            writer: if review_writer == &principal_writer
                || config.classify_resolved_path(surface)? == ward::Tier::Protected
            {
                principal_writer.clone()
            } else {
                review_writer.clone()
            },
            strands: vec![
                threads::Strand::ContentHash {
                    id: threads::StrandId::new(),
                    algorithm: threads::HashAlgo::Blake3,
                    value: blake3::hash(&disk).as_bytes().to_vec(),
                },
                threads::Strand::ManifestEntry {
                    id: threads::StrandId::new(),
                    manifest_id,
                    entry_hash,
                },
                threads::Strand::SerializationMarker {
                    id: threads::StrandId::new(),
                    format_version: SERIALIZATION_FORMAT_VERSION.to_string(),
                    contract_hash: blake3::hash(SERIALIZATION_CONTRACT).as_bytes().to_vec(),
                },
            ],
            holds_under: PROTECTED_CHANNELS.to_vec(),
            created_at: now,
            tension: threads::TensionState::Holds,
        };
        if drifted {
            let manifest_strand = thread
                .strands
                .iter()
                .find(|s| matches!(s, threads::Strand::ManifestEntry { .. }))
                .map(threads::Strand::id);
            thread.fray(
                manifest_strand,
                threads::Channel::Mutation,
                threads::FrayReason::ManifestEntryMismatch,
                now,
            );
        }
        woven.push(thread);
    }

    let structural = threads::AllSurfacesHoldOnChannels {
        name: format!("{familiar_id}-protected-surface"),
        surfaces: surfaces
            .iter()
            .map(|s| threads::SurfaceId::new(s.clone()))
            .collect(),
        channels: PROTECTED_CHANNELS.to_vec(),
    };
    let pattern: Box<dyn threads::PatternPredicate + Send + Sync> =
        if let Some(invariants) = config.identity_invariant_set()? {
            Box::new(threads::IdentityAwarePattern {
                structural: Box::new(structural),
                invariants,
            })
        } else {
            Box::new(structural)
        };
    let weave = threads::Weave::new(threads::WeaveId::new(), familiar_uuid, woven, pattern, None)
        .context("weaving protected surfaces")?;

    Ok(WeaveState {
        familiar_uuid,
        weave,
        baseline_snapshot,
    })
}

/// Deterministic weave-level familiar id from the human-readable familiar id
/// (UUIDv5 over the OID namespace, so audits correlate across restarts).
pub(crate) fn familiar_weave_id(familiar_id: &str) -> threads::FamiliarId {
    threads::FamiliarId(uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        familiar_id.as_bytes(),
    ))
}

/// Read a protected surface's current content for baseline comparison.
///
/// `surface` strings come from `ward.toml` declarations and Gate-2-resolved
/// targets. Resolved targets are already confined, but ward.toml literals are
/// only operator-authored convention — so this function re-enforces
/// confinement itself (fail-closed, review finding): no absolute paths, no
/// `..`/`.` segments, no symlinks anywhere in the path (intermediate
/// directories included), opened handles are revalidated before reading, and
/// the read is capped so a pathological declaration cannot balloon memory.
pub(crate) fn read_surface(workspace: &Path, surface: &str) -> Result<Vec<u8>> {
    Ok(read_surface_if_exists(workspace, surface)?.unwrap_or_default())
}

#[derive(Clone, Copy)]
enum SurfaceReadPolicy {
    WardFile,
    WardEditBudget { retained_content_bytes: u64 },
}

impl SurfaceReadPolicy {
    fn max_bytes(self) -> u64 {
        match self {
            Self::WardFile => MAX_SURFACE_BYTES,
            Self::WardEditBudget {
                retained_content_bytes,
            } => MAX_SURFACE_BYTES
                .min(ward::WARD_RETAINED_CONTENT_MAX_BYTES.saturating_sub(retained_content_bytes)),
        }
    }

    fn limit_error(self, surface: &str, observed_bytes: u64) -> anyhow::Error {
        match self {
            Self::WardFile => anyhow::anyhow!(
                "protected surface `{surface}` exceeds the {MAX_SURFACE_BYTES}-byte baseline cap"
            ),
            Self::WardEditBudget {
                retained_content_bytes: _,
            } if observed_bytes > ward::WARD_FILE_CONTENT_MAX_BYTES => {
                ward::WardEditBudgetFailure::ExistingBeforeImage {
                    target: surface.to_string(),
                    observed_bytes,
                    max_bytes: ward::WARD_FILE_CONTENT_MAX_BYTES,
                }
                .into()
            }
            Self::WardEditBudget {
                retained_content_bytes,
            } => ward::WardEditBudgetFailure::BatchRetainedMemory {
                attempted_bytes: retained_content_bytes.saturating_add(observed_bytes),
                max_bytes: ward::WARD_RETAINED_CONTENT_MAX_BYTES,
            }
            .into(),
        }
    }
}

/// The confined surface read with file absence preserved.
///
/// Probe evidence needs to distinguish an absent baseline from an existing
/// empty file so a later approval can detect either state changing. The
/// authority weave keeps its historical absent-as-empty convention through
/// [`read_surface`].
pub(crate) fn read_surface_if_exists(workspace: &Path, surface: &str) -> Result<Option<Vec<u8>>> {
    read_surface_if_exists_with_policy(workspace, surface, SurfaceReadPolicy::WardFile)
}

pub(crate) fn read_surface_if_exists_with_budget(
    workspace: &Path,
    surface: &str,
    budget: &mut ward::WardEditBudget,
) -> Result<Option<Vec<u8>>> {
    let contents = read_surface_if_exists_with_policy(
        workspace,
        surface,
        SurfaceReadPolicy::WardEditBudget {
            retained_content_bytes: budget.retained_content_bytes(),
        },
    )?;
    if let Some(contents) = contents.as_deref() {
        budget.reserve_retained_content(u64::try_from(contents.len()).map_err(|_| {
            ward::WardEditBudgetFailure::BatchRetainedMemory {
                attempted_bytes: u64::MAX,
                max_bytes: ward::WARD_RETAINED_CONTENT_MAX_BYTES,
            }
        })?)?;
    }
    Ok(contents)
}

fn read_surface_if_exists_with_policy(
    workspace: &Path,
    surface: &str,
    policy: SurfaceReadPolicy,
) -> Result<Option<Vec<u8>>> {
    if surface.starts_with('/') || surface.starts_with('\\') {
        anyhow::bail!("protected surface `{surface}` must be workspace-relative");
    }
    let mut components = Vec::new();
    for segment in surface.split('/').filter(|s| !s.is_empty()) {
        if segment == ".." || segment == "." || segment.contains('\\') || segment.contains(':') {
            anyhow::bail!(
                "protected surface `{surface}` contains a path-escaping segment; \
                 declarations must stay inside the familiar workspace"
            );
        }
        components.push(segment);
    }

    let mut directory = Dir::open_ambient_dir(workspace, ambient_authority())
        .with_context(|| format!("opening familiar workspace for surface `{surface}`"))?;
    let workspace_metadata = directory
        .dir_metadata()
        .with_context(|| format!("inspecting familiar workspace for surface `{surface}`"))?;
    if !workspace_metadata.is_dir() || metadata_is_windows_reparse_point(&workspace_metadata) {
        anyhow::bail!("familiar workspace for surface `{surface}` is not a real directory");
    }

    let Some((name, parents)) = components.split_last() else {
        anyhow::bail!("protected surface `{surface}` must name a file");
    };
    for component in parents {
        let metadata = match directory.symlink_metadata(component) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting surface `{surface}`"))
            }
        };
        if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
            anyhow::bail!(
                "protected surface `{surface}` has a symlinked or non-directory ancestor"
            );
        }
        directory = directory
            .open_dir_nofollow(component)
            .with_context(|| format!("opening ancestor of surface `{surface}`"))?;
        let opened_metadata = directory
            .dir_metadata()
            .with_context(|| format!("inspecting opened ancestor of surface `{surface}`"))?;
        if !opened_metadata.is_dir() || metadata_is_windows_reparse_point(&opened_metadata) {
            anyhow::bail!(
                "protected surface `{surface}` has a symlinked or non-directory ancestor"
            );
        }
    }

    let metadata = match directory.symlink_metadata(name) {
        Ok(metadata) => metadata,
        // An absent protected file baselines as empty: creating it later is
        // drift like any other content change.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("inspecting surface `{surface}`")),
    };
    // Only regular files are hashable surfaces; a symlinked or special-file
    // surface is refused rather than followed out of the workspace.
    if !metadata.is_file() {
        anyhow::bail!("protected surface `{surface}` is not a regular file inside the workspace");
    }
    let max_bytes = policy.max_bytes();
    if metadata.len() > max_bytes {
        return Err(policy.limit_error(surface, metadata.len()));
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let file = directory
        .open_with(name, &options)
        .with_context(|| format!("opening surface `{surface}`"))?;
    let opened_metadata = file
        .metadata()
        .with_context(|| format!("inspecting opened surface `{surface}`"))?;
    if !opened_metadata.is_file() || metadata_is_windows_reparse_point(&opened_metadata) {
        anyhow::bail!("protected surface `{surface}` is not a regular file inside the workspace");
    }
    if opened_metadata.len() > max_bytes {
        return Err(policy.limit_error(surface, opened_metadata.len()));
    }
    let mut bytes = Vec::with_capacity(opened_metadata.len().min(max_bytes) as usize);
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading surface `{surface}`"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(policy.limit_error(surface, bytes.len() as u64));
    }
    Ok(Some(bytes))
}

#[cfg(windows)]
fn metadata_is_windows_reparse_point(metadata: &cap_std::fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_windows_reparse_point(_metadata: &cap_std::fs::Metadata) -> bool {
    false
}

fn load_or_create_manifest_id(conn: &Connection, familiar_id: &str) -> Result<threads::ManifestId> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT manifest_id FROM ward_manifest WHERE familiar_id = ?1 LIMIT 1",
            params![familiar_id],
            |row| row.get(0),
        )
        .optional()
        .context("loading ward_manifest id")?;
    match existing {
        Some(raw) => Ok(threads::ManifestId(
            uuid::Uuid::parse_str(&raw).context("ward_manifest.manifest_id is not a uuid")?,
        )),
        None => Ok(threads::ManifestId::new()),
    }
}

pub(crate) fn load_baseline(
    conn: &Connection,
    familiar_id: &str,
    surface: &str,
) -> Result<Option<Vec<u8>>> {
    conn.query_row(
        "SELECT entry_hash FROM ward_manifest WHERE familiar_id = ?1 AND surface = ?2",
        params![familiar_id, surface],
        |row| row.get(0),
    )
    .optional()
    .context("loading ward_manifest baseline")
}

fn store_baseline(
    conn: &Connection,
    familiar_id: &str,
    surface: &str,
    manifest_id: &threads::ManifestId,
    entry_hash: &[u8; 32],
) -> Result<()> {
    conn.execute(
        "INSERT INTO ward_manifest (familiar_id, surface, manifest_id, entry_hash)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (familiar_id, surface) DO UPDATE SET
             manifest_id = excluded.manifest_id,
             entry_hash = excluded.entry_hash,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        params![
            familiar_id,
            surface,
            manifest_id.0.to_string(),
            entry_hash.as_slice()
        ],
    )
    .context("storing ward_manifest baseline")?;
    Ok(())
}

pub(crate) fn advance_surface_baseline(
    conn: &Connection,
    familiar_id: &str,
    workspace: &Path,
    surface: &str,
) -> Result<()> {
    let disk = read_surface(workspace, surface)?;
    advance_surface_baseline_from_bytes(conn, familiar_id, workspace, surface, &disk)
}

pub(crate) fn advance_surface_baseline_from_bytes(
    conn: &Connection,
    familiar_id: &str,
    workspace: &Path,
    surface: &str,
    expected_bytes: &[u8],
) -> Result<()> {
    let manifest_id = load_or_create_manifest_id(conn, familiar_id)?;
    let surface_id = threads::SurfaceId::new(surface.to_string());
    let disk = read_surface(workspace, surface)?;
    if disk != expected_bytes {
        anyhow::bail!("surface `{surface}` changed after approved apply; baseline not advanced");
    }
    let entry_hash = threads::manifest_entry_hash(&surface_id, expected_bytes);
    store_baseline(conn, familiar_id, surface, &manifest_id, &entry_hash)
}

pub(crate) fn append_audit_row(
    conn: &Connection,
    familiar_id: &str,
    familiar_uuid: &threads::FamiliarId,
    weave_hash: &[u8],
    request: &threads::MutationRequest,
    verdict: &threads::Verdict,
    now: time::OffsetDateTime,
) -> Result<()> {
    let record = threads::WardAuditRecord::for_verdict(
        *familiar_uuid,
        weave_hash,
        request,
        verdict,
        now,
        now,
    );
    let files_touched = serde_json::to_string(
        &record
            .files_touched
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>(),
    )?;
    let format = time::format_description::well_known::Rfc3339;
    conn.execute(
        "INSERT INTO ward_audit (
            event_type, proposal_id, familiar_id, ward_version, ward_hash,
            tier, decision, approver, diff_hash, files_touched, channel,
            thread_id, submitted_at, decided_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            record.event_type.tag(),
            record.proposal_id.map(|p| p.0.to_string()),
            // Human-readable familiar id in the store; the uuid rides in the
            // JSON record shape for cross-system correlation.
            familiar_id,
            record.ward_version,
            record.ward_hash,
            record.tier,
            record.decision,
            record.approver.map(|w| w.0),
            record.diff_hash,
            files_touched,
            record.channel.map(|c| format!("{c:?}").to_lowercase()),
            record.thread_id.map(|t| t.0.to_string()),
            record.submitted_at.format(&format)?,
            record.decided_at.format(&format)?,
        ],
    )
    .context("appending ward_audit row")?;
    Ok(())
}

/// Persist the Ward's Gate-4 apply records into the append-only `ward_audit`
/// ledger (#414; RFC-0001 §5.6, coven-threads#5).
///
/// One `apply_audit` row per written change that carries an
/// [`ward::AuditRecord`] (Tier-2 logged writes). Following the upstream
/// `WardAuditRecord::for_apply` contract, `diff_hash` carries the post-write
/// SHA-256 and the pre-write hash plus byte count ride in the `detail` JSON
/// (`{"prev_sha256":…,"bytes_written":…}`). The weave view is read-only:
/// persisting audit rows must not bootstrap baselines.
#[cfg(test)]
pub fn persist_apply_audit_records(
    conn: &mut Connection,
    familiar_id: &str,
    workspace: &Path,
    config: &ward::WardConfig,
    report: &ward::ApplyReport,
) -> Result<()> {
    persist_apply_audit_records_on_connection(
        conn,
        familiar_id,
        workspace,
        config,
        report,
        time::OffsetDateTime::now_utc(),
    )
}

pub(crate) fn persist_apply_audit_records_on_connection(
    conn: &Connection,
    familiar_id: &str,
    workspace: &Path,
    config: &ward::WardConfig,
    report: &ward::ApplyReport,
    now: time::OffsetDateTime,
) -> Result<()> {
    if report.audit_records().next().is_none() {
        return Ok(());
    }
    let owns_transaction = conn.is_autocommit();
    if owns_transaction {
        conn.execute_batch("BEGIN IMMEDIATE")
            .context("starting apply-audit batch transaction")?;
    }
    let result = (|| -> Result<()> {
        let state = build_weave_state_at(conn, familiar_id, workspace, config, &[], false, now)?;
        append_apply_audit_records_at(
            conn,
            None,
            familiar_id,
            state.weave.weave_hash(),
            report,
            threads::Channel::Mutation,
            now,
        )?;
        if owns_transaction {
            conn.execute_batch("COMMIT")
                .context("committing apply-audit batch transaction")?;
        }
        Ok(())
    })();
    if result.is_err() && owns_transaction {
        let _ = conn.execute_batch("ROLLBACK");
    }
    result
}

/// Append the Ward's logged apply records to an existing transaction scope.
///
/// Proposal finalization uses this form so `apply_audit` rows and the terminal
/// proposal event commit as one unit. Direct writes use
/// [`persist_apply_audit_records_on_connection`] to append within the existing
/// transaction when present, or a dedicated transaction otherwise.
pub(crate) fn append_apply_audit_records_at(
    conn: &Connection,
    proposal_id: Option<&str>,
    familiar_id: &str,
    ward_hash: &[u8],
    report: &ward::ApplyReport,
    channel: threads::Channel,
    now: time::OffsetDateTime,
) -> Result<()> {
    let records = report.audit_records();
    let familiar_uuid = familiar_weave_id(familiar_id);
    let format = time::format_description::well_known::Rfc3339;
    let now_text = now.format(&format)?;
    {
        let mut statement = conn.prepare(
            "INSERT INTO ward_audit (
                event_type, proposal_id, familiar_id, ward_version, ward_hash,
                tier, decision, approver, diff_hash, detail, files_touched,
                channel, submitted_at, decided_at
            ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, NULL, ?7, ?8, ?9, ?10, ?11, ?11)",
        )?;
        for audit in records {
            let prev = audit
                .prev_sha256
                .as_deref()
                .map(hex_to_bytes)
                .transpose()
                .context("decoding apply-audit prev_sha256")?;
            let next =
                hex_to_bytes(&audit.next_sha256).context("decoding apply-audit next_sha256")?;
            let record = threads::WardAuditRecord::for_apply(
                familiar_uuid,
                ward_hash,
                threads::SurfaceId::new(audit.resolved.clone()),
                &format!("tier_{}", u8::from(audit.tier)),
                prev.as_deref(),
                Some(&next),
                audit.bytes_written as u64,
                Some(channel),
                now,
                now,
            );
            let files_touched = serde_json::to_string(
                &record
                    .files_touched
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>(),
            )?;
            statement
                .execute(params![
                    record.event_type.tag(),
                    proposal_id,
                    // Human-readable familiar id in the store; the uuid rides in
                    // the JSON record shape for cross-system correlation.
                    familiar_id,
                    ward_hash,
                    record.tier,
                    record.decision,
                    record.diff_hash,
                    record.detail,
                    files_touched,
                    record.channel.map(|c| format!("{c:?}").to_lowercase()),
                    now_text,
                ])
                .context("appending apply_audit row")?;
        }
    }
    Ok(())
}

/// Decode a hex digest into raw bytes. The Ward emits SHA-256 hex strings;
/// the `ward_audit` ledger stores raw bytes (`diff_hash BLOB`).
fn hex_to_bytes(hex: &str) -> Result<Vec<u8>> {
    if !hex.is_ascii() {
        anyhow::bail!("non-ASCII hex digest");
    }
    if hex.len() != 64 {
        anyhow::bail!("expected 64-character SHA-256 hex digest");
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).context("invalid hex digest"))
        .collect()
}

/// Stage a proposal held **solely** for Gate-3 coherence review (Tier-1
/// targets) as a pending proposal beside the Tier-0 authority lane
/// (`docs/design/ward-gate3-coherence.md` G3.1).
///
/// Tier-1 surfaces are deliberately not woven, so the staged record carries
/// `FrayOrSnap::NotCovered { channel: Mutation }` — literally true: no thread
/// covers the surface — under a fresh `ThreadId`, plus the `reviewKind:
/// "coherence"` sidecar marker the decide path branches on. One
/// `proposal_submitted` row lands in the append-only `ward_audit` ledger.
/// The decide path re-probes this sidecar, keeps Tier-1 outside the weave, and
/// clears only `RequiresCoherenceReview` after an explicit principal approval.
pub(crate) fn stage_coherence_proposal(
    audit_reservation: &mut crate::store::WardAuditReservation<'_>,
    coven_home: &Path,
    familiar_id: &str,
    workspace: &Path,
    config: &ward::WardConfig,
    edits: &[ward::FileEdit],
    authorization: &ward::Authorization,
) -> Result<StagedCoherenceProposal> {
    ward::validate_file_edit_budget(edits)?;
    let request_writer = match &authorization.principal_signature_fingerprint {
        Some(fp) => threads::WriterId::new(format!("principal:{fp}")),
        None => threads::WriterId::new("client:unsigned"),
    };
    let now = crate::threads_clock::now(coven_home)?;
    // Read-only weave view: coherence staging must not bootstrap baselines.
    let state = build_weave_state_at(
        audit_reservation.connection(),
        familiar_id,
        workspace,
        config,
        &[],
        false,
        now,
    )?;
    let weave_hash = state.weave.weave_hash().to_vec();
    let thread_id = threads::ThreadId::new();
    let lane = StagingLane {
        thread_id,
        fray: threads::FrayOrSnap::NotCovered {
            channel: threads::Channel::Mutation,
        },
        review_kind: Some("coherence"),
    };
    let pending = pending_proposal(&state.familiar_uuid, &request_writer, &lane, edits, now);
    let staging = match config.compiled_approval_tiers()? {
        Some(bindings) => stage_scheduled_coherence_proposal(
            ScheduledSubmissionContext {
                audit_reservation,
                familiar_id,
                weave_hash: &weave_hash,
            },
            coven_home,
            pending,
            edits,
            &bindings,
            now,
            StagingProbeContext {
                familiar_id,
                workspace,
                config,
                authorization,
            },
        )?,
        None => {
            let (pending_path, proposal_id) = stage_legacy_pending_proposal(
                coven_home,
                pending,
                lane.review_kind,
                edits,
                StagingProbeContext {
                    familiar_id,
                    workspace,
                    config,
                    authorization,
                },
            )?;
            StagedCoherenceProposal {
                pending_path,
                proposal_id,
                scheduled: None,
            }
        }
    };

    if staging.scheduled.is_none() {
        let files_touched = serde_json::to_string(
            &edits
                .iter()
                .map(|edit| edit.target.as_str())
                .collect::<Vec<_>>(),
        )?;
        let format = time::format_description::well_known::Rfc3339;
        let now_text = now.format(&format)?;
        audit_reservation
            .connection()
            .execute(
                "INSERT INTO ward_audit (
                event_type, proposal_id, familiar_id, ward_version, ward_hash,
                tier, decision, approver, diff_hash, files_touched, channel,
                thread_id, submitted_at, decided_at, detail
            ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, NULL, NULL, ?7, ?8, ?10, ?9, ?9, NULL)",
                rusqlite::params![
                    threads::AuditEventType::ProposalSubmitted.tag(),
                    staging.proposal_id.as_str(),
                    familiar_id,
                    &weave_hash,
                    i64::from(u8::from(ward::Tier::Reviewed)),
                    "staged:coherence",
                    files_touched,
                    format!("{:?}", threads::Channel::Mutation).to_lowercase(),
                    now_text,
                    thread_id.0.to_string(),
                ],
            )
            .context("appending proposal_submitted audit for coherence staging")?;
    }

    Ok(staging)
}

/// Which review lane a staged proposal belongs to, plus the thread evidence
/// recorded with it.
struct StagingLane {
    thread_id: threads::ThreadId,
    fray: threads::FrayOrSnap,
    /// `Some("coherence")` writes the sidecar marker; `None` is the
    /// authority lane (absent field ⇒ authority for existing files).
    review_kind: Option<&'static str>,
}

struct StagingProbeContext<'a> {
    familiar_id: &'a str,
    workspace: &'a Path,
    config: &'a ward::WardConfig,
    authorization: &'a ward::Authorization,
}

struct ScheduledSubmissionContext<'a, 'conn> {
    audit_reservation: &'a mut crate::store::WardAuditReservation<'conn>,
    familiar_id: &'a str,
    weave_hash: &'a [u8],
}

fn stage_pending_proposal(
    coven_home: &Path,
    pending: &threads::PendingProposal,
    review_kind: Option<&'static str>,
    identity_evidence: Option<[u8; 32]>,
    probes: &[crate::ward_probes::SurfaceProbeReport],
) -> Result<PathBuf> {
    let pending_dir = coven_home.join("pending");
    std::fs::create_dir_all(&pending_dir)
        .with_context(|| format!("creating {}", pending_dir.display()))?;
    let path = pending_dir.join(pending.file_name());
    let body = {
        /// On-disk pending-proposal shape: the core type plus additive lane
        /// and probe-evidence sidecars. An absent lane still means authority;
        /// existing files and the core deserializer keep working unchanged.
        #[derive(serde::Serialize)]
        struct StagedProposalFile<'a> {
            #[serde(flatten)]
            proposal: &'a threads::PendingProposal,
            #[serde(rename = "reviewKind", skip_serializing_if = "Option::is_none")]
            review_kind: Option<&'static str>,
            #[serde(rename = "identityEvidence", skip_serializing_if = "Option::is_none")]
            identity_evidence: Option<[u8; 32]>,
            probes: &'a [crate::ward_probes::SurfaceProbeReport],
        }
        serde_json::to_vec_pretty(&StagedProposalFile {
            proposal: pending,
            review_kind,
            identity_evidence,
            probes,
        })
        .context("serializing pending proposal")?
    };
    crate::api::validate_proposal_envelope_preflight(&body)?;
    crate::proposal_store::publish_new(coven_home, &path, &body)?;
    Ok(path)
}

fn pending_proposal(
    familiar_uuid: &threads::FamiliarId,
    writer: &threads::WriterId,
    lane: &StagingLane,
    edits: &[ward::FileEdit],
    now: time::OffsetDateTime,
) -> threads::PendingProposal {
    threads::PendingProposal {
        id: threads::ProposalId::new(),
        familiar_id: *familiar_uuid,
        writer: writer.clone(),
        channel: threads::Channel::Mutation,
        thread_id: lane.thread_id,
        fray: lane.fray.clone(),
        edits: edits
            .iter()
            .map(|edit| threads::StagedEdit {
                surface: threads::SurfaceId::new(edit.target.clone()),
                contents: threads::StagedContents::from_bytes(&edit.new_contents),
            })
            .collect(),
        staged_at: now,
    }
}

fn stage_legacy_pending_proposal(
    coven_home: &Path,
    pending: threads::PendingProposal,
    review_kind: Option<&'static str>,
    edits: &[ward::FileEdit],
    probe_context: StagingProbeContext<'_>,
) -> Result<(PathBuf, String)> {
    ward::validate_file_edit_budget(edits)?;
    let identity_evidence = staging_identity_evidence(coven_home, edits, &probe_context)?;
    let probes = crate::ward_probes::run_at_staging(
        probe_context.workspace,
        probe_context.config,
        edits,
        probe_context.authorization,
    )
    .context("running deterministic Ward probes")?;
    let path = stage_pending_proposal(
        coven_home,
        &pending,
        review_kind,
        identity_evidence,
        &probes,
    )?;
    Ok((path, pending.id.0.to_string()))
}

fn staging_identity_evidence(
    coven_home: &Path,
    edits: &[ward::FileEdit],
    probe_context: &StagingProbeContext<'_>,
) -> Result<Option<[u8; 32]>> {
    let identity_context = crate::ward_identity::candidate_identity_context(
        coven_home,
        probe_context.familiar_id,
        probe_context.workspace,
        probe_context.config,
        edits,
        probe_context.authorization,
        None,
    );
    if let Some(verdict) =
        crate::ward_identity::candidate_rejection(probe_context.config, identity_context.as_ref())?
    {
        anyhow::bail!("identity predicates refuse proposal staging: {verdict:?}");
    }
    crate::ward_identity::candidate_binding(probe_context.config, identity_context.as_ref())
}

fn stage_scheduled_coherence_proposal(
    submission: ScheduledSubmissionContext<'_, '_>,
    coven_home: &Path,
    pending: threads::PendingProposal,
    edits: &[ward::FileEdit],
    bindings: &ward::CompiledApprovalTiers,
    now: time::OffsetDateTime,
    probe_context: StagingProbeContext<'_>,
) -> Result<StagedCoherenceProposal> {
    let mut budget = ward::validate_file_edit_budget(edits)?;
    let identity_evidence = staging_identity_evidence(coven_home, edits, &probe_context)?;
    let diff = materialize_diff(probe_context.workspace, edits, &mut budget)?;
    let region_evidence = threads::SurfaceRegionRegistry::default_registry().classify_all(&diff);
    if region_evidence.is_empty() {
        return Err(scheduled_publication_error(
            ScheduledPublicationFailure::MissingRegionEvidence,
        ));
    }

    let mut covered_surfaces = BTreeSet::new();
    let mut approval_path: Option<threads::ApprovalPath> = None;
    for evidence in &region_evidence {
        for surface in &evidence.affected_surfaces {
            covered_surfaces.insert(surface.as_str().to_string());
        }
        let path = bindings
            .approval_path_for(&evidence.region_id)
            .cloned()
            .ok_or_else(|| {
                scheduled_publication_error(ScheduledPublicationFailure::UnboundRegion {
                    region: evidence.region_id.as_str().to_string(),
                })
            })?;
        approval_path = Some(match approval_path {
            Some(existing) => existing.highest(path),
            None => path,
        });
    }
    for edit in edits {
        if !covered_surfaces.contains(edit.target.as_str()) {
            return Err(scheduled_publication_error(
                ScheduledPublicationFailure::UnclassifiedSurface {
                    surface: edit.target.clone(),
                },
            ));
        }
    }

    let path_floor = edits.iter().try_fold(u8::MAX, |floor, edit| {
        Ok::<_, anyhow::Error>(floor.min(u8::from(
            probe_context.config.classify_resolved_path(&edit.target)?,
        )))
    })?;
    let region_floor = threads::SurfaceRegionRegistry::path_tier_floor(&region_evidence);
    let classification = threads::ProposalClassification {
        proposal_id: pending.id,
        familiar_id: pending.familiar_id,
        channel: pending.channel,
        affected_surfaces: pending
            .edits
            .iter()
            .map(|edit| edit.surface.clone())
            .collect(),
        affected_regions: region_evidence
            .iter()
            .map(|item| item.region_id.clone())
            .collect(),
        path_tier_floor: path_floor.min(region_floor),
        approval_path: approval_path.expect("non-empty region evidence binds an approval path"),
        evidence_replay_hash: threads::evidence_replay_hash(&diff, &region_evidence),
        classified_at: now,
    };
    let scheduled =
        crate::proposal_scheduler::ScheduledProposal::try_new(pending, classification, diff)
            .map_err(|error| {
                scheduled_publication_error(ScheduledPublicationFailure::InvalidClassification {
                    reason: error.to_string(),
                })
            })?;
    let probes = crate::ward_probes::run_at_staging(
        probe_context.workspace,
        probe_context.config,
        edits,
        probe_context.authorization,
    )
    .context("running deterministic Ward probes")?;

    let pending_dir = coven_home.join("pending");
    std::fs::create_dir_all(&pending_dir)
        .with_context(|| format!("creating {}", pending_dir.display()))?;
    let path = pending_dir.join(scheduled.pending().file_name());
    let body = {
        #[derive(serde::Serialize)]
        struct StagedScheduledProposalFile<'a> {
            #[serde(flatten)]
            scheduled: &'a crate::proposal_scheduler::ScheduledProposal,
            #[serde(rename = "identityEvidence", skip_serializing_if = "Option::is_none")]
            identity_evidence: Option<[u8; 32]>,
            probes: &'a [crate::ward_probes::SurfaceProbeReport],
        }
        serde_json::to_vec_pretty(&StagedScheduledProposalFile {
            scheduled: &scheduled,
            identity_evidence,
            probes: &probes,
        })
        .context("serializing scheduled proposal")?
    };
    crate::api::validate_proposal_envelope_preflight(&body)?;
    let recovery = ScheduledSubmissionRecovery::new(
        &scheduled.pending().id.0.to_string(),
        submission.familiar_id,
        submission.weave_hash,
        &body,
    )?;
    submission
        .audit_reservation
        .replace_purpose(&recovery.purpose()?)?;
    submission.audit_reservation.preserve_if_unfinished();
    crate::proposal_store::publish_new(coven_home, &path, &body)?;
    maybe_fail_scheduled_submission_after_publish(coven_home)?;
    append_scheduled_submission_audit(
        submission.audit_reservation.connection(),
        &scheduled,
        identity_evidence,
        submission.familiar_id,
        submission.weave_hash,
    )?;
    Ok(StagedCoherenceProposal {
        pending_path: path,
        proposal_id: scheduled.pending().id.0.to_string(),
        scheduled: Some(scheduled),
    })
}

pub(crate) fn append_scheduled_submission_audit(
    conn: &Connection,
    scheduled: &crate::proposal_scheduler::ScheduledProposal,
    identity_evidence: Option<[u8; 32]>,
    familiar_id: &str,
    weave_hash: &[u8],
) -> Result<()> {
    anyhow::ensure!(
        crate::threads_gate::familiar_weave_id(familiar_id) == scheduled.pending().familiar_id,
        "scheduled proposal familiar does not match submission authority"
    );
    anyhow::ensure!(
        weave_hash.len() == 32,
        "scheduled proposal weave hash is not a SHA-256 digest"
    );
    let pending = scheduled.pending();
    let files_touched = serde_json::to_string(
        &pending
            .edits
            .iter()
            .map(|edit| edit.surface.as_str())
            .collect::<Vec<_>>(),
    )?;
    let submitted_at = pending
        .staged_at
        .format(&time::format_description::well_known::Rfc3339)?;
    let detail = serde_json::to_string(&json!({
        "classification": scheduled.classification(),
        "veto_deadline": scheduled.veto_deadline(),
        "earliest_close": scheduled.earliest_close(),
        "identity_evidence": identity_evidence,
    }))?;
    conn.execute(
        "INSERT INTO ward_audit (
            event_type, proposal_id, familiar_id, ward_version, ward_hash,
            tier, decision, approver, diff_hash, files_touched, channel,
            thread_id, submitted_at, decided_at, detail
        ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, 'staged:scheduled', NULL, NULL, ?6, ?7, ?8, ?9, ?9, ?10)",
        rusqlite::params![
            threads::AuditEventType::ProposalSubmitted.tag(),
            pending.id.0.to_string(),
            familiar_id,
            weave_hash,
            i64::from(u8::from(ward::Tier::Reviewed)),
            files_touched,
            format!("{:?}", pending.channel).to_lowercase(),
            pending.thread_id.0.to_string(),
            submitted_at,
            detail,
        ],
    )
    .context("appending scheduled proposal submission audit")?;
    Ok(())
}

#[cfg(test)]
fn scheduled_submission_failpoints() -> &'static std::sync::Mutex<BTreeSet<PathBuf>> {
    static FAILPOINTS: std::sync::OnceLock<std::sync::Mutex<BTreeSet<PathBuf>>> =
        std::sync::OnceLock::new();
    FAILPOINTS.get_or_init(|| std::sync::Mutex::new(BTreeSet::new()))
}

#[cfg(test)]
pub(crate) fn fail_next_scheduled_submission_after_publish(coven_home: &Path) {
    scheduled_submission_failpoints()
        .lock()
        .expect("scheduled submission failpoint lock poisoned")
        .insert(coven_home.to_path_buf());
}

#[cfg(test)]
fn maybe_fail_scheduled_submission_after_publish(coven_home: &Path) -> Result<()> {
    if scheduled_submission_failpoints()
        .lock()
        .map_err(|_| anyhow::anyhow!("scheduled submission failpoint lock poisoned"))?
        .remove(coven_home)
    {
        anyhow::bail!("injected failure after scheduled proposal publication");
    }
    Ok(())
}

#[cfg(not(test))]
fn maybe_fail_scheduled_submission_after_publish(_coven_home: &Path) -> Result<()> {
    Ok(())
}

fn materialize_diff(
    workspace: &Path,
    edits: &[ward::FileEdit],
    budget: &mut ward::WardEditBudget,
) -> Result<threads::MaterializedDiff> {
    let mut surfaces = Vec::with_capacity(edits.len());
    for edit in edits {
        surfaces.push(threads::SurfaceDiff {
            surface: threads::SurfaceId::new(edit.target.clone()),
            before: read_surface_if_exists_with_budget(workspace, &edit.target, budget)?,
            after: Some(edit.new_contents.clone()),
        });
    }
    threads::MaterializedDiff::try_new(surfaces).map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store;

    #[test]
    fn scheduled_weave_uses_live_principal_binding_and_preserves_protected_writer() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path();
        std::fs::write(workspace.join("SOUL.md"), "# Synthetic identity\n")?;
        std::fs::write(workspace.join("TOOLS.md"), "Synthetic tools\n")?;
        let config = ward::WardConfig::from_toml_str(
            r#"principal_key_fingerprint = "synthetic-current"
protected_surface = ["SOUL.md"]
[[surface]]
path = "SOUL.md"
tier = 0
[[surface]]
path = "TOOLS.md"
tier = 1
"#,
        )?;
        let conn = store::open_store(&workspace.join("coven.sqlite3"))?;
        let stale = threads::WriterId::new("principal:synthetic-retired");
        let anonymous = threads::WriterId::new("anonymous");
        for candidate in [&stale, &anonymous] {
            let state = build_weave_state_for_writer_at(
                &conn,
                "synthetic",
                workspace,
                &config,
                &["TOOLS.md".to_string()],
                false,
                time::OffsetDateTime::UNIX_EPOCH,
                Some(candidate),
            )?;
            let request = |surface: &str, writer: &threads::WriterId| threads::MutationRequest {
                surface: threads::SurfaceId::new(surface),
                writer: writer.clone(),
                channel: threads::Channel::Mutation,
                identity_context: None,
            };
            assert!(
                !threads::validate_fail_closed(&state.weave, &request("SOUL.md", candidate))
                    .permits_write()
            );
            assert_eq!(
                threads::validate_fail_closed(&state.weave, &request("TOOLS.md", candidate))
                    .permits_write(),
                candidate == &anonymous,
            );
            assert!(threads::validate_fail_closed(
                &state.weave,
                &request(
                    "SOUL.md",
                    &threads::WriterId::new("principal:synthetic-current")
                ),
            )
            .permits_write());
        }
        Ok(())
    }

    fn ward_config() -> ward::WardConfig {
        ward::WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val-1"
protected_surface = ["SOUL.md", "IDENTITY.md"]
default_tier = 2

[[surface]]
path = "SOUL.md"
tier = 0

[[surface]]
path = "IDENTITY.md"
tier = 0

[[surface]]
path = "notes/**"
tier = 2
"#,
        )
        .expect("fixture ward config parses")
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        coven_home: PathBuf,
        workspace: PathBuf,
        conn: Connection,
    }

    fn fixture() -> Fixture {
        let temp = tempfile::tempdir().expect("tempdir");
        let coven_home = temp.path().to_path_buf();
        let workspace = coven_home.join("familiars").join("sage");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("SOUL.md"), "# SOUL\nI am Sage.\n").unwrap();
        std::fs::write(workspace.join("IDENTITY.md"), "# IDENTITY\n").unwrap();
        let conn = store::open_store(&coven_home.join("coven.sqlite3")).unwrap();
        Fixture {
            _temp: temp,
            coven_home,
            workspace,
            conn,
        }
    }

    fn signed() -> ward::Authorization {
        ward::Authorization::signed_by("fp-val-1")
    }

    fn identity_config() -> ward::WardConfig {
        ward::WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val-1"
protected_surface = ["SOUL.md", "IDENTITY.md", "MEMORY.md"]

[[identity_invariant]]
fact = "name"
operator = "equals"
expected = "Sage"

[[identity_invariant]]
fact = "person"
operator = "equals"
expected = "Val"

[[identity_invariant]]
fact = "pronouns"
operator = "equals"
expected = "she/her"

[[identity_invariant]]
fact = "purpose"
operator = "includes"
expected = "research"

[[identity_invariant]]
fact = "coven"
operator = "equals"
expected = "OpenCoven"

[[surface]]
path = "SOUL.md"
tier = 0

[[surface]]
path = "IDENTITY.md"
tier = 0

[[surface]]
path = "MEMORY.md"
tier = 0
"#,
        )
        .expect("identity config parses")
    }

    fn minimal_identity_config() -> ward::WardConfig {
        ward::WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val-1"
protected_surface = ["SOUL.md", "IDENTITY.md", "MEMORY.md"]

[[identity_invariant]]
fact = "name"
operator = "equals"
expected = "Sage"

[[identity_invariant]]
fact = "person"
operator = "equals"
expected = "Val"

[[surface]]
path = "SOUL.md"
tier = 0

[[surface]]
path = "IDENTITY.md"
tier = 0

[[surface]]
path = "MEMORY.md"
tier = 0
"#,
        )
        .expect("minimal identity config parses")
    }

    fn canonical_soul(name: &str, purpose: &str) -> String {
        format!("# SOUL\n## I am {name}\nMy purpose is {purpose}.\n")
    }

    fn canonical_identity(name: &str, pronouns: &str) -> String {
        format!("# IDENTITY.md - {name}\n- **Name:** {name}\n- **Pronouns:** {pronouns}\n")
    }

    fn configure_identity_sources(f: &Fixture) {
        std::fs::write(
            f.coven_home.join("familiars.toml"),
            r#"[[familiar]]
id = "sage"
display_name = "Sage"
role = "Research"
description = "Reads and synthesizes."
pronouns = "she/her"
person = "Val"
coven = "OpenCoven"
"#,
        )
        .unwrap();
        std::fs::write(
            f.workspace.join("SOUL.md"),
            canonical_soul("Sage", "research"),
        )
        .unwrap();
        std::fs::write(
            f.workspace.join("IDENTITY.md"),
            canonical_identity("Sage", "she/her"),
        )
        .unwrap();
        std::fs::write(f.workspace.join("MEMORY.md"), "facts stay local\n").unwrap();
    }

    #[test]
    fn hex_to_bytes_rejects_non_ascii_without_panicking() {
        let error = hex_to_bytes("0éx").expect_err("non-ASCII hex must be rejected");
        assert!(error.to_string().contains("non-ASCII"), "{error:#}");
    }

    #[test]
    fn hex_to_bytes_rejects_non_sha256_lengths() {
        let error = hex_to_bytes("abcd").expect_err("non-SHA-256 digest must be rejected");
        assert!(error.to_string().contains("64-character"), "{error:#}");
    }

    #[test]
    fn persist_apply_audit_records_rolls_back_the_batch_on_insert_failure() {
        let mut f = fixture();
        let config = ward_config();
        let ward = ward::Ward::new(f.workspace.clone(), config.clone()).unwrap();
        let report = ward
            .apply(
                &[
                    ward::FileEdit::new("notes/a.md", "one"),
                    ward::FileEdit::new("notes/b.md", "two"),
                ],
                &ward::Authorization::unsigned(),
            )
            .unwrap();
        f.conn
            .execute_batch(
                r#"
                CREATE TRIGGER fail_second_apply_audit
                BEFORE INSERT ON ward_audit
                WHEN NEW.files_touched = '["notes/b.md"]'
                BEGIN
                    SELECT RAISE(ABORT, 'injected second-row failure');
                END;
                "#,
            )
            .unwrap();

        let result =
            persist_apply_audit_records(&mut f.conn, "sage", &f.workspace, &config, &report);
        assert!(result.is_err(), "injected insert failure must surface");
        let count: i64 = f
            .conn
            .query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE event_type = 'apply_audit'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "the failed batch must leave no partial rows");
    }

    #[test]
    fn persist_apply_audit_records_uses_an_existing_transaction() {
        let f = fixture();
        let config = ward_config();
        let ward = ward::Ward::new(f.workspace.clone(), config.clone()).unwrap();
        let report = ward
            .apply(
                &[
                    ward::FileEdit::new("notes/a.md", "one"),
                    ward::FileEdit::new("notes/b.md", "two"),
                ],
                &ward::Authorization::unsigned(),
            )
            .unwrap();

        f.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        persist_apply_audit_records_on_connection(
            &f.conn,
            "sage",
            &f.workspace,
            &config,
            &report,
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
        let count_in_transaction: i64 = f
            .conn
            .query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE event_type = 'apply_audit'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count_in_transaction, 2,
            "existing transaction should see both audit rows"
        );

        f.conn.execute_batch("ROLLBACK").unwrap();
        let count_after_rollback: i64 = f
            .conn
            .query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE event_type = 'apply_audit'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count_after_rollback, 0,
            "outer rollback must still control audit rows"
        );
    }

    fn soul_edit() -> Vec<ward::FileEdit> {
        vec![ward::FileEdit::new(
            "SOUL.md",
            "# SOUL\nI am Sage, updated.\n",
        )]
    }

    #[test]
    fn first_sight_bootstraps_baseline_and_permits_principal() {
        let f = fixture();
        let report = gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &ward_config(),
                edits: &soul_edit(),
                gated_targets: &["SOUL.md".to_string()],
                authorization: &signed(),
            },
        )
        .unwrap();

        assert!(
            matches!(report.outcome, GateOutcome::Permitted),
            "{report:?}"
        );
        assert!(matches!(
            report.verdicts[0].1,
            threads::Verdict::Permit { .. }
        ));
        // Baselines recorded for both declared protected surfaces.
        let count: i64 = f
            .conn
            .query_row(
                "SELECT COUNT(*) FROM ward_manifest WHERE familiar_id = 'sage'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
        // Verdict audited.
        let decision: String = f
            .conn
            .query_row(
                "SELECT decision FROM ward_audit WHERE familiar_id = 'sage'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(decision, "permit");
    }

    #[test]
    fn unsigned_writer_is_not_bound_and_rejects() {
        let f = fixture();
        let report = gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &ward_config(),
                edits: &soul_edit(),
                gated_targets: &["SOUL.md".to_string()],
                authorization: &ward::Authorization::unsigned(),
            },
        )
        .unwrap();
        assert!(
            matches!(report.outcome, GateOutcome::Rejected),
            "{report:?}"
        );
        let decision: String = f
            .conn
            .query_row(
                "SELECT decision FROM ward_audit WHERE familiar_id = 'sage'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(decision, "reject:writer_not_bound");
    }

    #[test]
    fn out_of_band_drift_stages_proposal_to_pending() {
        let f = fixture();
        let config = ward_config();
        // First request bootstraps baselines.
        gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &config,
                edits: &soul_edit(),
                gated_targets: &["SOUL.md".to_string()],
                authorization: &signed(),
            },
        )
        .unwrap();

        // SOUL.md drifts outside the authority path.
        std::fs::write(f.workspace.join("SOUL.md"), "# SOUL\nI am Mallory.\n").unwrap();

        let report = gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &config,
                edits: &soul_edit(),
                gated_targets: &["SOUL.md".to_string()],
                authorization: &signed(),
            },
        )
        .unwrap();

        let GateOutcome::Staged { pending_path, .. } = &report.outcome else {
            panic!("expected Staged, got {report:?}");
        };
        assert!(pending_path.exists(), "pending file must exist");
        let raw = std::fs::read(pending_path).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(value["probes"][0]["target"], "SOUL.md");
        assert_eq!(value["probes"][0]["surface"], "SOUL.md");
        assert_eq!(value["probes"][0]["status"], "unscored");
        assert_eq!(value["probes"][0]["results"], serde_json::json!([]));
        crate::api::validate_proposal_envelope_preflight(&raw)
            .expect("legacy staged proposal must pass envelope preflight");
        assert_eq!(
            value["probes"][0]["baselineSha256"].as_str().map(str::len),
            Some(64)
        );
        let staged: threads::PendingProposal = serde_json::from_slice(&raw).unwrap();
        assert_eq!(staged.edits.len(), 1);
        assert!(matches!(
            staged.fray,
            threads::FrayOrSnap::Frayed {
                reason: threads::FrayReason::ManifestEntryMismatch,
                ..
            }
        ));
        // The protected surface itself is untouched by staging.
        let disk = std::fs::read_to_string(f.workspace.join("SOUL.md")).unwrap();
        assert!(
            disk.contains("Mallory"),
            "staging must not write the surface"
        );
        // Audit trail carries the degrade decision.
        let decision: String = f
            .conn
            .query_row(
                "SELECT decision FROM ward_audit WHERE familiar_id='sage' \
                 ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(decision, "degrade_to_proposal");
    }

    #[test]
    fn drift_on_sibling_surface_does_not_stop_healthy_surface() {
        // §2.2: degradation is local to the drifted surface; the familiar
        // continues on other surfaces.
        let f = fixture();
        let config = ward_config();
        gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &config,
                edits: &soul_edit(),
                gated_targets: &["SOUL.md".to_string()],
                authorization: &signed(),
            },
        )
        .unwrap();
        std::fs::write(f.workspace.join("IDENTITY.md"), "# IDENTITY drifted\n").unwrap();

        let report = gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &config,
                edits: &soul_edit(),
                gated_targets: &["SOUL.md".to_string()],
                authorization: &signed(),
            },
        )
        .unwrap();
        assert!(
            matches!(report.outcome, GateOutcome::Permitted),
            "healthy SOUL.md must permit despite IDENTITY.md drift: {report:?}"
        );
    }

    #[test]
    fn ward_audit_is_append_only() {
        let f = fixture();
        gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &ward_config(),
                edits: &soul_edit(),
                gated_targets: &["SOUL.md".to_string()],
                authorization: &signed(),
            },
        )
        .unwrap();
        // RFC-0001 §5.6: entries MUST NOT be deleted or modified — the store
        // itself aborts, regardless of caller discipline.
        let update = f
            .conn
            .execute("UPDATE ward_audit SET decision = 'permit'", []);
        assert!(update.is_err(), "UPDATE must abort on ward_audit");
        let delete = f.conn.execute("DELETE FROM ward_audit", []);
        assert!(delete.is_err(), "DELETE must abort on ward_audit");
    }

    #[test]
    fn traversal_surface_declarations_are_refused() {
        // Review finding: ward.toml surface strings were joined with
        // PathBuf::push, so ".." segments escaped the workspace. read_surface
        // now fail-closes on escaping declarations instead of reading outside.
        let f = fixture();
        // A secret outside the workspace that a poisoned ward.toml might aim at.
        std::fs::write(f.coven_home.join("outside-secret.txt"), b"secret").unwrap();

        let config = ward::WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val-1"
protected_surface = ["../outside-secret.txt"]
default_tier = 2

[[surface]]
path = "../outside-secret.txt"
tier = 0
"#,
        )
        .expect("ward config parses");

        let err = gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &config,
                edits: &[ward::FileEdit::new("SOUL.md", "x")],
                gated_targets: &["SOUL.md".to_string()],
                authorization: &signed(),
            },
        )
        .expect_err("escaping declaration must refuse");
        let message = format!("{err:#}");
        assert!(
            message.contains("path-escaping"),
            "error should name the escape: {message}"
        );
        // Fail-closed: nothing was audited or staged for the refused run.
        let count: i64 = f
            .conn
            .query_row("SELECT COUNT(*) FROM ward_audit", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn absolute_surface_declarations_are_refused() {
        let f = fixture();
        let config = ward::WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val-1"
protected_surface = ["/etc/hosts"]
default_tier = 2

[[surface]]
path = "/etc/hosts"
tier = 0
"#,
        )
        .expect("ward config parses");

        let err = gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &config,
                edits: &[ward::FileEdit::new("SOUL.md", "x")],
                gated_targets: &["SOUL.md".to_string()],
                authorization: &signed(),
            },
        )
        .expect_err("absolute declaration must refuse");
        assert!(format!("{err:#}").contains("workspace-relative"));
    }

    #[test]
    fn identity_predicate_rejects_failed_candidate_fact() {
        let f = fixture();
        configure_identity_sources(&f);

        let report = gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &identity_config(),
                edits: &[ward::FileEdit::new(
                    "SOUL.md",
                    canonical_soul("Sage", "sabotage"),
                )],
                gated_targets: &["SOUL.md".to_string()],
                authorization: &signed(),
            },
        )
        .unwrap();

        assert!(
            matches!(report.outcome, GateOutcome::Rejected),
            "{report:?}"
        );
        let threads::Verdict::Reject {
            reason: threads::RejectReason::WeaveBroken { reason },
            ..
        } = &report.verdicts[0].1
        else {
            panic!("expected weave-broken reject, got {report:?}");
        };
        assert!(reason.contains("Purpose identity invariant did not hold"));
    }

    #[test]
    fn identity_predicate_fails_closed_when_authoritative_source_is_missing() {
        let f = fixture();
        configure_identity_sources(&f);
        std::fs::remove_file(f.workspace.join("IDENTITY.md")).unwrap();

        let report = gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &minimal_identity_config(),
                edits: &[ward::FileEdit::new(
                    "SOUL.md",
                    canonical_soul("Sage", "research"),
                )],
                gated_targets: &["SOUL.md".to_string()],
                authorization: &signed(),
            },
        )
        .unwrap();

        assert!(
            matches!(report.outcome, GateOutcome::Rejected),
            "{report:?}"
        );
        let threads::Verdict::Reject {
            reason: threads::RejectReason::WeaveBroken { reason },
            ..
        } = &report.verdicts[0].1
        else {
            panic!("expected weave-broken reject, got {report:?}");
        };
        assert!(reason.contains("identity fact unavailable"));
    }

    #[test]
    fn identity_predicate_fails_closed_on_ambiguous_roster_sources() {
        let f = fixture();
        std::fs::write(
            f.coven_home.join("familiars.toml"),
            r#"[[familiar]]
id = "sage"
display_name = "Sage"
role = "Research"
description = "Reads and synthesizes."
pronouns = "she/her"
person = "Val"
coven = "OpenCoven"

[[familiar]]
id = "sage"
display_name = "Sage"
role = "Research"
description = "Conflicting duplicate."
pronouns = "she/her"
person = "Val"
coven = "OpenCoven"
"#,
        )
        .unwrap();
        std::fs::write(
            f.workspace.join("SOUL.md"),
            canonical_soul("Sage", "research"),
        )
        .unwrap();
        std::fs::write(
            f.workspace.join("IDENTITY.md"),
            canonical_identity("Sage", "she/her"),
        )
        .unwrap();

        let report = gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &minimal_identity_config(),
                edits: &[ward::FileEdit::new(
                    "SOUL.md",
                    canonical_soul("Sage", "research"),
                )],
                gated_targets: &["SOUL.md".to_string()],
                authorization: &signed(),
            },
        )
        .unwrap();

        assert!(
            matches!(report.outcome, GateOutcome::Rejected),
            "{report:?}"
        );
        let threads::Verdict::Reject {
            reason: threads::RejectReason::WeaveBroken { reason },
            ..
        } = &report.verdicts[0].1
        else {
            panic!("expected weave-broken reject, got {report:?}");
        };
        assert!(reason.contains("identity fact unavailable"));
    }

    #[test]
    fn identity_predicate_reads_complete_candidate_for_unrelated_targets() {
        let f = fixture();
        configure_identity_sources(&f);
        std::fs::write(
            f.workspace.join("SOUL.md"),
            canonical_soul("Sage", "sabotage"),
        )
        .unwrap();

        let report = gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &identity_config(),
                edits: &[ward::FileEdit::new("MEMORY.md", "refined notes\n")],
                gated_targets: &["MEMORY.md".to_string()],
                authorization: &signed(),
            },
        )
        .unwrap();

        assert!(
            matches!(report.outcome, GateOutcome::Rejected),
            "{report:?}"
        );
        let threads::Verdict::Reject {
            reason: threads::RejectReason::WeaveBroken { reason },
            ..
        } = &report.verdicts[0].1
        else {
            panic!("expected weave-broken reject, got {report:?}");
        };
        assert!(reason.contains("Purpose identity invariant did not hold"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_surface_is_refused_not_followed() {
        let f = fixture();
        std::fs::write(f.coven_home.join("outside-secret.txt"), b"secret").unwrap();
        std::os::unix::fs::symlink(
            f.coven_home.join("outside-secret.txt"),
            f.workspace.join("LINKED.md"),
        )
        .unwrap();
        let config = ward::WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val-1"
protected_surface = ["LINKED.md"]
default_tier = 2

[[surface]]
path = "LINKED.md"
tier = 0
"#,
        )
        .expect("ward config parses");

        let err = gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &config,
                edits: &[ward::FileEdit::new("LINKED.md", "x")],
                gated_targets: &["LINKED.md".to_string()],
                authorization: &signed(),
            },
        )
        .expect_err("symlinked surface must refuse");
        assert!(format!("{err:#}").contains("not a regular file"));
    }

    #[cfg(unix)]
    #[test]
    fn special_file_surface_is_refused_without_opening_it() {
        use std::os::unix::ffi::OsStrExt;

        let f = fixture();
        let fifo = f.workspace.join("events.fifo");
        let fifo = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);

        let err = read_surface_if_exists(&f.workspace, "events.fifo")
            .expect_err("a FIFO must refuse before attempting a read");
        assert!(format!("{err:#}").contains("not a regular file"));
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_surface_replacement_never_reads_outside_the_workspace() {
        let f = fixture();
        let surface = f.workspace.join("REVIEWED.md");
        let parked = f.workspace.join("REVIEWED.parked");
        let outside = f.coven_home.join("outside-secret.txt");
        std::fs::write(&surface, b"inside").unwrap();
        std::fs::write(&outside, b"outside-secret").unwrap();

        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..250 {
                    if std::fs::rename(&surface, &parked).is_ok() {
                        let _ = std::os::unix::fs::symlink(&outside, &surface);
                        let _ = std::fs::remove_file(&surface);
                        let _ = std::fs::rename(&parked, &surface);
                    }
                }
            });

            for _ in 0..250 {
                if let Ok(Some(bytes)) = read_surface_if_exists(&f.workspace, "REVIEWED.md") {
                    assert_eq!(bytes, b"inside");
                }
            }
        });
    }

    #[test]
    fn surface_read_is_bounded_to_the_baseline_cap() {
        let f = fixture();
        let bytes = vec![b'x'; MAX_SURFACE_BYTES as usize + 1];
        std::fs::write(f.workspace.join("large.md"), bytes).unwrap();

        let err = read_surface_if_exists(&f.workspace, "large.md")
            .expect_err("an oversized surface must refuse");
        assert!(format!("{err:#}").contains("baseline cap"));
    }

    #[cfg(unix)]
    #[test]
    fn surface_read_errors_name_only_the_logical_surface() {
        let f = fixture();
        let missing_workspace = f.workspace.join("missing-workspace");
        let missing_error = read_surface_if_exists(&missing_workspace, "reviewed/file.md")
            .expect_err("a missing workspace must refuse");
        let missing_rendered = format!("{missing_error:#}");

        assert!(
            missing_rendered.contains("reviewed/file.md"),
            "{missing_rendered}"
        );
        assert!(
            !missing_rendered.contains(&missing_workspace.display().to_string()),
            "{missing_rendered}"
        );

        std::os::unix::fs::symlink("loop", f.workspace.join("loop")).unwrap();

        let error = read_surface_if_exists(&f.workspace, "loop/file.md")
            .expect_err("a symlink loop must refuse");
        let rendered = format!("{error:#}");

        assert!(rendered.contains("loop/file.md"), "{rendered}");
        assert!(
            !rendered.contains(&f.workspace.display().to_string()),
            "{rendered}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_ancestor_directory_is_refused() {
        // Second review pass finding: symlink_metadata only guards the final
        // component. A surface like "linkdir/secret" where linkdir points
        // outside the workspace must refuse, not read through the link.
        let f = fixture();
        let outside = f.coven_home.join("outside-dir");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.md"), b"secret").unwrap();
        std::os::unix::fs::symlink(&outside, f.workspace.join("linkdir")).unwrap();

        let config = ward::WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val-1"
protected_surface = ["linkdir/secret.md"]
default_tier = 2

[[surface]]
path = "linkdir/secret.md"
tier = 0
"#,
        )
        .expect("ward config parses");

        let err = gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &config,
                edits: &[ward::FileEdit::new("linkdir/secret.md", "x")],
                gated_targets: &["linkdir/secret.md".to_string()],
                authorization: &signed(),
            },
        )
        .expect_err("symlinked ancestor must refuse");
        assert!(
            format!("{err:#}").contains("symlinked or non-directory ancestor"),
            "error should name the escape: {err:#}"
        );
    }

    #[test]
    fn absent_nested_surface_still_baselines_as_empty() {
        // The ancestor walk must not break the absent-surface convention: a
        // declared surface in a not-yet-created subdirectory (no symlinks)
        // baselines as empty rather than erroring.
        let f = fixture();
        let config = ward::WardConfig::from_toml_str(
            r#"
principal_key_fingerprint = "fp-val-1"
protected_surface = ["SOUL.md", "identity/CORE.md"]
default_tier = 2

[[surface]]
path = "SOUL.md"
tier = 0

[[surface]]
path = "identity/CORE.md"
tier = 0
"#,
        )
        .expect("ward config parses");

        let report = gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &config,
                edits: &soul_edit(),
                gated_targets: &["SOUL.md".to_string()],
                authorization: &signed(),
            },
        )
        .unwrap();
        assert!(
            matches!(report.outcome, GateOutcome::Permitted),
            "{report:?}"
        );
    }

    #[test]
    fn no_protected_targets_is_a_noop_permit() {
        let f = fixture();
        let report = gate_protected_edits(
            &f.conn,
            &GateRequest {
                coven_home: &f.coven_home,
                familiar_id: "sage",
                workspace: &f.workspace,
                config: &ward_config(),
                edits: &[ward::FileEdit::new("notes/today.md", "hello")],
                gated_targets: &[],
                authorization: &ward::Authorization::unsigned(),
            },
        )
        .unwrap();
        assert!(matches!(report.outcome, GateOutcome::Permitted));
        assert!(report.verdicts.is_empty());
        let count: i64 = f
            .conn
            .query_row("SELECT COUNT(*) FROM ward_audit", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "editable-tier writes are not the weave's lane");
    }
}
