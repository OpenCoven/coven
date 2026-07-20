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
//!
//! ## Atomic write & staging threat model
//!
//! [`Ward::apply`] commits each cleared edit by staging a randomized sibling
//! file (`.{name}.ward-staged-<uuid>`, opened with `create_new`) and renaming
//! it onto the Gate-2-validated target:
//!
//! - **Pre-planted staging symlinks or hard links** cannot be followed: the
//!   staging name is unpredictable and `create_new` refuses to open through
//!   an existing directory entry.
//! - **Symlinked targets or parent directories** are refused by Gate 2 before
//!   any byte is written, and the target's parent is re-created and verified
//!   component-by-component (never following symlinks) immediately before
//!   staging — a directory component swapped for a symlink after adjudication
//!   fails closed with no side effects outside the home.
//! - **Hard-linked targets** are harmless by construction: `rename` replaces
//!   the directory entry and never writes through the linked inode.
//!
//! Phase 5 approved writes additionally preserve the file actually displaced by
//! the atomic commit. Linux and macOS exchange the staged and target entries;
//! Windows uses `ReplaceFileW` with a distinct randomized sibling backup. The
//! displaced before-image and installed bytes are verified before the batch can
//! finalize, and rollback uses the same primitive in reverse so concurrent
//! target bytes are preserved rather than overwritten.
//!
//! Residual risk (accepted): a same-privilege process that can already write
//! inside the familiar home can still swap path components in the window
//! between the per-component verification and the final `rename`. The check
//! narrows that race without eliminating it; full elimination needs
//! directory-handle-relative I/O (e.g. `openat2` + `RESOLVE_BENEATH`), which
//! is not portable across the supported platforms. A final-component swap can
//! also point the Gate 4 pre-write audit read at an attacker-chosen readable
//! file, affecting only the recorded `prev_sha256` — never where bytes land.

use std::collections::{BTreeMap, BTreeSet};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::ffi::CString;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::os::unix::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
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

/// Conventional name of the Ward configuration file inside a familiar home.
pub const WARD_CONFIG_FILE: &str = "ward.toml";

