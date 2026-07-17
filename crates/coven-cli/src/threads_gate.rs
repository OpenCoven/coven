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

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use coven_threads_core as threads;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

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
    /// downstream. Empty means no protected target: the gate is a no-op
    /// `Permitted` — editable-tier writes are the Ward tiers' lane.
    pub gated_targets: &'a [String],
    /// The proposal's authorization.
    pub authorization: &'a ward::Authorization,
}

/// The daemon's current weave state for one familiar, as used by the validator.
pub(crate) struct WeaveState {
    pub familiar_uuid: threads::FamiliarId,
    pub weave: threads::Weave,
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
    if gated_targets.is_empty() {
        return Ok(GateReport {
            verdicts: Vec::new(),
            outcome: GateOutcome::Permitted,
        });
    }

    let request_writer = match &authorization.principal_signature_fingerprint {
        Some(fp) => threads::WriterId::new(format!("principal:{fp}")),
        None => threads::WriterId::new("client:unsigned"),
    };
    let now = time::OffsetDateTime::now_utc();
    let state = build_weave_state(conn, familiar_id, workspace, config, gated_targets, true)?;
    let familiar_uuid = state.familiar_uuid;
    let weave = state.weave;

    // Validate every gated target; audit every verdict (RFC-0001 §5.6).
    let mut verdicts = Vec::with_capacity(gated_targets.len());
    let mut degraded: Option<(threads::ThreadId, threads::FrayOrSnap)> = None;
    let mut rejected = false;
    for target in gated_targets {
        let request = threads::MutationRequest {
            surface: threads::SurfaceId::new(target.clone()),
            writer: request_writer.clone(),
            channel: threads::Channel::Mutation,
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
        let (pending_path, proposal_id) = stage_pending_proposal(
            coven_home,
            &familiar_uuid,
            &request_writer,
            thread_id,
            fray,
            edits,
            now,
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
    let familiar_uuid = familiar_weave_id(familiar_id);
    let principal_writer =
        threads::WriterId::new(format!("principal:{}", config.principal_key_fingerprint));

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
    let now = time::OffsetDateTime::now_utc();
    let mut woven = Vec::with_capacity(surfaces.len());
    for surface in &surfaces {
        let surface_id = threads::SurfaceId::new(surface.clone());
        let disk = read_surface(workspace, surface)?;
        let current_hash = threads::manifest_entry_hash(&surface_id, &disk);

        let baseline = load_baseline(conn, familiar_id, surface)?;
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

        let mut thread = threads::Thread {
            id: threads::ThreadId::new(),
            surface: surface_id.clone(),
            writer: principal_writer.clone(),
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

    let pattern = threads::AllSurfacesHoldOnChannels {
        name: format!("{familiar_id}-protected-surface"),
        surfaces: surfaces
            .iter()
            .map(|s| threads::SurfaceId::new(s.clone()))
            .collect(),
        channels: PROTECTED_CHANNELS.to_vec(),
    };
    let weave = threads::Weave::new(
        threads::WeaveId::new(),
        familiar_uuid,
        woven,
        Box::new(pattern),
        None,
    )
    .context("weaving protected surfaces")?;

    Ok(WeaveState {
        familiar_uuid,
        weave,
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
/// directories included), and the read is capped so a pathological
/// declaration cannot balloon memory.
fn read_surface(workspace: &Path, surface: &str) -> Result<Vec<u8>> {
    const MAX_SURFACE_BYTES: u64 = 16 * 1024 * 1024;

    if surface.starts_with('/') || surface.starts_with('\\') {
        anyhow::bail!("protected surface `{surface}` must be workspace-relative");
    }
    let mut path = workspace.to_path_buf();
    for segment in surface.split('/').filter(|s| !s.is_empty()) {
        if segment == ".." || segment == "." || segment.contains('\\') || segment.contains(':') {
            anyhow::bail!(
                "protected surface `{surface}` contains a path-escaping segment; \
                 declarations must stay inside the familiar workspace"
            );
        }
        path.push(segment);
    }

    // Intermediate symlinks: `symlink_metadata` below only protects the final
    // component, so canonicalize the deepest *existing* ancestor and require
    // it to stay inside the canonical workspace (second review pass finding —
    // `linkdir/secret` with `linkdir` pointing outside must refuse).
    let canonical_workspace = workspace.canonicalize().with_context(|| {
        format!(
            "familiar workspace `{}` is not resolvable",
            workspace.display()
        )
    })?;
    let mut ancestor = path.parent();
    let deepest_existing = loop {
        match ancestor {
            Some(candidate) => match candidate.canonicalize() {
                Ok(resolved) => break Some(resolved),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    ancestor = candidate.parent();
                }
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!("resolving ancestor of surface {}", path.display())
                    })
                }
            },
            None => break None,
        }
    };
    match deepest_existing {
        Some(resolved) if resolved.starts_with(&canonical_workspace) => {}
        _ => anyhow::bail!(
            "protected surface `{surface}` resolves outside the familiar workspace \
             (symlinked ancestor?); declarations must stay inside it"
        ),
    }

    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        // An absent protected file baselines as empty: creating it later is
        // drift like any other content change.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| format!("inspecting surface {}", path.display()))
        }
    };
    // Only regular files are hashable surfaces; a symlinked or special-file
    // surface is refused rather than followed out of the workspace.
    if !metadata.is_file() {
        anyhow::bail!("protected surface `{surface}` is not a regular file inside the workspace");
    }
    if metadata.len() > MAX_SURFACE_BYTES {
        anyhow::bail!(
            "protected surface `{surface}` exceeds the {MAX_SURFACE_BYTES}-byte baseline cap"
        );
    }
    std::fs::read(&path).with_context(|| format!("reading surface {}", path.display()))
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

fn load_baseline(conn: &Connection, familiar_id: &str, surface: &str) -> Result<Option<Vec<u8>>> {
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
    let manifest_id = load_or_create_manifest_id(conn, familiar_id)?;
    let surface_id = threads::SurfaceId::new(surface.to_string());
    let disk = read_surface(workspace, surface)?;
    let entry_hash = threads::manifest_entry_hash(&surface_id, &disk);
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

fn stage_pending_proposal(
    coven_home: &Path,
    familiar_uuid: &threads::FamiliarId,
    writer: &threads::WriterId,
    thread_id: threads::ThreadId,
    fray: threads::FrayOrSnap,
    edits: &[ward::FileEdit],
    now: time::OffsetDateTime,
) -> Result<(PathBuf, String)> {
    let proposal = threads::PendingProposal {
        id: threads::ProposalId::new(),
        familiar_id: *familiar_uuid,
        writer: writer.clone(),
        channel: threads::Channel::Mutation,
        thread_id,
        fray,
        edits: edits
            .iter()
            .map(|edit| threads::StagedEdit {
                surface: threads::SurfaceId::new(edit.target.clone()),
                contents: threads::StagedContents::from_bytes(&edit.new_contents),
            })
            .collect(),
        staged_at: now,
    };

    let pending_dir = coven_home.join("pending");
    std::fs::create_dir_all(&pending_dir)
        .with_context(|| format!("creating {}", pending_dir.display()))?;
    let path = pending_dir.join(proposal.file_name());
    let body = serde_json::to_vec_pretty(&proposal).context("serializing pending proposal")?;
    // Atomic sibling-staged write, same discipline as the Ward's own writes.
    let staged = path.with_extension("json.staged");
    std::fs::write(&staged, &body)
        .with_context(|| format!("staging pending proposal at {}", staged.display()))?;
    std::fs::rename(&staged, &path)
        .with_context(|| format!("committing pending proposal at {}", path.display()))?;
    Ok((path, proposal.id.0.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store;

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
        let staged: threads::PendingProposal =
            serde_json::from_slice(&std::fs::read(pending_path).unwrap()).unwrap();
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
            format!("{err:#}").contains("resolves outside"),
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
