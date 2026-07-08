//! The Ward — runtime enforcement of a familiar's protected surface.
//!
//! The Ward is the identity-layer authority described by the Familiar Contract
//! (RFC-0001) and the Coven Familiar Spec
//! (`specs/coven-familiar-spec/PRODUCT.md`). It sits between a familiar's
//! self-improvement loop and its own identity files, refusing modifications
//! that would change *who the familiar is* while allowing the large editable
//! surface that governs *how well it works*.
//!
//! The full design specifies four gates:
//!
//! 1. **Authorization verification** — a modification to the Tier 0 protected
//!    surface requires a signature from the familiar's principal.
//! 2. **Surface discrimination** — canonical path materialization. A proposal
//!    that nominally targets an editable path but resolves (via `..`, symlink,
//!    hardlink, or case collision) to a protected path is caught here and
//!    classified by its *real* target.
//! 3. **Identity coherence validation** — Tier 0/1 modifications must pass the
//!    familiar's probe set. *(Requires model/regression infrastructure; not
//!    implemented in this module — see [`Verdict::RequiresCoherenceReview`].)*
//! 4. **Audit logging** — modifications are recorded to an append-only log.
//!    [`Ward::apply`] emits a tamper-evident [`AuditRecord`] (before/after
//!    SHA-256 content hashes) for every Tier 2 write; persisting those records
//!    to the daemon's store is a follow-up.
//!
//! This module implements the two **deterministic** gates — 1 and 2 — which are
//! the load-bearing structural checks. It has no dependency on the language
//! model, and every decision it makes is a pure function of the declared
//! surface and the proposal. [`Ward::apply`] is the **fail-closed enforcement
//! boundary**: it routes every write through Gates 1–2, refuses or holds the
//! whole proposal as a unit if any target is blocked or needs coherence review,
//! and only then writes — emitting Gate 4 audit records. Gate 3 (coherence)
//! remains deferred: a change that needs it is *held*, never written.
//!
//! ## Fail-closed posture
//!
//! Consistent with the daemon's authority model (the daemon is the sole
//! authority; a working directory must canonicalize *inside* its root), the
//! Ward fails closed: any proposal whose target cannot be safely resolved
//! inside the familiar home — traversal escape, symlink escape, or a
//! case-insensitive collision with a protected path — is [`Verdict::Blocked`].

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

/// Trust tier of a path within a familiar's surface.
///
/// Lower is more protected. Numbering matches the Coven Familiar Spec
/// (`identity.*.tier`): Tier 0 is the protected surface `S_p(F)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum Tier {
    /// Protected surface. Modifications require principal authorization (Gate 1).
    Protected = 0,
    /// Ward-reviewed surface. Modifications require coherence review (Gate 3).
    Reviewed = 1,
    /// Auto-approved with logging (Gate 4).
    Logged = 2,
    /// Unrestricted scratch. No gate applies.
    Free = 3,
}

impl Tier {
    fn as_u8(self) -> u8 {
        self as u8
    }
}

impl From<Tier> for u8 {
    fn from(tier: Tier) -> Self {
        tier.as_u8()
    }
}

impl TryFrom<u8> for Tier {
    type Error = String;

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(Tier::Protected),
            1 => Ok(Tier::Reviewed),
            2 => Ok(Tier::Logged),
            3 => Ok(Tier::Free),
            other => Err(format!("invalid ward tier {other}; expected 0..=3")),
        }
    }
}

/// One declared region of a familiar's surface.
///
/// `path` is a glob relative to the familiar home. A trailing `/` is treated as
/// "everything under this directory".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceEntry {
    /// Glob pattern, relative to the familiar home, using forward slashes.
    pub path: String,
    /// Trust tier assigned to matching paths.
    pub tier: Tier,
}

/// A familiar's Ward configuration — the declared surface plus the principal
/// binding that authorizes Tier 0 changes.
///
/// Loadable from a `ward.toml` (see [`WardConfig::from_toml_str`]). The type is
/// also `serde`-portable to JSON so it can ride inside a `familiar.yaml`
/// identity block once a YAML loader feeds it in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WardConfig {
    /// Fingerprint of the principal's signing key. A Tier 0 modification is
    /// authorized only if its proposal carries a signature with this
    /// fingerprint (Gate 1).
    pub principal_key_fingerprint: String,
    /// Declared surface regions.
    #[serde(default)]
    pub surface: Vec<SurfaceEntry>,
    /// The Tier 0 paths, enumerated explicitly. Validated to match exactly the
    /// set of `tier = 0` entries (Familiar Spec validation rule 6).
    #[serde(default)]
    pub protected_surface: Vec<String>,
    /// Tier assigned to a cleanly-resolved path inside the home that matches no
    /// declared entry. Defaults to [`Tier::Logged`] so the editable surface
    /// stays large while unknown edits are still recorded — not frozen.
    #[serde(default = "default_unmatched_tier")]
    pub default_tier: Tier,
}

fn default_unmatched_tier() -> Tier {
    Tier::Logged
}