impl WardConfig {
    /// Load the Ward configuration from `<home>/ward.toml`.
    ///
    /// Returns `Ok(None)` when the file does not exist (the familiar has no
    /// declared Ward). A present-but-invalid file is an error, never silently
    /// ignored: a malformed Ward must not degrade into "no Ward".
    pub fn load(home: &Path) -> Result<Option<Self>> {
        let path = home.join(WARD_CONFIG_FILE);
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(anyhow!("reading ward config {}: {err}", path.display()));
            }
        };
        Self::from_toml_str(&raw)
            .with_context(|| format!("invalid ward config at {}", path.display()))
            .map(Some)
    }

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
#[serde(rename_all = "camelCase")]
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
    /// - Otherwise every edit is Tier 2/3: each is written atomically (staged
    ///   as a randomized `create_new` sibling in the target's re-verified
    ///   directory, then renamed into place) and every Tier 2 write emits a
    ///   Gate 4 [`AuditRecord`].
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
            let audit = write_atomic(&canonical_home, &abs, &edit.new_contents, &decision)?;
            changes.push(AppliedChange {
                decision,
                disposition: Disposition::Applied,
                audit,
            });
        }
        Ok(ApplyReport { changes })
    }

    /// Apply edits after an explicit principal proposal approval has cleared the
    /// daemon-side threads replay. Gate 1 and Gate 2 still run here; Gate 3's
    /// "held for review" state is the decision this endpoint represents.
    pub(crate) fn apply_after_threads_approval(
        &self,
        edits: &[FileEdit],
        authorization: &Authorization,
        expected_before: &BTreeMap<String, Vec<u8>>,
    ) -> Result<ApplyReport> {
        let proposal = Proposal {
            targets: edits.iter().map(|e| e.target.clone()).collect(),
            authorization: authorization.clone(),
        };
        let outcome = self.evaluate(&proposal);
        if outcome.is_blocked() {
            let changes = outcome
                .decisions
                .into_iter()
                .map(|decision| AppliedChange {
                    disposition: if decision.verdict.is_blocked() {
                        Disposition::Refused
                    } else {
                        Disposition::HeldForCoherence
                    },
                    decision,
                    audit: None,
                })
                .collect();
            return Ok(ApplyReport { changes });
        }

        let canonical_home = self
            .home
            .canonicalize()
            .with_context(|| format!("ward home `{}` is not resolvable", self.home.display()))?;
        let changes = write_atomically_if_unchanged(
            &canonical_home,
            edits,
            outcome.decisions,
            expected_before,
        )?;
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
fn write_atomic(
    canonical_home: &Path,
    path: &Path,
    contents: &[u8],
    decision: &Decision,
) -> Result<Option<AuditRecord>> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("target has no parent directory: {}", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("target has no file name: {}", path.display()))?;

    // Re-create and verify the parent immediately before staging. Gate 2
    // cleared the target earlier; a directory component swapped for a symlink
    // since then (TOCTOU) must fail closed — with no side effects outside the
    // familiar home — rather than redirect the write.
    let canonical_parent = prepare_staging_parent(canonical_home, parent)?;
    let path = canonical_parent.join(name);

    let need_audit = decision.tier == Tier::Logged;

    // Capture prior contents for the audit hash (only when we will log).
    let prev = if need_audit {
        match std::fs::read(&path) {
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

    let (staged, mut file) = create_staging_file(&path)?;
    let commit = (|| -> Result<()> {
        file.write_all(contents)
            .with_context(|| format!("staging write to {}", staged.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing staged write to {}", staged.display()))?;
        drop(file);
        std::fs::rename(&staged, &path)
            .with_context(|| format!("committing write to {}", path.display()))?;
        Ok(())
    })();
    if commit.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    commit?;

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

struct ApprovedWritePaths {
    staged: PathBuf,
    displaced: PathBuf,
}

impl ApprovedWritePaths {
    fn new(target: &Path, staged: PathBuf) -> Result<Self> {
        let displaced = approved_write_displaced_path(target, &staged)?;
        Ok(Self { staged, displaced })
    }
}

struct PreparedConditionalWrite {
    path: PathBuf,
    paths: Option<ApprovedWritePaths>,
    already_applied: bool,
    expected_before: Vec<u8>,
    new_contents: Vec<u8>,
    decision: Decision,
}

/// Commit an approved proposal only while every target still has the exact
/// before-image reviewed by the scheduler.
///
/// Existing targets are atomically replaced from randomized sibling staging
/// files while preserving the displaced target at a known sibling path. The
/// displaced bytes are then compared with the approved before-image; a mismatch
/// restores them and rolls back every earlier edit owned by this apply attempt.
/// Already-applied bytes are accepted so crash recovery remains idempotent.
fn write_atomically_if_unchanged(
    canonical_home: &Path,
    edits: &[FileEdit],
    decisions: Vec<Decision>,
    expected_before: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<AppliedChange>> {
    let mut prepared = Vec::with_capacity(edits.len());
    let mut preparation_error = None;
    for (edit, decision) in edits.iter().zip(decisions) {
        let result = (|| -> Result<PreparedConditionalWrite> {
            let expected = expected_before
                .get(&edit.target)
                .with_context(|| format!("missing approved before-image for `{}`", edit.target))?
                .clone();
            let resolved = join_resolved(canonical_home, &decision.resolved);
            let parent = resolved
                .parent()
                .ok_or_else(|| anyhow!("target has no parent directory: {}", resolved.display()))?;
            let name = resolved
                .file_name()
                .ok_or_else(|| anyhow!("target has no file name: {}", resolved.display()))?;
            let canonical_parent = prepare_staging_parent(canonical_home, parent)?;
            let path = canonical_parent.join(name);
            let current = std::fs::read(&path)
                .with_context(|| format!("reading approved target {}", path.display()))?;
            let already_applied = current == edit.new_contents;
            if current != expected && !already_applied {
                bail!(
                    "approved target `{}` changed after review; refusing to overwrite it",
                    edit.target
                );
            }
            Ok(PreparedConditionalWrite {
                path,
                paths: None,
                already_applied,
                expected_before: expected,
                new_contents: edit.new_contents.clone(),
                decision,
            })
        })();
        match result {
            Ok(write) => prepared.push(write),
            Err(error) => {
                if preparation_error.is_none() {
                    preparation_error = Some(error);
                }
            }
        }
    }
    if let Some(error) = preparation_error {
        return fail_after_conditional_rollback(&prepared, &[], error);
    }

    for index in 0..prepared.len() {
        if prepared[index].already_applied {
            continue;
        }
        match stage_contents(&prepared[index].path, &prepared[index].new_contents) {
            Ok(paths) => prepared[index].paths = Some(paths),
            Err(error) => return fail_after_conditional_rollback(&prepared, &[], error),
        }
    }

    let mut swapped = Vec::new();
    for (index, write) in prepared.iter().enumerate() {
        let Some(paths) = &write.paths else {
            continue;
        };
        if let Err(error) = maybe_run_conditional_write_hook(&write.path) {
            return fail_after_conditional_rollback(&prepared, &swapped, error);
        }
        if let Err(error) =
            atomic_replace_preserving_target(&write.path, &paths.staged, &paths.displaced)
        {
            if failed_replace_displaced_target(&paths.displaced) {
                swapped.push(index);
            }
            return fail_after_conditional_rollback(
                &prepared,
                &swapped,
                error.context(format!(
                    "committing approved write to {}",
                    write.path.display()
                )),
            );
        }
        swapped.push(index);

        let verification = (|| -> Result<()> {
            let displaced = std::fs::read(&paths.displaced).with_context(|| {
                format!("reading displaced target {}", paths.displaced.display())
            })?;
            let installed = std::fs::read(&write.path)
                .with_context(|| format!("verifying approved write {}", write.path.display()))?;
            if displaced != write.expected_before || installed != write.new_contents {
                bail!(
                    "approved target `{}` changed during commit",
                    write.decision.target
                );
            }
            Ok(())
        })();
        if let Err(error) = verification {
            return fail_after_conditional_rollback(&prepared, &swapped, error);
        }
    }

    let final_verification = (|| -> Result<()> {
        for write in &prepared {
            let current = std::fs::read(&write.path).with_context(|| {
                format!("revalidating approved target {}", write.path.display())
            })?;
            if current != write.new_contents {
                bail!(
                    "approved target `{}` changed before batch finalization",
                    write.decision.target
                );
            }
        }
        Ok(())
    })();
    if let Err(error) = final_verification {
        return fail_after_conditional_rollback(&prepared, &swapped, error);
    }

    for write in &prepared {
        if let Some(paths) = &write.paths {
            std::fs::remove_file(&paths.displaced).with_context(|| {
                format!(
                    "removing approved-write backup {}",
                    paths.displaced.display()
                )
            })?;
        }
    }

    Ok(prepared
        .into_iter()
        .map(|write| {
            let audit = (write.decision.tier == Tier::Logged).then(|| AuditRecord {
                target: write.decision.target.clone(),
                resolved: write.decision.resolved.clone(),
                tier: write.decision.tier,
                prev_sha256: Some(sha256_hex(&write.expected_before)),
                next_sha256: sha256_hex(&write.new_contents),
                bytes_written: write.new_contents.len(),
            });
            AppliedChange {
                decision: write.decision,
                disposition: Disposition::Applied,
                audit,
            }
        })
        .collect())
}

fn rollback_conditional_writes(
    prepared: &[PreparedConditionalWrite],
    swapped: &[usize],
) -> Result<()> {
    let mut errors = Vec::new();
    for &index in swapped.iter().rev() {
        let write = &prepared[index];
        let paths = write
            .paths
            .as_ref()
            .context("swapped conditional write has no approved-write paths")?;
        if let Err(error) = restore_swapped_write(write, paths) {
            errors.push(format!("{}: {error:#}", write.decision.target));
        }
    }
    for write in prepared.iter().rev().filter(|write| write.already_applied) {
        if write.expected_before != write.new_contents {
            errors.push(format!(
                "{}: approved bytes predated this apply attempt; ownership is unproven, so \
                 recovery left them in place",
                write.decision.target
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!("conditional rollback failed: {}", errors.join("; "))
    }
}

fn fail_after_conditional_rollback(
    prepared: &[PreparedConditionalWrite],
    swapped: &[usize],
    error: anyhow::Error,
) -> Result<Vec<AppliedChange>> {
    let rollback = rollback_conditional_writes(prepared, swapped);
    match rollback {
        Ok(()) => {
            cleanup_conditional_staging(prepared);
            Err(error.context("approved proposal was rolled back"))
        }
        Err(rollback_error) => Err(anyhow!(
            "{error:#}; conditional rollback also failed: {rollback_error:#}"
        )),
    }
}

fn restore_swapped_write(
    write: &PreparedConditionalWrite,
    paths: &ApprovedWritePaths,
) -> Result<()> {
    let restore_bytes = std::fs::read(&paths.displaced)
        .with_context(|| format!("reading rollback source {}", paths.displaced.display()))?;
    atomic_replace_preserving_target(&write.path, &paths.displaced, &paths.staged).with_context(
        || {
            format!(
                "restoring approved target {} from {}",
                write.path.display(),
                paths.displaced.display()
            )
        },
    )?;
    let verification = verify_rollback_exchange(write, &paths.staged, &restore_bytes);
    if let Err(error) = verification {
        atomic_replace_preserving_target(&write.path, &paths.staged, &paths.displaced)
            .with_context(|| {
                format!(
                    "putting concurrent bytes back at {} after rollback verification failed: \
                     {error:#}",
                    write.path.display()
                )
            })?;
        return Err(error);
    }
    Ok(())
}

fn verify_rollback_exchange(
    write: &PreparedConditionalWrite,
    displaced: &Path,
    expected_restored: &[u8],
) -> Result<()> {
    let displaced = std::fs::read(displaced).with_context(|| {
        format!(
            "reading rollback-displaced bytes for {}",
            write.path.display()
        )
    })?;
    let restored = std::fs::read(&write.path)
        .with_context(|| format!("verifying rollback of {}", write.path.display()))?;
    if displaced != write.new_contents || restored != expected_restored {
        bail!(
            "target changed while rolling back approved write {}",
            write.path.display()
        );
    }
    Ok(())
}

fn cleanup_conditional_staging(prepared: &[PreparedConditionalWrite]) {
    for write in prepared {
        if let Some(paths) = &write.paths {
            let _ = std::fs::remove_file(&paths.staged);
            if paths.displaced != paths.staged {
                let _ = std::fs::remove_file(&paths.displaced);
            }
        }
    }
}

pub(crate) const fn supports_atomic_approved_writes() -> bool {
    cfg!(any(target_os = "linux", target_os = "macos", windows))
}

fn stage_contents(path: &Path, contents: &[u8]) -> Result<ApprovedWritePaths> {
    let (staged, mut file) = create_staging_file(path)?;
    let result = (|| -> Result<()> {
        file.write_all(contents)
            .with_context(|| format!("staging write to {}", staged.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing staged write to {}", staged.display()))?;
        Ok(())
    })();
    drop(file);
    if let Err(error) = result {
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }
    match ApprovedWritePaths::new(path, staged.clone()) {
        Ok(paths) => Ok(paths),
        Err(error) => {
            let _ = std::fs::remove_file(staged);
            Err(error)
        }
    }
}

#[cfg(test)]
type ConditionalWriteHook = std::sync::Mutex<BTreeMap<PathBuf, Vec<(PathBuf, Vec<u8>)>>>;

#[cfg(test)]
fn conditional_write_hook() -> &'static ConditionalWriteHook {
    static HOOK: std::sync::OnceLock<ConditionalWriteHook> = std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
fn set_conditional_write_hook(path: PathBuf, replacement: Vec<u8>) {
    set_conditional_write_actions(path.clone(), vec![(path, replacement)]);
}

#[cfg(test)]
fn set_conditional_write_actions(trigger: PathBuf, actions: Vec<(PathBuf, Vec<u8>)>) {
    conditional_write_hook()
        .lock()
        .expect("conditional write hook lock poisoned")
        .insert(trigger, actions);
}

#[cfg(test)]
fn maybe_run_conditional_write_hook(path: &Path) -> Result<()> {
    let mut hook = conditional_write_hook()
        .lock()
        .expect("conditional write hook lock poisoned");
    if let Some(actions) = hook.remove(path) {
        for (target, replacement) in actions {
            std::fs::write(&target, replacement).with_context(|| {
                format!(
                    "running conditional write test hook for {}",
                    target.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(not(test))]
fn maybe_run_conditional_write_hook(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn atomic_replace_preserving_target(
    target: &Path,
    replacement: &Path,
    displaced: &Path,
) -> Result<()> {
    if replacement != displaced {
        bail!("macOS approved-write exchange requires a shared replacement/backup path");
    }
    const RENAME_SWAP: u32 = 0x0000_0002;
    unsafe extern "C" {
        fn renamex_np(
            from: *const libc::c_char,
            to: *const libc::c_char,
            flags: u32,
        ) -> libc::c_int;
    }

    let replacement = CString::new(replacement.as_os_str().as_bytes())
        .context("replacement path contains NUL")?;
    let target = CString::new(target.as_os_str().as_bytes()).context("target path contains NUL")?;
    // SAFETY: both C strings are NUL-terminated and remain alive for the call.
    let result = unsafe { renamex_np(replacement.as_ptr(), target.as_ptr(), RENAME_SWAP) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error()).context("atomically exchanging approved target")
    }
}

#[cfg(target_os = "linux")]
fn atomic_replace_preserving_target(
    target: &Path,
    replacement: &Path,
    displaced: &Path,
) -> Result<()> {
    if replacement != displaced {
        bail!("Linux approved-write exchange requires a shared replacement/backup path");
    }
    const RENAME_EXCHANGE: libc::c_uint = 1 << 1;
    let replacement = CString::new(replacement.as_os_str().as_bytes())
        .context("replacement path contains NUL")?;
    let target = CString::new(target.as_os_str().as_bytes()).context("target path contains NUL")?;
    // SAFETY: both C strings are NUL-terminated and remain alive for the syscall.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            replacement.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            RENAME_EXCHANGE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error()).context("atomically exchanging approved target")
    }
}

#[cfg(windows)]
fn atomic_replace_preserving_target(
    target: &Path,
    replacement: &Path,
    displaced: &Path,
) -> Result<()> {
    use windows_sys::Win32::Foundation::ERROR_UNABLE_TO_MOVE_REPLACEMENT_2;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    let target_wide = windows_path(target, "target")?;
    let replacement_wide = windows_path(replacement, "replacement")?;
    let displaced_wide = windows_path(displaced, "displaced")?;
    // REPLACEFILE_WRITE_THROUGH is documented as unsupported. The staged file
    // is synced before this call, and no ignore flags are used.
    let result = unsafe {
        ReplaceFileW(
            target_wide.as_ptr(),
            replacement_wide.as_ptr(),
            displaced_wide.as_ptr(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if result != 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 as i32) {
            if let Err(recovery_error) =
                restore_displaced_target_after_partial_replace(target, displaced)
            {
                return Err(anyhow!(
                    "atomically replacing approved target failed: {error}; restoring the \
                     documented partial replacement also failed: {recovery_error:#}"
                ));
            }
        }
        Err(error).context("atomically replacing approved target while preserving displaced bytes")
    }
}

#[cfg(windows)]
fn windows_path(path: &Path, label: &str) -> Result<Vec<u16>> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        bail!("{label} path contains NUL");
    }
    wide.push(0);
    Ok(wide)
}

#[cfg(windows)]
fn restore_displaced_target_after_partial_replace(target: &Path, displaced: &Path) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};

    let target = windows_path(target, "target")?;
    let displaced = windows_path(displaced, "displaced")?;
    let result =
        unsafe { MoveFileExW(displaced.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if result != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
            .context("atomically restoring the target after a partial Windows file replacement")
    }
}

#[cfg(windows)]
fn failed_replace_displaced_target(displaced: &Path) -> bool {
    match std::fs::symlink_metadata(displaced) {
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

#[cfg(not(windows))]
fn failed_replace_displaced_target(_displaced: &Path) -> bool {
    false
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn atomic_replace_preserving_target(
    _target: &Path,
    _replacement: &Path,
    _displaced: &Path,
) -> Result<()> {
    bail!("atomic approved-write exchange is unsupported on this platform")
}

#[cfg(windows)]
fn approved_write_displaced_path(target: &Path, _staged: &Path) -> Result<PathBuf> {
    // Use an independent random name so observing the staged entry does not
    // disclose the backup path before the atomic ReplaceFileW call.
    for _ in 0..16 {
        let displaced = displaced_path(target);
        match std::fs::symlink_metadata(&displaced) {
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(displaced),
            Ok(_) => continue,
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "checking approved-write backup path {}",
                        displaced.display()
                    )
                })
            }
        }
    }
    bail!(
        "could not select a fresh approved-write backup beside {}",
        target.display()
    )
}

#[cfg(not(windows))]
fn approved_write_displaced_path(_target: &Path, staged: &Path) -> Result<PathBuf> {
    Ok(staged.to_path_buf())
}

#[cfg(windows)]
fn displaced_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.ward-displaced-{}", uuid::Uuid::new_v4()))
}

/// Create a fresh sibling staging file for an atomic write.
///
/// The staging path is intentionally unpredictable and opened with
/// `create_new(true)` so a pre-planted symlink or hard link cannot be followed
/// before the final rename commits the edit into the Gate-2-validated target.
fn create_staging_file(path: &Path) -> Result<(PathBuf, std::fs::File)> {
    for _ in 0..16 {
        let staged = staged_path(path);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
        {
            Ok(file) => return Ok((staged, file)),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("creating staging file {}", staged.display()))
            }
        }
    }
    bail!(
        "could not create a fresh staging file beside {}",
        path.display()
    )
}

/// A randomized sibling staging path for an atomic write.
fn staged_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.ward-staged-{}", uuid::Uuid::new_v4()))
}

/// Re-create and verify the staging parent directory component-by-component,
/// never following symlinks.
///
/// `parent` must be the Gate-2-resolved target's parent, lexically under
/// `canonical_home`. Each component is created with a single (non-recursive,
/// non-following) `create_dir` and then checked via `symlink_metadata` to be a
/// real directory — so a component swapped for an escaping symlink after
/// adjudication fails closed without creating anything outside the home.
fn prepare_staging_parent(canonical_home: &Path, parent: &Path) -> Result<PathBuf> {
    let rel = parent.strip_prefix(canonical_home).map_err(|_| {
        anyhow!(
            "staging parent `{}` is not under the familiar home `{}`",
            parent.display(),
            canonical_home.display()
        )
    })?;

    let mut verified = canonical_home.to_path_buf();
    for component in rel.components() {
        let Component::Normal(part) = component else {
            bail!(
                "staging parent `{}` contains a non-normal path component",
                parent.display()
            );
        };
        verified.push(part);

        match std::fs::symlink_metadata(&verified) {
            Ok(meta) if meta.is_dir() => continue,
            Ok(_) => bail!(
                "staging parent component `{}` is not a real directory — refusing to \
                 follow it outside the familiar home",
                verified.display()
            ),
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("verifying staging parent {}", verified.display()))
            }
        }

        match std::fs::create_dir(&verified) {
            Ok(()) => {}
            // A concurrent apply may have created it; the re-check below rules.
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
            Err(err) => {
                return Err(err).with_context(|| format!("creating {}", verified.display()))
            }
        }
        let meta = std::fs::symlink_metadata(&verified)
            .with_context(|| format!("verifying staging parent {}", verified.display()))?;
        if !meta.is_dir() {
            bail!(
                "staging parent component `{}` is not a real directory — refusing to \
                 follow it outside the familiar home",
                verified.display()
            );
        }
    }
    Ok(verified)
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

    #[cfg(windows)]
    const _: () = assert!(supports_atomic_approved_writes());

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

    /// Directory entries that look like approved-write working files (randomized
    /// names make exact-path existence checks meaningless).
    fn staging_litter(dir: &Path) -> Vec<String> {
        match fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.contains(".ward-staged") || name.contains(".ward-displaced"))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn unix_approved_write_paths_reuse_staging_path_for_exchange() {
        let staged = PathBuf::from(".SOUL.md.ward-staged-test");
        let paths = ApprovedWritePaths::new(Path::new("SOUL.md"), staged.clone()).unwrap();

        assert_eq!(paths.staged, staged);
        assert_eq!(paths.displaced, staged);
    }

    #[cfg(windows)]
    #[test]
    fn windows_approved_write_paths_select_distinct_displaced_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("SOUL.md");
        let staged = tmp.path().join(".SOUL.md.ward-staged-test");
        let paths = ApprovedWritePaths::new(&target, staged.clone()).unwrap();

        assert_eq!(paths.staged, staged);
        assert_ne!(paths.displaced, paths.staged);
        assert_eq!(paths.displaced.parent(), target.parent());
        assert!(paths
            .displaced
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(".ward-displaced-"));
        assert!(!paths.displaced.exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_atomic_replace_preserves_displaced_bytes_for_commit_and_rollback() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("SOUL.md");
        let staged = tmp.path().join(".SOUL.md.ward-staged-test");
        fs::write(&target, b"old soul").unwrap();
        fs::write(&staged, b"new soul").unwrap();
        let paths = ApprovedWritePaths::new(&target, staged).unwrap();

        atomic_replace_preserving_target(&target, &paths.staged, &paths.displaced).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new soul");
        assert_eq!(fs::read(&paths.displaced).unwrap(), b"old soul");
        assert!(!paths.staged.exists());

        atomic_replace_preserving_target(&target, &paths.displaced, &paths.staged).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"old soul");
        assert_eq!(fs::read(&paths.staged).unwrap(), b"new soul");
        assert!(!paths.displaced.exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_partial_replace_state_requires_rollback_before_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let displaced = tmp.path().join(".SOUL.md.ward-displaced-test");

        assert!(!failed_replace_displaced_target(&displaced));
        fs::write(&displaced, b"old soul").unwrap();
        assert!(failed_replace_displaced_target(&displaced));
    }

    #[cfg(windows)]
    #[test]
    fn windows_partial_replace_recovery_restores_target_without_consuming_staged_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("SOUL.md");
        let staged = tmp.path().join(".SOUL.md.ward-staged-test");
        let displaced = tmp.path().join(".SOUL.md.ward-displaced-test");
        fs::write(&staged, b"new soul").unwrap();
        fs::write(&displaced, b"old soul").unwrap();

        restore_displaced_target_after_partial_replace(&target, &displaced).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"old soul");
        assert_eq!(fs::read(&staged).unwrap(), b"new soul");
        assert!(!displaced.exists());
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

        // No staging litter remains after the atomic renames.
        assert_eq!(
            staging_litter(&tmp.path().join("memory")),
            Vec::<String>::new()
        );
        assert_eq!(
            staging_litter(&tmp.path().join("scratch")),
            Vec::<String>::new()
        );
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
    fn approved_apply_rolls_back_batch_if_target_changes_immediately_before_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::write(tmp.path().join("SOUL.md"), b"old soul").unwrap();
        fs::write(tmp.path().join("IDENTITY.md"), b"old identity").unwrap();
        let edits = vec![
            FileEdit::new("SOUL.md", b"new soul".to_vec()),
            FileEdit::new("IDENTITY.md", b"new identity".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("SOUL.md".to_string(), b"old soul".to_vec()),
            ("IDENTITY.md".to_string(), b"old identity".to_vec()),
        ]);
        set_conditional_write_hook(
            tmp.path().canonicalize().unwrap().join("IDENTITY.md"),
            b"concurrent identity".to_vec(),
        );

        let error = ward
            .apply_after_threads_approval(
                &edits,
                &Authorization::signed_by("SHA256:principal-key"),
                &expected,
            )
            .expect_err("concurrent target replacement must fail closed");

        assert!(
            format!("{error:#}").contains("changed during commit"),
            "unexpected error: {error:#}"
        );
        assert_eq!(fs::read(tmp.path().join("SOUL.md")).unwrap(), b"old soul");
        assert_eq!(
            fs::read(tmp.path().join("IDENTITY.md")).unwrap(),
            b"concurrent identity"
        );
        assert_eq!(staging_litter(tmp.path()), Vec::<String>::new());
    }

    #[test]
    fn approved_recovery_does_not_claim_ownership_of_same_byte_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::write(tmp.path().join("SOUL.md"), b"new soul").unwrap();
        fs::write(tmp.path().join("IDENTITY.md"), b"concurrent identity").unwrap();
        let edits = vec![
            FileEdit::new("SOUL.md", b"new soul".to_vec()),
            FileEdit::new("IDENTITY.md", b"new identity".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("SOUL.md".to_string(), b"old soul".to_vec()),
            ("IDENTITY.md".to_string(), b"old identity".to_vec()),
        ]);

        let error = ward
            .apply_after_threads_approval(
                &edits,
                &Authorization::signed_by("SHA256:principal-key"),
                &expected,
            )
            .expect_err("diverged recovery target must fail the whole batch");

        assert!(
            format!("{error:#}").contains("ownership is unproven"),
            "unexpected error: {error:#}"
        );
        assert_eq!(fs::read(tmp.path().join("SOUL.md")).unwrap(), b"new soul");
        assert_eq!(
            fs::read(tmp.path().join("IDENTITY.md")).unwrap(),
            b"concurrent identity"
        );
        assert_eq!(staging_litter(tmp.path()), Vec::<String>::new());
    }

    #[test]
    fn approved_recovery_revalidates_already_applied_entries_before_finalizing() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::write(tmp.path().join("SOUL.md"), b"new soul").unwrap();
        fs::write(tmp.path().join("IDENTITY.md"), b"old identity").unwrap();
        let edits = vec![
            FileEdit::new("SOUL.md", b"new soul".to_vec()),
            FileEdit::new("IDENTITY.md", b"new identity".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("SOUL.md".to_string(), b"old soul".to_vec()),
            ("IDENTITY.md".to_string(), b"old identity".to_vec()),
        ]);
        let canonical_home = tmp.path().canonicalize().unwrap();
        set_conditional_write_actions(
            canonical_home.join("IDENTITY.md"),
            vec![(canonical_home.join("SOUL.md"), b"concurrent soul".to_vec())],
        );

        let error = ward
            .apply_after_threads_approval(
                &edits,
                &Authorization::signed_by("SHA256:principal-key"),
                &expected,
            )
            .expect_err("already-applied targets must be revalidated");

        assert!(
            format!("{error:#}").contains("changed before batch finalization"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            fs::read(tmp.path().join("SOUL.md")).unwrap(),
            b"concurrent soul"
        );
        assert_eq!(
            fs::read(tmp.path().join("IDENTITY.md")).unwrap(),
            b"old identity"
        );
    }

    #[test]
    fn approved_rollback_preserves_bytes_changed_during_rollback() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::write(tmp.path().join("SOUL.md"), b"old soul").unwrap();
        fs::write(tmp.path().join("IDENTITY.md"), b"old identity").unwrap();
        let edits = vec![
            FileEdit::new("SOUL.md", b"new soul".to_vec()),
            FileEdit::new("IDENTITY.md", b"new identity".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("SOUL.md".to_string(), b"old soul".to_vec()),
            ("IDENTITY.md".to_string(), b"old identity".to_vec()),
        ]);
        let canonical_home = tmp.path().canonicalize().unwrap();
        set_conditional_write_actions(
            canonical_home.join("IDENTITY.md"),
            vec![
                (canonical_home.join("SOUL.md"), b"concurrent soul".to_vec()),
                (
                    canonical_home.join("IDENTITY.md"),
                    b"concurrent identity".to_vec(),
                ),
            ],
        );

        let error = ward
            .apply_after_threads_approval(
                &edits,
                &Authorization::signed_by("SHA256:principal-key"),
                &expected,
            )
            .expect_err("rollback must not overwrite a concurrent target");

        assert!(
            format!("{error:#}").contains("conditional rollback also failed"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            fs::read(tmp.path().join("SOUL.md")).unwrap(),
            b"concurrent soul"
        );
        assert_eq!(
            fs::read(tmp.path().join("IDENTITY.md")).unwrap(),
            b"concurrent identity"
        );
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

    #[cfg(unix)]
    #[test]
    fn apply_does_not_follow_preplanted_staging_symlink() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let victim = tmp.path().join("victim.txt");
        fs::write(&victim, b"safe").unwrap();
        symlink(&victim, home.join("notes.md.ward-staged")).unwrap();

        let ward = ward_in(&home);
        let edits = vec![FileEdit::new("notes.md", b"new note".to_vec())];
        let report = ward.apply(&edits, &Authorization::unsigned()).unwrap();

        assert!(report.is_applied());
        assert_eq!(fs::read(&victim).unwrap(), b"safe");
        assert_eq!(fs::read(home.join("notes.md")).unwrap(), b"new note");
        // The attacker's decoy is untouched: still a symlink pointing at the
        // victim, and the randomized staging left no litter of its own.
        let decoy = home.join("notes.md.ward-staged");
        assert!(decoy.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&decoy).unwrap(), victim);
        assert_eq!(
            staging_litter(&home),
            vec!["notes.md.ward-staged".to_string()]
        );
    }

    #[test]
    fn failed_commit_cleans_up_staged_file() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        // The target exists as a directory: staging succeeds, the final
        // rename fails, and the staged file must not be left behind.
        fs::create_dir_all(tmp.path().join("scratch/notes.txt")).unwrap();

        let edits = vec![FileEdit::new("scratch/notes.txt", b"x".to_vec())];
        let result = ward.apply(&edits, &Authorization::unsigned());

        assert!(result.is_err());
        assert_eq!(
            staging_litter(&tmp.path().join("scratch")),
            Vec::<String>::new()
        );
    }

    #[cfg(unix)]
    #[test]
    fn apply_refuses_symlinked_parent_directory() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&outside).unwrap();
        // `memory` is pre-planted as a symlink escaping the home.
        symlink(&outside, home.join("memory")).unwrap();

        let ward = ward_in(&home);
        let edits = vec![FileEdit::new("memory/log.md", b"leak".to_vec())];
        let report = ward.apply(&edits, &Authorization::unsigned()).unwrap();

        // Gate 2 refuses before any byte is written outside the home.
        assert!(report.is_refused());
        assert!(!outside.join("log.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_fails_closed_when_parent_swapped_for_symlink() {
        use std::os::unix::fs::symlink;

        // Simulates the TOCTOU window: a directory component is swapped for
        // an escaping symlink *after* Gate 2 adjudication. The per-component
        // parent re-check must refuse rather than follow it — and must not
        // create directories outside the home as a side effect.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, home.join("memory")).unwrap();

        let canonical_home = home.canonicalize().unwrap();
        let decision = Decision {
            target: "memory/notes/log.md".into(),
            resolved: "memory/notes/log.md".into(),
            tier: Tier::Free,
            verdict: Verdict::Allow,
        };
        let err = write_atomic(
            &canonical_home,
            &canonical_home.join("memory/notes/log.md"),
            b"leak",
            &decision,
        )
        .unwrap_err();

        assert!(err.to_string().contains("not a real directory"));
        // Fail-closed with zero side effects outside the home.
        assert!(!outside.join("notes").exists());
        assert!(!outside.join("log.md").exists());
        assert_eq!(staging_litter(&outside), Vec::<String>::new());
    }

    #[test]
    fn apply_replaces_hard_linked_target_without_writing_through_it() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join("memory")).unwrap();
        let outside = tmp.path().join("outside.txt");
        fs::write(&outside, b"safe").unwrap();
        // The target is pre-planted as a hard link to a file outside the home.
        fs::hard_link(&outside, home.join("memory/log.md")).unwrap();

        let ward = ward_in(&home);
        let edits = vec![FileEdit::new("memory/log.md", b"new".to_vec())];
        let report = ward.apply(&edits, &Authorization::unsigned()).unwrap();

        // rename() replaces the directory entry and never writes through the
        // linked inode: the outside file keeps its bytes.
        assert!(report.is_applied());
        assert_eq!(fs::read(&outside).unwrap(), b"safe");
        assert_eq!(fs::read(home.join("memory/log.md")).unwrap(), b"new");
        // The audit still hashes the true prior contents of the target path.
        let audit = report.audit_records().next().unwrap();
        assert_eq!(audit.prev_sha256, Some(sha256_hex(b"safe")));
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