impl WardConfig {
    /// Parse a `ward.toml` document.
    pub fn from_toml_str(input: &str) -> Result<Self> {
        let config: WardConfig = toml::from_str(input).context("failed to parse ward.toml")?;
        config.validate()?;
        Ok(config)
    }

    /// Validate internal consistency of the configuration.
    ///
    /// - `protected_surface` MUST enumerate exactly the `tier = 0` entries.
    /// - the principal key fingerprint MUST be non-empty.
    pub fn validate(&self) -> Result<()> {
        if self.principal_key_fingerprint.trim().is_empty() {
            bail!("ward config has an empty principal_key_fingerprint; a familiar with no principal cannot be warded");
        }

        let declared_tier0: BTreeSet<&str> = self
            .surface
            .iter()
            .filter(|entry| entry.tier == Tier::Protected)
            .map(|entry| entry.path.as_str())
            .collect();
        let enumerated: BTreeSet<&str> =
            self.protected_surface.iter().map(String::as_str).collect();

        if declared_tier0 != enumerated {
            let missing: Vec<&str> = declared_tier0.difference(&enumerated).copied().collect();
            let extra: Vec<&str> = enumerated.difference(&declared_tier0).copied().collect();
            bail!(
                "protected_surface must enumerate exactly the tier-0 paths; \
                 missing from protected_surface: {missing:?}; \
                 not declared tier-0: {extra:?}"
            );
        }

        Ok(())
    }
}

/// Whether a proposal carries principal authorization for Tier 0 changes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Authorization {
    /// Fingerprint of the key that signed this proposal, if any.
    pub principal_signature_fingerprint: Option<String>,
}

impl Authorization {
    /// A proposal that carries a principal signature with the given fingerprint.
    pub fn signed_by(fingerprint: impl Into<String>) -> Self {
        Self {
            principal_signature_fingerprint: Some(fingerprint.into()),
        }
    }

    /// A proposal with no principal authorization.
    pub fn unsigned() -> Self {
        Self::default()
    }
}

/// A proposed modification the Ward must adjudicate.
#[derive(Debug, Clone)]
pub struct Proposal {
    /// Target paths, relative to the familiar home, that the modification would
    /// write. Forward slashes.
    pub targets: Vec<String>,
    /// Authorization accompanying the proposal.
    pub authorization: Authorization,
}

/// The Ward's ruling on a single target path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The change may be applied without further gates (Tier 3).
    Allow,
    /// The change is allowed but MUST be recorded to the audit log (Tier 2,
    /// Gate 4).
    AllowWithLog,
    /// A Tier 1 change: must pass identity-coherence review (Gate 3) before it
    /// can be applied. Not adjudicated by this module.
    RequiresCoherenceReview,
    /// A Tier 0 change carrying valid principal authorization: authorized by
    /// Gate 1, but still subject to coherence (Gate 3) and audit (Gate 4).
    AuthorizedProtectedChange,
    /// Refused. `reason` explains which gate rejected it.
    Blocked { reason: BlockReason },
}

impl Verdict {
    /// Whether this verdict, on its own, refuses the change.
    pub fn is_blocked(&self) -> bool {
        matches!(self, Verdict::Blocked { .. })
    }
}

/// Why the Ward refused a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockReason {
    /// The target escapes the familiar home via `..` traversal.
    TraversalEscape,
    /// The target resolves outside the familiar home via a symlink.
    SymlinkEscape,
    /// The target collides case-insensitively with a protected path (defends
    /// case-insensitive filesystems).
    CaseCollision { protected_as: String },
    /// A Tier 0 modification lacking a valid principal signature.
    Unauthorized,
    /// The target could not be resolved (I/O error during materialization).
    Unresolvable { detail: String },
}

impl std::fmt::Display for BlockReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockReason::TraversalEscape => {
                write!(f, "target escapes the familiar home via `..` traversal")
            }
            BlockReason::SymlinkEscape => {
                write!(f, "target resolves outside the familiar home via a symlink")
            }
            BlockReason::CaseCollision { protected_as } => write!(
                f,
                "target collides case-insensitively with protected path `{protected_as}`"
            ),
            BlockReason::Unauthorized => write!(
                f,
                "tier-0 protected surface modification requires a valid principal signature"
            ),
            BlockReason::Unresolvable { detail } => {
                write!(f, "target could not be resolved: {detail}")
            }
        }
    }
}

/// The Ward's decision about one target path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The target as supplied in the proposal.
    pub target: String,
    /// The home-relative path the target actually resolves to (Gate 2 output).
    pub resolved: String,
    /// The tier the resolved path was classified into.
    pub tier: Tier,
    /// The ruling.
    pub verdict: Verdict,
}

/// The Ward's decision about a whole proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Per-target decisions.
    pub decisions: Vec<Decision>,
}

impl Outcome {
    /// Whether any target was blocked. A proposal is refused as a unit if any
    /// of its targets is refused.
    pub fn is_blocked(&self) -> bool {
        self.decisions.iter().any(|d| d.verdict.is_blocked())
    }

    /// The blocked decisions, if any.
    pub fn blocked(&self) -> impl Iterator<Item = &Decision> {
        self.decisions.iter().filter(|d| d.verdict.is_blocked())
    }
}

/// A single file write the Ward is asked to apply.
///
/// The caller supplies the desired end state (full contents), not a patch: a
/// patch parser would be additional attack surface *inside* the security
/// boundary, and full-content writes make the diff a pure function of on-disk
/// state and the proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdit {
    /// Home-relative target, forward slashes.
    pub target: String,
    /// The full contents the file should have after the apply.
    pub new_contents: Vec<u8>,
}

impl FileEdit {
    /// A write of `new_contents` to the home-relative `target`.
    pub fn new(target: impl Into<String>, new_contents: impl Into<Vec<u8>>) -> Self {
        Self {
            target: target.into(),
            new_contents: new_contents.into(),
        }
    }
}

/// What the Ward did — or refused to do — with one edit in an apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Written to disk. The edit cleared every applicable gate (Tier 2/3).
    Applied,
    /// Not written. The proposal requires Gate 3 coherence review — it targets
    /// Tier 1, or a Tier 0 path authorized by Gate 1 but not yet
    /// coherence-cleared. The whole proposal is held until review.
    HeldForCoherence,
    /// Not written. Some target in the proposal was Blocked (Gate 1/2), so the
    /// proposal is refused as a unit.
    Refused,
}

/// A Gate 4 audit record for a change the Ward wrote.
///
/// The before/after content hashes make the record tamper-evident. (The spec
/// leaves the canonical hash open: BLAKE3 is the eventual recommendation;
/// SHA-256 is used here as the documented fallback.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    /// The target as supplied in the edit.
    pub target: String,
    /// The home-relative path actually written (Gate 2 output).
    pub resolved: String,
    /// Tier of the written path.
    pub tier: Tier,
    /// SHA-256 (hex) of the prior contents, or `None` if the file was created.
    pub prev_sha256: Option<String>,
    /// SHA-256 (hex) of the written contents.
    pub next_sha256: String,
    /// Number of bytes written.
    pub bytes_written: usize,
}

/// The Ward's ruling and action for a single edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedChange {
    /// The adjudication that produced this action.
    pub decision: Decision,
    /// What the Ward did with the edit.
    pub disposition: Disposition,
    /// Present iff the edit was written *and* its tier requires logging
    /// (Tier 2). Tier 3 (free) writes carry no audit record.
    pub audit: Option<AuditRecord>,
}

/// The result of [`Ward::apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    /// Per-edit outcomes, in proposal order.
    pub changes: Vec<AppliedChange>,
}

impl ApplyReport {
    /// Whether the proposal was refused as a unit (some target Blocked).
    /// Nothing was written.
    pub fn is_refused(&self) -> bool {
        self.changes
            .iter()
            .any(|c| c.disposition == Disposition::Refused)
    }

    /// Whether the proposal is held pending Gate 3 coherence review. Nothing
    /// was written.
    pub fn is_held(&self) -> bool {
        !self.is_refused()
            && self
                .changes
                .iter()
                .any(|c| c.disposition == Disposition::HeldForCoherence)
    }

    /// Whether every edit in the proposal was written.
    pub fn is_applied(&self) -> bool {
        !self.changes.is_empty()
            && self
                .changes
                .iter()
                .all(|c| c.disposition == Disposition::Applied)
    }

    /// The Gate 4 audit records for the changes that were written.
    pub fn audit_records(&self) -> impl Iterator<Item = &AuditRecord> {
        self.changes.iter().filter_map(|c| c.audit.as_ref())
    }
}

/// A configured Ward for one familiar home.
pub struct Ward {
    home: PathBuf,
    config: WardConfig,
    // Per-tier matchers, indexed by tier number (0..=3).
    matchers: [GlobSet; 4],
    // Case-insensitive matcher over the tier-0 patterns (case-collision guard).
    protected_ci: GlobSet,
}

impl Ward {
    /// Build a Ward for the familiar rooted at `home`.
    pub fn new(home: impl Into<PathBuf>, config: WardConfig) -> Result<Self> {
        config.validate()?;
        let home = home.into();

        let mut builders: [GlobSetBuilder; 4] = [
            GlobSetBuilder::new(),
            GlobSetBuilder::new(),
            GlobSetBuilder::new(),
            GlobSetBuilder::new(),
        ];
        let mut protected_ci = GlobSetBuilder::new();

        for entry in &config.surface {
            let glob = compile_glob(&entry.path, false)
                .with_context(|| format!("invalid surface glob `{}`", entry.path))?;
            builders[entry.tier.as_u8() as usize].add(glob);

            if entry.tier == Tier::Protected {
                let ci = compile_glob(&entry.path, true)
                    .with_context(|| format!("invalid protected surface glob `{}`", entry.path))?;
                protected_ci.add(ci);
            }
        }

        let matchers = [
            builders[0].build()?,
            builders[1].build()?,
            builders[2].build()?,
            builders[3].build()?,
        ];

        Ok(Ward {
            home,
            config,
            matchers,
            protected_ci: protected_ci.build()?,
        })
    }

    /// Adjudicate a proposal. Runs Gate 2 (surface discrimination) then Gate 1
    /// (authorization) for each target.
    pub fn evaluate(&self, proposal: &Proposal) -> Outcome {
        let decisions = proposal
            .targets
            .iter()
            .map(|target| self.evaluate_target(target, &proposal.authorization))
            .collect();
        Outcome { decisions }
    }

    fn evaluate_target(&self, target: &str, authorization: &Authorization) -> Decision {
        // Gate 2: surface discrimination — resolve the real target.
        let resolved = match self.materialize(target) {
            Ok(resolved) => resolved,
            Err(reason) => {
                return Decision {
                    target: target.to_string(),
                    resolved: target.to_string(),
                    // On a resolution failure we cannot know the tier; treat as
                    // maximally protected for reporting.
                    tier: Tier::Protected,
                    verdict: Verdict::Blocked { reason },
                };
            }
        };

        // Case-collision guard: a path that matches a protected pattern only
        // when compared case-insensitively is a smuggling attempt on a
        // case-insensitive filesystem. Fail closed.
        if self.protected_ci.is_match(&resolved) && !self.matchers[0].is_match(&resolved) {
            return Decision {
                target: target.to_string(),
                resolved: resolved.clone(),
                tier: Tier::Protected,
                verdict: Verdict::Blocked {
                    reason: BlockReason::CaseCollision {
                        protected_as: resolved,
                    },
                },
            };
        }

        let tier = self.classify(&resolved);

        // Gate 1: authorization.
        let verdict = match tier {
            Tier::Protected => {
                if self.is_authorized(authorization) {
                    Verdict::AuthorizedProtectedChange
                } else {
                    Verdict::Blocked {
                        reason: BlockReason::Unauthorized,
                    }
                }
            }
            Tier::Reviewed => Verdict::RequiresCoherenceReview,
            Tier::Logged => Verdict::AllowWithLog,
            Tier::Free => Verdict::Allow,
        };

        Decision {
            target: target.to_string(),
            resolved,
            tier,
            verdict,
        }
    }

    /// Classify a home-relative resolved path into its trust tier, taking the
    /// most protective (lowest) tier among all matching entries. Fail-closed on
    /// ambiguity means overlapping declarations resolve to the stricter tier.
    fn classify(&self, resolved: &str) -> Tier {
        for (idx, matcher) in self.matchers.iter().enumerate() {
            if matcher.is_match(resolved) {
                // idx is a valid tier by construction (0..=3).
                return Tier::try_from(idx as u8).expect("matcher index is a valid tier");
            }
        }
        self.config.default_tier
    }

    fn is_authorized(&self, authorization: &Authorization) -> bool {
        authorization
            .principal_signature_fingerprint
            .as_deref()
            .is_some_and(|fp| fp == self.config.principal_key_fingerprint)
    }

    /// Gate 2: resolve a proposed target to the home-relative path it actually
    /// writes, defending against `..` traversal and symlink escape.
    ///
    /// Returns the resolved path (forward-slashed, relative to home) or a
    /// [`BlockReason`] if the target cannot be safely confined to the home.
    fn materialize(&self, target: &str) -> std::result::Result<String, BlockReason> {
        // 1. Lexically normalize the joined path (fold `.` and `..`). A target
        //    that would climb above the home is a traversal escape.
        let normalized = lexical_join(&self.home, target).ok_or(BlockReason::TraversalEscape)?;

        // 2. Resolve symlinks on the longest existing prefix. If the canonical
        //    prefix leaves the (canonical) home, it is a symlink escape.
        let canonical_home = self
            .home
            .canonicalize()
            .map_err(|err| BlockReason::Unresolvable {
                detail: format!("home `{}`: {err}", self.home.display()),
            })?;

        let resolved_abs = resolve_within(&canonical_home, &normalized)?;

        // 3. Express the resolved path relative to the home, forward-slashed.
        let rel = resolved_abs
            .strip_prefix(&canonical_home)
            .map_err(|_| BlockReason::SymlinkEscape)?;
        Ok(to_forward_slashes(rel))
    }

    /// The fail-closed diff/apply boundary — the real security choke point.
    ///
    /// Adjudicates `edits` (via [`Ward::evaluate`]) and, only if the whole
    /// proposal clears every applicable gate, writes them to disk.
    /// All-or-nothing:
    ///
    /// - If any target is **Blocked** (Gate 1/2), the proposal is *refused* as a
    ///   unit and nothing is written.
    /// - If any target needs **Gate 3 coherence review** — Tier 1, or a Tier 0
    ///   change authorized by Gate 1 (Gate 3 is not yet implemented, so an
    ///   authorized protected change cannot be cleared here) — the proposal is
    ///   *held* as a unit and nothing is written.
    /// - Otherwise every edit is Tier 2/3: each is written atomically (staged in
    ///   the target's directory, then renamed into place) and every Tier 2
    ///   write emits a Gate 4 [`AuditRecord`].
    ///
    /// Because writes are routed through [`Ward::evaluate`] first, Gate 2 path
    /// confinement applies to the apply too: an edit that resolves out of the
    /// familiar home (via `..`, symlink, or case collision) is refused before
    /// any byte is written. Returns `Err` only on an I/O failure *after* the
    /// gates cleared; a refusal or hold is a normal [`ApplyReport`], not an error.
    pub fn apply(&self, edits: &[FileEdit], authorization: &Authorization) -> Result<ApplyReport> {
        let proposal = Proposal {
            targets: edits.iter().map(|e| e.target.clone()).collect(),
            authorization: authorization.clone(),
        };
        let outcome = self.evaluate(&proposal);

        // Decide the proposal-wide disposition before touching the filesystem.
        let unit = if outcome.is_blocked() {
            Disposition::Refused
        } else if outcome
            .decisions
            .iter()
            .any(|d| requires_coherence(&d.verdict))
        {
            Disposition::HeldForCoherence
        } else {
            Disposition::Applied
        };

        // Refused or held: write nothing, report per-target dispositions.
        if unit != Disposition::Applied {
            let changes = outcome
                .decisions
                .into_iter()
                .map(|decision| {
                    let disposition = if decision.verdict.is_blocked() {
                        Disposition::Refused
                    } else if requires_coherence(&decision.verdict) {
                        Disposition::HeldForCoherence
                    } else {
                        // A cleared edit bundled with a refused/held one is
                        // still not written — the proposal is all-or-nothing.
                        unit
                    };
                    AppliedChange {
                        decision,
                        disposition,
                        audit: None,
                    }
                })
                .collect();
            return Ok(ApplyReport { changes });
        }

        // Every edit is Tier 2/3 and cleared. Write each atomically.
        let canonical_home = self
            .home
            .canonicalize()
            .with_context(|| format!("ward home `{}` is not resolvable", self.home.display()))?;
        let mut changes = Vec::with_capacity(edits.len());
        for (edit, decision) in edits.iter().zip(outcome.decisions) {
            let abs = join_resolved(&canonical_home, &decision.resolved);
            let audit = write_atomic(&abs, &edit.new_contents, &decision)?;
            changes.push(AppliedChange {
                decision,
                disposition: Disposition::Applied,
                audit,
            });
        }
        Ok(ApplyReport { changes })
    }
}

/// Whether a verdict needs Gate 3 coherence review before it can be applied.
///
/// This includes Gate-1 authorized Tier 0 changes: Gate 3 is not yet
/// implemented, so an authorized protected change cannot be cleared for apply
/// here. Fail-closed — hold rather than write.
fn requires_coherence(verdict: &Verdict) -> bool {
    matches!(
        verdict,
        Verdict::RequiresCoherenceReview | Verdict::AuthorizedProtectedChange
    )
}

/// Join a Gate-2 resolved (home-relative, forward-slashed) path onto the
/// canonical home to get the absolute path to write.
fn join_resolved(canonical_home: &Path, resolved: &str) -> PathBuf {
    let mut path = canonical_home.to_path_buf();
    for segment in resolved.split('/').filter(|s| !s.is_empty()) {
        path.push(segment);
    }
    path
}

/// Write `contents` to `path` atomically: stage a sibling file, then rename it
/// into place (rename is atomic within a filesystem). Returns a Gate 4 audit
/// record for Tier 2 writes; Tier 3 (free) writes return `None`. Tier 0/1 never
/// reach this function — they are held or refused upstream.
fn write_atomic(path: &Path, contents: &[u8], decision: &Decision) -> Result<Option<AuditRecord>> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("target has no parent directory: {}", path.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

    let need_audit = decision.tier == Tier::Logged;

    // Capture prior contents for the audit hash (only when we will log).
    let prev = if need_audit {
        match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                return Err(anyhow!(
                    "reading prior contents of {}: {err}",
                    path.display()
                ))
            }
        }
    } else {
        None
    };

    let staged = staged_path(path);
    std::fs::write(&staged, contents)
        .with_context(|| format!("staging write to {}", staged.display()))?;
    std::fs::rename(&staged, path)
        .with_context(|| format!("committing write to {}", path.display()))?;

    let audit = need_audit.then(|| AuditRecord {
        target: decision.target.clone(),
        resolved: decision.resolved.clone(),
        tier: decision.tier,
        prev_sha256: prev.as_deref().map(sha256_hex),
        next_sha256: sha256_hex(contents),
        bytes_written: contents.len(),
    });
    Ok(audit)
}

/// The sibling staging path for an atomic write (`<name>.ward-staged`).
fn staged_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".ward-staged");
    path.with_file_name(name)
}

/// Lowercase hex SHA-256 of `bytes`.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Compile a surface glob. A trailing `/` means "everything under here".
fn compile_glob(pattern: &str, case_insensitive: bool) -> Result<Glob> {
    let normalized = if let Some(stripped) = pattern.strip_suffix('/') {
        format!("{stripped}/**")
    } else {
        pattern.to_string()
    };
    GlobBuilder::new(&normalized)
        .case_insensitive(case_insensitive)
        .literal_separator(true)
        .build()
        .map_err(|err| anyhow!("bad glob `{pattern}`: {err}"))
}

/// Lexically join `base` and a relative `target`, folding `.`/`..` without
/// touching the filesystem. Returns `None` if the result would escape `base`.
fn lexical_join(base: &Path, target: &str) -> Option<PathBuf> {
    // An absolute target is never allowed; the surface is home-relative.
    let target_path = Path::new(target);
    if target_path.is_absolute() {
        return None;
    }

    let mut stack: Vec<std::ffi::OsString> = Vec::new();
    for component in target_path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Cannot climb above the home root.
                stack.pop()?;
            }
            Component::Normal(part) => stack.push(part.to_os_string()),
            // Absolute prefixes / root were rejected above.
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    let mut out = base.to_path_buf();
    for part in stack {
        out.push(part);
    }
    Some(out)
}

/// Canonicalize the longest existing prefix of `normalized` (resolving
/// symlinks) and re-attach the non-existing tail, verifying the result stays
/// under `canonical_home`.
fn resolve_within(
    canonical_home: &Path,
    normalized: &Path,
) -> std::result::Result<PathBuf, BlockReason> {
    // Walk the tail components that do not yet exist, canonicalizing the
    // existing ancestor.
    let mut existing = normalized.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();

    loop {
        if existing.exists() {
            break;
        }
        match existing.file_name() {
            Some(name) => {
                tail.push(name.to_os_string());
                if !existing.pop() {
                    break;
                }
            }
            None => break,
        }
    }

    let canonical_existing = existing
        .canonicalize()
        .map_err(|err| BlockReason::Unresolvable {
            detail: format!("{}: {err}", existing.display()),
        })?;

    // The existing (symlink-resolved) ancestor must stay within the home.
    if !canonical_existing.starts_with(canonical_home) {
        return Err(BlockReason::SymlinkEscape);
    }

    let mut resolved = canonical_existing;
    for name in tail.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn to_forward_slashes(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sample_config() -> WardConfig {
        WardConfig {
            principal_key_fingerprint: "SHA256:principal-key".to_string(),
            surface: vec![
                SurfaceEntry {
                    path: "SOUL.md".into(),
                    tier: Tier::Protected,
                },
                SurfaceEntry {
                    path: "IDENTITY.md".into(),
                    tier: Tier::Protected,
                },
                SurfaceEntry {
                    path: "USER.md".into(),
                    tier: Tier::Protected,
                },
                SurfaceEntry {
                    path: "MEMORY.md".into(),
                    tier: Tier::Reviewed,
                },
                SurfaceEntry {
                    path: "memory/".into(),
                    tier: Tier::Logged,
                },
                SurfaceEntry {
                    path: "scratch/".into(),
                    tier: Tier::Free,
                },
            ],
            protected_surface: vec!["SOUL.md".into(), "IDENTITY.md".into(), "USER.md".into()],
            default_tier: Tier::Logged,
        }
    }

    fn ward_in(dir: &Path) -> Ward {
        Ward::new(dir.to_path_buf(), sample_config()).expect("valid ward")
    }

    #[test]
    fn parses_ward_toml() {
        // Root-level scalars/arrays must precede the `[[surface]]`
        // array-of-tables, or TOML binds them to the last table.
        let toml = r#"
principal_key_fingerprint = "SHA256:abc"
protected_surface = ["SOUL.md"]

[[surface]]
path = "SOUL.md"
tier = 0

[[surface]]
path = "memory/"
tier = 2
"#;
        let config = WardConfig::from_toml_str(toml).expect("parses");
        assert_eq!(config.principal_key_fingerprint, "SHA256:abc");
        assert_eq!(config.surface.len(), 2);
        assert_eq!(config.surface[0].tier, Tier::Protected);
        assert_eq!(config.default_tier, Tier::Logged);
    }

    #[test]
    fn validation_rejects_mismatched_protected_surface() {
        let mut config = sample_config();
        config.protected_surface = vec!["SOUL.md".into()]; // missing IDENTITY.md, USER.md
        let err = config.validate().expect_err("must reject");
        assert!(err.to_string().contains("protected_surface"));
    }

    #[test]
    fn validation_rejects_empty_principal() {
        let mut config = sample_config();
        config.principal_key_fingerprint = "  ".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn protected_change_blocked_without_signature() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        let proposal = Proposal {
            targets: vec!["SOUL.md".into()],
            authorization: Authorization::unsigned(),
        };
        let outcome = ward.evaluate(&proposal);
        assert!(outcome.is_blocked());
        assert_eq!(
            outcome.decisions[0].verdict,
            Verdict::Blocked {
                reason: BlockReason::Unauthorized
            }
        );
        assert_eq!(outcome.decisions[0].tier, Tier::Protected);
    }

    #[test]
    fn protected_change_authorized_with_matching_signature() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        let proposal = Proposal {
            targets: vec!["IDENTITY.md".into()],
            authorization: Authorization::signed_by("SHA256:principal-key"),
        };
        let outcome = ward.evaluate(&proposal);
        assert!(!outcome.is_blocked());
        assert_eq!(
            outcome.decisions[0].verdict,
            Verdict::AuthorizedProtectedChange
        );
    }

    #[test]
    fn wrong_signature_does_not_authorize() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        let proposal = Proposal {
            targets: vec!["SOUL.md".into()],
            authorization: Authorization::signed_by("SHA256:attacker-key"),
        };
        let outcome = ward.evaluate(&proposal);
        assert!(outcome.is_blocked());
    }

    #[test]
    fn tier_classification_maps_to_verdicts() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());

        let reviewed = ward.evaluate(&Proposal {
            targets: vec!["MEMORY.md".into()],
            authorization: Authorization::unsigned(),
        });
        assert_eq!(
            reviewed.decisions[0].verdict,
            Verdict::RequiresCoherenceReview
        );

        let logged = ward.evaluate(&Proposal {
            targets: vec!["memory/2026-07-08.md".into()],
            authorization: Authorization::unsigned(),
        });
        assert_eq!(logged.decisions[0].verdict, Verdict::AllowWithLog);
        assert_eq!(logged.decisions[0].tier, Tier::Logged);

        let free = ward.evaluate(&Proposal {
            targets: vec!["scratch/notes.txt".into()],
            authorization: Authorization::unsigned(),
        });
        assert_eq!(free.decisions[0].verdict, Verdict::Allow);
    }

    #[test]
    fn unmatched_path_defaults_to_logged() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        let outcome = ward.evaluate(&Proposal {
            targets: vec!["TOOLS.md".into()],
            authorization: Authorization::unsigned(),
        });
        assert_eq!(outcome.decisions[0].tier, Tier::Logged);
        assert_eq!(outcome.decisions[0].verdict, Verdict::AllowWithLog);
    }

    #[test]
    fn traversal_escape_is_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        let outcome = ward.evaluate(&Proposal {
            targets: vec!["../../etc/passwd".into()],
            authorization: Authorization::unsigned(),
        });
        assert!(outcome.is_blocked());
        assert_eq!(
            outcome.decisions[0].verdict,
            Verdict::Blocked {
                reason: BlockReason::TraversalEscape
            }
        );
    }

    #[test]
    fn traversal_that_lands_back_on_protected_is_classified_protected() {
        // `memory/../SOUL.md` normalizes to `SOUL.md` — Gate 2 must see the real
        // target and Gate 1 must then block it.
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        let outcome = ward.evaluate(&Proposal {
            targets: vec!["memory/../SOUL.md".into()],
            authorization: Authorization::unsigned(),
        });
        assert_eq!(outcome.decisions[0].resolved, "SOUL.md");
        assert_eq!(outcome.decisions[0].tier, Tier::Protected);
        assert!(outcome.is_blocked());
    }

    #[test]
    fn absolute_target_is_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        let outcome = ward.evaluate(&Proposal {
            targets: vec!["/etc/hosts".into()],
            authorization: Authorization::unsigned(),
        });
        assert!(outcome.is_blocked());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_blocked() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&outside).unwrap();
        // home/escape -> outside
        symlink(&outside, home.join("escape")).unwrap();

        let ward = ward_in(&home);
        let outcome = ward.evaluate(&Proposal {
            targets: vec!["escape/loot.md".into()],
            authorization: Authorization::unsigned(),
        });
        assert!(outcome.is_blocked());
        assert_eq!(
            outcome.decisions[0].verdict,
            Verdict::Blocked {
                reason: BlockReason::SymlinkEscape
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_pointing_at_protected_is_classified_protected() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("SOUL.md"), "soul").unwrap();
        // home/alias.md -> home/SOUL.md ; editing the alias must resolve to SOUL.md
        symlink(home.join("SOUL.md"), home.join("alias.md")).unwrap();

        let ward = ward_in(&home);
        let outcome = ward.evaluate(&Proposal {
            targets: vec!["alias.md".into()],
            authorization: Authorization::unsigned(),
        });
        assert_eq!(outcome.decisions[0].resolved, "SOUL.md");
        assert_eq!(outcome.decisions[0].tier, Tier::Protected);
        assert!(outcome.is_blocked());
    }

    #[test]
    fn case_collision_with_protected_is_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        // `soul.md` differs from the declared `SOUL.md` only by case.
        let outcome = ward.evaluate(&Proposal {
            targets: vec!["soul.md".into()],
            authorization: Authorization::unsigned(),
        });
        assert!(outcome.is_blocked());
        assert!(matches!(
            outcome.decisions[0].verdict,
            Verdict::Blocked {
                reason: BlockReason::CaseCollision { .. }
            }
        ));
    }

    #[test]
    fn overlapping_entries_take_the_most_protective_tier() {
        let mut config = sample_config();
        // Declare a broad logged region and a narrow reviewed file inside it.
        config.surface.push(SurfaceEntry {
            path: "memory/pinned.md".into(),
            tier: Tier::Reviewed,
        });
        let tmp = tempfile::tempdir().unwrap();
        let ward = Ward::new(tmp.path().to_path_buf(), config).unwrap();
        let outcome = ward.evaluate(&Proposal {
            targets: vec!["memory/pinned.md".into()],
            authorization: Authorization::unsigned(),
        });
        // Reviewed (tier 1) is more protective than Logged (tier 2).
        assert_eq!(outcome.decisions[0].tier, Tier::Reviewed);
    }

    #[test]
    fn proposal_blocked_as_unit_if_any_target_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        let outcome = ward.evaluate(&Proposal {
            targets: vec!["scratch/ok.txt".into(), "SOUL.md".into()],
            authorization: Authorization::unsigned(),
        });
        assert!(outcome.is_blocked());
        assert_eq!(outcome.blocked().count(), 1);
    }

    // ---- apply: the fail-closed diff/apply boundary ----

    #[test]
    fn apply_writes_free_and_logged_and_audits_only_logged() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        let edits = vec![
            FileEdit::new("scratch/notes.txt", b"scratch".to_vec()),
            FileEdit::new("memory/log.md", b"entry".to_vec()),
        ];
        let report = ward.apply(&edits, &Authorization::unsigned()).unwrap();

        assert!(report.is_applied());
        assert!(!report.is_refused() && !report.is_held());
        assert_eq!(
            fs::read(tmp.path().join("scratch/notes.txt")).unwrap(),
            b"scratch"
        );
        assert_eq!(
            fs::read(tmp.path().join("memory/log.md")).unwrap(),
            b"entry"
        );

        // Only the Tier 2 (memory/) write is audited; Tier 3 (scratch) is not.
        let audits: Vec<_> = report.audit_records().collect();
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].resolved, "memory/log.md");
        assert_eq!(audits[0].tier, Tier::Logged);
        assert_eq!(audits[0].prev_sha256, None);
        assert_eq!(audits[0].next_sha256, sha256_hex(b"entry"));
        assert_eq!(audits[0].bytes_written, 5);

        // No staging litter remains after the atomic rename.
        assert!(!tmp.path().join("memory/log.md.ward-staged").exists());
    }

    #[test]
    fn apply_refuses_whole_proposal_if_any_target_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        // A harmless scratch write bundled with an unauthorized Tier 0 change.
        let edits = vec![
            FileEdit::new("scratch/ok.txt", b"ok".to_vec()),
            FileEdit::new("SOUL.md", b"pwned".to_vec()),
        ];
        let report = ward.apply(&edits, &Authorization::unsigned()).unwrap();

        assert!(report.is_refused());
        assert!(!report.is_applied());
        // Fail-closed: NOTHING is written, not even the harmless scratch edit.
        assert!(!tmp.path().join("scratch/ok.txt").exists());
        assert!(!tmp.path().join("SOUL.md").exists());
    }

    #[test]
    fn apply_holds_whole_proposal_for_coherence_review() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        // MEMORY.md is Tier 1 (Ward-reviewed) — needs Gate 3.
        let edits = vec![
            FileEdit::new("scratch/ok.txt", b"ok".to_vec()),
            FileEdit::new("MEMORY.md", b"revised".to_vec()),
        ];
        let report = ward.apply(&edits, &Authorization::unsigned()).unwrap();

        assert!(report.is_held());
        assert!(!report.is_refused());
        assert!(!report.is_applied());
        // Held as a unit: nothing written.
        assert!(!tmp.path().join("scratch/ok.txt").exists());
        assert!(!tmp.path().join("MEMORY.md").exists());
    }

    #[test]
    fn authorized_protected_change_is_held_not_applied() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        let edits = vec![FileEdit::new("SOUL.md", b"new soul".to_vec())];
        // Valid Gate 1 signature, but Gate 3 (coherence) is unavailable.
        let report = ward
            .apply(&edits, &Authorization::signed_by("SHA256:principal-key"))
            .unwrap();

        // Fail-closed: authorized, but cannot be coherence-cleared, so held.
        assert!(report.is_held());
        assert_eq!(
            report.changes[0].decision.verdict,
            Verdict::AuthorizedProtectedChange
        );
        assert!(!tmp.path().join("SOUL.md").exists());
    }

    #[test]
    fn apply_confines_writes_within_home_gate2() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        // Escapes the home; Gate 2 must refuse before any write.
        let edits = vec![FileEdit::new("../escape.txt", b"x".to_vec())];
        let report = ward.apply(&edits, &Authorization::unsigned()).unwrap();

        assert!(report.is_refused());
        assert!(!tmp.path().parent().unwrap().join("escape.txt").exists());
    }

    #[test]
    fn apply_overwrites_existing_and_hashes_prior_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        fs::write(tmp.path().join("memory/log.md"), b"old").unwrap();

        let edits = vec![FileEdit::new("memory/log.md", b"new".to_vec())];
        let report = ward.apply(&edits, &Authorization::unsigned()).unwrap();

        assert!(report.is_applied());
        assert_eq!(fs::read(tmp.path().join("memory/log.md")).unwrap(), b"new");
        let audit = report.audit_records().next().unwrap();
        assert_eq!(audit.prev_sha256, Some(sha256_hex(b"old")));
        assert_eq!(audit.next_sha256, sha256_hex(b"new"));
    }
}
