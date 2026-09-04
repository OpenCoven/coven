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
//!    familiar's deterministic probe set before an explicit principal decision.
//!    Probe execution lives in the sibling `ward_probes` module; this module
//!    continues to represent the hold as [`Verdict::RequiresCoherenceReview`].
//! 4. **Audit logging** — modifications are recorded to an append-only log.
//!    [`Ward::apply`] emits a tamper-evident [`AuditRecord`] (before/after
//!    SHA-256 content hashes) for every Tier 2 write, and the daemon persists
//!    apply records to its audit ledger.
//!
//! This module implements the two **deterministic** gates — 1 and 2 — which are
//! the load-bearing structural checks. It has no dependency on the language
//! model, and every decision it makes is a pure function of the declared
//! surface and the proposal. [`Ward::apply`] is the **fail-closed enforcement
//! boundary**: it routes every write through Gates 1–2, refuses or holds the
//! whole proposal as a unit if any target is blocked or needs coherence review,
//! and only then writes — emitting Gate 4 audit records. Tier 0/1 changes stay
//! held here; the daemon's staged proposal flow attaches probe evidence for a
//! separate explicit coherence decision.
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
//! [`Ward::apply`] stages every cleared edit before changing any target, then
//! commits the batch through randomized sibling files
//! (`.{name}.ward-staged-<uuid>`, opened with `create_new`):
//!
//! - **Pre-planted staging symlinks or hard links** cannot be followed: the
//!   staging name is unpredictable and `create_new` refuses to open through
//!   an existing directory entry.
//! - **Symlinked targets or parent directories** are refused by Gate 2 before
//!   any byte is written. The target's parent is opened component-by-component
//!   without following links and retained as the authority for staging,
//!   commit, verification, rollback, and cleanup. Absolute paths retained
//!   beside those handles are diagnostic labels only.
//! - **Pre-existing hard-linked targets** are not written through by commit:
//!   atomic replacement changes the directory entry instead. Hard links are
//!   not generally harmless, however. A same-privilege process can mutate an
//!   installed inode through another open handle or link; when the Ward
//!   observes that drift, it preserves the entry and reports an ambiguous
//!   potentially-committed outcome instead of deleting it as Ward-owned.
//!
//! Direct and approved writes preserve the file actually displaced by commit.
//! Linux uses `openat2` with `RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS |
//! RESOLVE_NO_SYMLINKS` for parent and target opens, then `*at` operations for
//! every sibling mutation. Kernels without `openat2` fall back to
//! component-at-a-time `openat` with `O_NOFOLLOW`; because only one normalized
//! component is resolved per retained handle, that fallback cannot traverse a
//! replacement ancestor. macOS uses the corresponding
//! `openat`/`fstatat`/`linkat`/`unlinkat` operations and `renameatx_np` through
//! rustix. Windows retains non-share-delete directory handles, opens entries
//! without following reparse points, and moves exact source handles with
//! `SetFileInformationByHandle`; the stable absolute destination spelling is
//! safe because the retained handles prevent ancestor renames.
//! Linux and macOS exchange the staged and target entries. Windows moves the
//! target to a randomized backup and installs the staged entry with no-replace
//! semantics.
//! Both paths verify the displaced file identity, displaced before-image, and
//! installed bytes before the batch can finalize. Gate 4 hashes that verified
//! commit-displaced before-image.
//! Rollback uses the corresponding reverse operation and accepts success only
//! when the retained handles, identities, and contents prove the expected
//! transition. If that proof fails, the Ward leaves potentially concurrent
//! target entries in place and returns a typed ambiguous outcome with the
//! affected targets. A file that was absent during preparation is installed
//! with a no-replace hard link to its synced staging inode; rollback first moves
//! the entry to a randomized no-replace capture and removes it only when both
//! inode identity and contents still belong to that apply attempt.
//! Staging, backup, and rollback cleanup follows the same rule: capture without
//! replacement, inspect as a no-follow regular file, and reverify the retained
//! identity and bytes immediately before deletion. A replacement observed after
//! the first verification is restored to the artifact path (or left at the
//! capture path if restoration would overwrite another entry) and reported as a
//! cleanup failure. POSIX has no conditional unlink-by-inode primitive, so a
//! same-privilege replacement in the final verify-to-`unlinkat` interval remains
//! tracked by #924 rather than being overstated as closed here. Known
//! non-regular entries are rejected from metadata before open; the no-follow
//! open remains nonblocking to cover a concurrent type swap.
//!
//! Residual risk (accepted): on Unix, a same-privilege process can rename an
//! already-open parent directory. Ward operations remain attached to that
//! original directory object, even if it is moved elsewhere; they never follow
//! the replacement pathname. Windows prevents that rename while the retained
//! directory handle is live. Final-component replacement remains a separate
//! race: the Ward verifies the exact regular file displaced by commit and rolls
//! the batch back instead of emitting a false `prev_sha256`. Observed identity
//! or content drift always fails closed without overwriting or deleting the
//! unrecognized entry. The configured familiar-home spelling is rebound before
//! mutation, so retargeting an initially symlinked workspace also fails closed.

use std::collections::{BTreeMap, BTreeSet};
#[cfg(all(test, unix))]
use std::ffi::CString;
use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
#[cfg(any(target_os = "linux", not(unix)))]
use cap_fs_ext::DirExt;
#[cfg(not(unix))]
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::Dir;
#[cfg(not(unix))]
use cap_std::fs::OpenOptions as CapOpenOptions;
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

/// Shared Ward content ceiling for one retained regular-file image.
///
/// Gate-3 protected-surface baselines and both direct and approved writes use
/// this 16 MiB policy so one existing target cannot force the daemon to retain
/// an arbitrarily large before-image.
pub(crate) const WARD_FILE_CONTENT_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Maximum total proposed and before-image bytes retained by one Ward apply.
///
/// This includes the caller-owned proposed content buffers borrowed for the
/// duration of the apply and every existing-file before-image retained for
/// rollback and Gate-4 hashing, regardless of the edits' tiers.
pub(crate) const WARD_RETAINED_CONTENT_MAX_BYTES: u64 = WARD_FILE_CONTENT_MAX_BYTES;

/// Maximum edits in one submitted, approved, or recovered Ward apply.
///
/// An existing target can retain three descriptors through finalization: its
/// before-image, installed staging inode, and displaced backup. Capping at 32
/// bounds that worst case at 96 descriptors, leaving substantial headroom
/// under the portable low 256-descriptor soft limit common on macOS and Linux
/// for the daemon's sockets, database, logs, and unrelated concurrent work.
pub(crate) const WARD_EDIT_MAX_COUNT: usize = 32;

/// Fixed stack scratch used by bounded reads and content verification.
const DIRECT_VERIFICATION_SCRATCH_BYTES: usize = 64 * 1024;

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

/// One deterministic, offline Gate-3 probe declared in `ward.toml`.
///
/// The probe matches Gate-2-resolved surface paths. Parameters are deliberately
/// small and typed: `parse` uses `format`, while `pattern-lint` uses
/// `forbidden` and `required`. The other v1 probes have no parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeConfig {
    /// Glob over familiar-home-relative, forward-slashed surface paths.
    pub surface: String,
    /// Deterministic probe implementation to run.
    pub id: ProbeId,
    /// Declared parser for the `parse` probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ProbeFormat>,
    /// Regexes that must not match for `pattern-lint`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forbidden: Vec<String>,
    /// Regexes that must match for `pattern-lint`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
}

impl ProbeConfig {
    pub(crate) fn surface_matcher(&self) -> Result<globset::GlobMatcher> {
        Ok(compile_glob(&self.surface, false)?.compile_matcher())
    }
}

/// The four deterministic Gate-3 probes in the v1 contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProbeId {
    Parse,
    SizeDelta,
    ProtectedRegion,
    PatternLint,
}

/// Supported declared formats for the deterministic `parse` probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProbeFormat {
    Toml,
    Json,
    MarkdownFrontMatter,
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
    /// Deterministic, advisory Gate-3 probes. The singular field name maps to
    /// TOML's repeated `[[probe]]` tables.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probe: Vec<ProbeConfig>,
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

        for (index, probe) in self.probe.iter().enumerate() {
            if probe.surface.trim().is_empty() {
                bail!("probe[{index}] has an empty surface glob");
            }
            if probe.surface.starts_with(['/', '\\'])
                || probe.surface.contains('\\')
                || probe.surface.contains(':')
                || probe
                    .surface
                    .split('/')
                    .any(|segment| segment == "." || segment == "..")
            {
                bail!(
                    "probe[{index}] surface `{}` must stay relative to the familiar home",
                    probe.surface
                );
            }
            compile_glob(&probe.surface, false)
                .with_context(|| format!("invalid probe[{index}] surface glob"))?;

            let has_patterns = !probe.forbidden.is_empty() || !probe.required.is_empty();
            match probe.id {
                ProbeId::Parse if has_patterns => {
                    bail!("probe[{index}] `parse` does not accept regex parameters")
                }
                ProbeId::PatternLint if probe.format.is_some() => {
                    bail!("probe[{index}] `pattern-lint` does not accept `format`")
                }
                ProbeId::SizeDelta | ProbeId::ProtectedRegion
                    if probe.format.is_some() || has_patterns =>
                {
                    bail!(
                        "probe[{index}] `{}` does not accept parameters",
                        match probe.id {
                            ProbeId::SizeDelta => "size-delta",
                            ProbeId::ProtectedRegion => "protected-region",
                            _ => unreachable!("guard restricts this branch"),
                        }
                    )
                }
                _ => {}
            }
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

pub(crate) fn validate_file_edit_budget(edits: &[FileEdit]) -> Result<WardEditBudget> {
    let mut budget = WardEditBudget::for_edit_count(edits.len())?;
    for edit in edits {
        budget.reserve_proposed_content(&edit.new_contents)?;
    }
    Ok(budget)
}

pub(crate) fn staged_contents_decoded_len(encoding: &str, data: &str) -> Result<u64> {
    match encoding {
        "utf8" => u64::try_from(data.len()).map_err(|_| {
            WardEditBudgetFailure::BatchRetainedMemory {
                attempted_bytes: u64::MAX,
                max_bytes: WARD_RETAINED_CONTENT_MAX_BYTES,
            }
            .into()
        }),
        "base64" => base64_decoded_len(data),
        _ => bail!("staged contents use unknown encoding `{encoding}`"),
    }
}

fn base64_decoded_len(data: &str) -> Result<u64> {
    let mut encoded_bytes = 0_u64;
    let mut padding = 0_u64;
    let mut saw_padding = false;
    for byte in data.bytes() {
        if byte == b'\n' {
            continue;
        }
        encoded_bytes = encoded_bytes.checked_add(1).ok_or({
            WardEditBudgetFailure::BatchRetainedMemory {
                attempted_bytes: u64::MAX,
                max_bytes: WARD_RETAINED_CONTENT_MAX_BYTES,
            }
        })?;
        match byte {
            b'=' => {
                saw_padding = true;
                padding += 1;
                if padding > 2 {
                    bail!("staged base64 contents have invalid padding");
                }
            }
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' => {
                if saw_padding {
                    bail!("staged base64 contents have data after padding");
                }
            }
            other => bail!("staged base64 contents contain invalid byte {other:#04x}"),
        }
    }
    if !encoded_bytes.is_multiple_of(4) {
        bail!("staged base64 contents length is not a multiple of 4");
    }
    encoded_bytes
        .checked_div(4)
        .and_then(|chunks| chunks.checked_mul(3))
        .and_then(|bytes| bytes.checked_sub(padding))
        .ok_or_else(|| {
            WardEditBudgetFailure::BatchRetainedMemory {
                attempted_bytes: u64::MAX,
                max_bytes: WARD_RETAINED_CONTENT_MAX_BYTES,
            }
            .into()
        })
}

pub(crate) fn validate_staged_edit_budget(
    edits: &[coven_threads_core::StagedEdit],
) -> Result<WardEditBudget> {
    let mut budget = WardEditBudget::for_edit_count(edits.len())?;
    for edit in edits {
        let bytes = match &edit.contents {
            coven_threads_core::StagedContents::Utf8(contents) => {
                staged_contents_decoded_len("utf8", contents)?
            }
            coven_threads_core::StagedContents::Base64(contents) => {
                staged_contents_decoded_len("base64", contents)?
            }
        };
        budget.reserve_retained_content(bytes)?;
    }
    Ok(budget)
}

fn validate_approved_edit_budget(
    edits: &[FileEdit],
    expected_before: &BTreeMap<String, Option<Vec<u8>>>,
) -> Result<WardEditBudget> {
    let mut budget = validate_file_edit_budget(edits)?;
    WardEditBudget::for_edit_count(expected_before.len())?;
    for contents in expected_before.values().flatten() {
        let bytes = u64::try_from(contents.len()).map_err(|_| {
            WardEditBudgetFailure::BatchRetainedMemory {
                attempted_bytes: u64::MAX,
                max_bytes: WARD_RETAINED_CONTENT_MAX_BYTES,
            }
        })?;
        budget.reserve_retained_content(bytes)?;
    }
    Ok(budget)
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

/// Whether a conditional approved write is the first apply or crash recovery.
///
/// Only recovery may accept a target that already contains the staged
/// after-bytes. An initial apply must still observe the exact reviewed
/// before-image so same-byte concurrent writes cannot be mistaken for our own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApprovedApplyMode {
    Initial,
    Recovery,
}

#[derive(Debug)]
struct ApprovedApplyError {
    failure: ApprovedApplyFailure,
    source: anyhow::Error,
}

impl std::fmt::Display for ApprovedApplyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:#}", self.source)
    }
}

impl std::error::Error for ApprovedApplyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ApprovedApplyFailure {
    NoWrite,
    RolledBack,
    RolledBackCleanupFailed { targets: Vec<String> },
    Applied(ApplyReport),
    Ambiguous { targets: Vec<String> },
}

fn approved_apply_error(source: anyhow::Error, failure: ApprovedApplyFailure) -> anyhow::Error {
    ApprovedApplyError { failure, source }.into()
}

pub(crate) fn approved_apply_failure(error: &anyhow::Error) -> Option<&ApprovedApplyFailure> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<ApprovedApplyError>()
            .map(|failure| &failure.failure)
    })
}

/// Classify an approved-apply error for durable decision recovery.
///
/// `Some(false)` means the writer either never changed a target or proved that
/// its changes were rolled back. `Some(true)` means recovery state must remain
/// durable because a write may have committed. `None` is an unclassified error
/// and callers must conservatively preserve recovery state.
pub(crate) fn approved_apply_error_may_have_committed_write(error: &anyhow::Error) -> Option<bool> {
    approved_apply_failure(error).map(|failure| {
        matches!(
            failure,
            ApprovedApplyFailure::Applied(_) | ApprovedApplyFailure::Ambiguous { .. }
        )
    })
}

#[derive(Debug, Clone)]
pub(crate) enum DirectApplyFailure {
    RolledBack,
    RolledBackCleanupFailed { targets: Vec<String> },
    Applied(ApplyReport),
    Ambiguous { targets: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WardEditBudgetFailure {
    BatchEditCount {
        attempted_edits: usize,
        max_edits: usize,
    },
    ProposalEnvelopeBytes {
        attempted_bytes: u64,
        max_bytes: u64,
    },
    ExistingBeforeImage {
        target: String,
        observed_bytes: u64,
        max_bytes: u64,
    },
    BatchRetainedMemory {
        attempted_bytes: u64,
        max_bytes: u64,
    },
}

impl std::fmt::Display for WardEditBudgetFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BatchEditCount {
                attempted_edits,
                max_edits,
            } => write!(
                formatter,
                "Ward apply contains {attempted_edits} edits, \
                 over the {max_edits}-edit limit"
            ),
            Self::ProposalEnvelopeBytes {
                attempted_bytes,
                max_bytes,
            } => write!(
                formatter,
                "Ward proposal envelope is {attempted_bytes} bytes, \
                 over the {max_bytes}-byte encoded-file limit"
            ),
            Self::ExistingBeforeImage {
                target,
                observed_bytes,
                max_bytes,
            } => write!(
                formatter,
                "Ward target `{target}` retained before-image is {observed_bytes} bytes, \
                 over the {max_bytes}-byte per-file limit"
            ),
            Self::BatchRetainedMemory {
                attempted_bytes,
                max_bytes,
            } => write!(
                formatter,
                "Ward apply batch retained-memory would be {attempted_bytes} bytes, \
                 over the {max_bytes}-byte limit"
            ),
        }
    }
}

impl std::error::Error for WardEditBudgetFailure {}

pub(crate) fn ward_edit_budget_failure(error: &anyhow::Error) -> Option<&WardEditBudgetFailure> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<WardEditBudgetFailure>())
}

/// Count and retained-content accounting shared by submitted, approved, and
/// recovered Ward edits.
///
/// Callers can initialize this from an untrusted edit count, then reserve
/// borrowed proposed-content lengths without cloning or allocating. Existing
/// before-images reserve from the same aggregate budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WardEditBudget {
    retained_content_bytes: u64,
}

impl WardEditBudget {
    pub(crate) fn for_edit_count(edit_count: usize) -> Result<Self> {
        if edit_count > WARD_EDIT_MAX_COUNT {
            return Err(WardEditBudgetFailure::BatchEditCount {
                attempted_edits: edit_count,
                max_edits: WARD_EDIT_MAX_COUNT,
            }
            .into());
        }
        Ok(Self {
            retained_content_bytes: 0,
        })
    }

    pub(crate) fn reserve_retained_content(&mut self, additional_bytes: u64) -> Result<()> {
        self.retained_content_bytes =
            reserve_ward_content_bytes(self.retained_content_bytes, additional_bytes)?;
        Ok(())
    }

    pub(crate) fn reserve_proposed_content(&mut self, contents: &[u8]) -> Result<()> {
        let bytes = u64::try_from(contents.len()).map_err(|_| {
            WardEditBudgetFailure::BatchRetainedMemory {
                attempted_bytes: u64::MAX,
                max_bytes: WARD_RETAINED_CONTENT_MAX_BYTES,
            }
        })?;
        self.reserve_retained_content(bytes)
    }

    pub(crate) fn retained_content_bytes(self) -> u64 {
        self.retained_content_bytes
    }
}

#[derive(Debug)]
struct StagingCleanupError {
    path: PathBuf,
    source: anyhow::Error,
}

impl std::fmt::Display for StagingCleanupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:#}", self.source)
    }
}

impl std::error::Error for StagingCleanupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn staging_cleanup_error(path: &Path, source: anyhow::Error) -> anyhow::Error {
    StagingCleanupError {
        path: path.to_path_buf(),
        source,
    }
    .into()
}

fn staging_cleanup_failure_path(error: &anyhow::Error) -> Option<&Path> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<StagingCleanupError>()
            .map(|failure| failure.path.as_path())
    })
}

#[derive(Debug)]
struct DirectRollbackCleanupError {
    targets: Vec<String>,
    messages: Vec<String>,
}

impl std::fmt::Display for DirectRollbackCleanupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "removing rollback artifacts failed: {}",
            self.messages.join("; ")
        )
    }
}

#[derive(Debug)]
struct DirectApplyError {
    failure: DirectApplyFailure,
    source: anyhow::Error,
}

impl std::fmt::Display for DirectApplyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:#}", self.source)
    }
}

impl std::error::Error for DirectApplyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn direct_apply_error(source: anyhow::Error, failure: DirectApplyFailure) -> anyhow::Error {
    DirectApplyError { failure, source }.into()
}

pub(crate) fn direct_apply_failure(error: &anyhow::Error) -> Option<&DirectApplyFailure> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<DirectApplyError>()
            .map(|failure| &failure.failure)
    })
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
    /// SHA-256 (hex) of the verified commit-displaced regular-file contents,
    /// or `None` when a no-replace creation succeeded.
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

#[cfg(test)]
thread_local! {
    static EVALUATE_CALL_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_evaluate_call_count() {
    EVALUATE_CALL_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn evaluate_call_count() -> usize {
    EVALUATE_CALL_COUNT.with(std::cell::Cell::get)
}

#[cfg(test)]
fn record_evaluate_call() {
    EVALUATE_CALL_COUNT.with(|count| count.set(count.get() + 1));
}

#[cfg(not(test))]
fn record_evaluate_call() {}

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
        self.evaluate_with_home(proposal, None)
    }

    fn evaluate_with_home(&self, proposal: &Proposal, canonical_home: Option<&Path>) -> Outcome {
        record_evaluate_call();
        let decisions = proposal
            .targets
            .iter()
            .map(|target| self.evaluate_target(target, &proposal.authorization, canonical_home))
            .collect();
        Outcome { decisions }
    }

    fn evaluate_target(
        &self,
        target: &str,
        authorization: &Authorization,
        canonical_home: Option<&Path>,
    ) -> Decision {
        // Gate 2: surface discrimination — resolve the real target.
        let resolved = match canonical_home {
            Some(canonical_home) => self.materialize_with_home(target, canonical_home),
            None => self.materialize(target),
        };
        let resolved = match resolved {
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
        let canonical_home = self
            .home
            .canonicalize()
            .map_err(|err| BlockReason::Unresolvable {
                detail: format!("home `{}`: {err}", self.home.display()),
            })?;
        self.materialize_with_home(target, &canonical_home)
    }

    fn materialize_with_home(
        &self,
        target: &str,
        canonical_home: &Path,
    ) -> std::result::Result<String, BlockReason> {
        // 1. Lexically normalize the joined path (fold `.` and `..`). A target
        //    that would climb above the home is a traversal escape.
        let normalized =
            lexical_join(canonical_home, target).ok_or(BlockReason::TraversalEscape)?;

        // 2. Resolve symlinks on the longest existing prefix. If the canonical
        //    prefix leaves the canonical home, it is a symlink escape.
        let resolved_abs = resolve_within(canonical_home, &normalized)?;

        // 3. Express the resolved path relative to the home, forward-slashed.
        let rel = resolved_abs
            .strip_prefix(canonical_home)
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
    ///   change authorized by Gate 1 — the direct apply path *holds* the
    ///   proposal as a unit. Probe evidence and the principal's decision are
    ///   handled by the daemon's staged proposal flow.
    /// - Otherwise every edit is Tier 2/3: each is written atomically (staged
    ///   as a randomized `create_new` sibling in the target's re-verified
    ///   directory, then renamed into place) and every Tier 2 write emits a
    ///   Gate 4 [`AuditRecord`].
    ///
    /// Because writes are routed through [`Ward::evaluate`] first, Gate 2 path
    /// confinement applies to the apply too: an edit that resolves out of the
    /// familiar home (via `..`, symlink, or case collision) is refused before
    /// any byte is written. Count and proposed-content budgets run before
    /// target cloning or Gate 2; budget and I/O failures return `Err`, while a
    /// refusal or hold is a normal [`ApplyReport`].
    pub fn apply(&self, edits: &[FileEdit], authorization: &Authorization) -> Result<ApplyReport> {
        validate_file_edit_budget(edits)?;
        let anchored_home = AnchoredHome::open(&self.home)?;
        let proposal = Proposal {
            targets: edits.iter().map(|e| e.target.clone()).collect(),
            authorization: authorization.clone(),
        };
        let outcome = self.evaluate_with_home(&proposal, Some(&anchored_home.absolute));
        maybe_swap_evaluated_home(&self.home)?;

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
        anchored_home.verify_path_unchanged()?;

        // Every edit is Tier 2/3 and cleared. Stage and commit the batch as one
        // rollback unit so a later failure cannot strand an unaudited write.
        write_direct_batch(&anchored_home, edits, outcome.decisions)
    }

    /// Apply edits after an explicit principal proposal approval has cleared the
    /// daemon-side threads replay. Gate 1 and Gate 2 still run here; Gate 3's
    /// "held for review" state is the decision this endpoint represents.
    pub(crate) fn apply_after_threads_approval(
        &self,
        edits: &[FileEdit],
        authorization: &Authorization,
        expected_before: &BTreeMap<String, Vec<u8>>,
        expected_resolved: &BTreeMap<String, String>,
        mode: ApprovedApplyMode,
    ) -> Result<ApplyReport> {
        (|| -> Result<()> {
            let mut budget = validate_file_edit_budget(edits)?;
            WardEditBudget::for_edit_count(expected_before.len())?;
            for contents in expected_before.values() {
                let bytes = u64::try_from(contents.len()).map_err(|_| {
                    WardEditBudgetFailure::BatchRetainedMemory {
                        attempted_bytes: u64::MAX,
                        max_bytes: WARD_RETAINED_CONTENT_MAX_BYTES,
                    }
                })?;
                budget.reserve_retained_content(bytes)?;
            }
            Ok(())
        })()
        .map_err(|error| approved_apply_error(error, ApprovedApplyFailure::NoWrite))?;
        let anchored_home = AnchoredHome::open(&self.home)
            .map_err(|error| approved_apply_error(error, ApprovedApplyFailure::NoWrite))?;
        let proposal = Proposal {
            targets: edits.iter().map(|e| e.target.clone()).collect(),
            authorization: authorization.clone(),
        };
        let outcome = self.evaluate_with_home(&proposal, Some(&anchored_home.absolute));
        maybe_swap_evaluated_home(&self.home)
            .map_err(|error| approved_apply_error(error, ApprovedApplyFailure::NoWrite))?;
        ensure_expected_resolutions(&outcome.decisions, expected_resolved)
            .map_err(|error| approved_apply_error(error, ApprovedApplyFailure::NoWrite))?;
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
        anchored_home
            .verify_path_unchanged()
            .map_err(|error| approved_apply_error(error, ApprovedApplyFailure::NoWrite))?;
        let expected_before = expected_before
            .iter()
            .map(|(target, contents)| (target.clone(), Some(contents.clone())))
            .collect();
        let changes = write_atomically_if_unchanged(
            &anchored_home,
            edits,
            outcome.decisions,
            &expected_before,
            mode,
        )?;
        Ok(ApplyReport { changes })
    }

    /// Apply edits after an explicit principal coherence decision.
    ///
    /// This is deliberately narrower than [`Ward::apply_after_threads_approval`]:
    /// it clears only [`Verdict::RequiresCoherenceReview`]. A protected target
    /// remains refused even when it carries valid Gate-1 authorization, and
    /// Gate-2 or authorization failures remain refused. `expected_before`
    /// binds the apply to the surface snapshot reviewed by the principal;
    /// `None` represents a reviewed target that did not exist at staging time.
    pub(crate) fn apply_after_coherence_approval(
        &self,
        edits: &[FileEdit],
        authorization: &Authorization,
        expected_before: &BTreeMap<String, Option<Vec<u8>>>,
        expected_resolved: &BTreeMap<String, String>,
        mode: ApprovedApplyMode,
    ) -> Result<ApplyReport> {
        validate_approved_edit_budget(edits, expected_before)
            .map_err(|error| approved_apply_error(error, ApprovedApplyFailure::NoWrite))?;
        let anchored_home = AnchoredHome::open(&self.home)
            .map_err(|error| approved_apply_error(error, ApprovedApplyFailure::NoWrite))?;
        let proposal = Proposal {
            targets: edits.iter().map(|edit| edit.target.clone()).collect(),
            authorization: authorization.clone(),
        };
        let outcome = self.evaluate_with_home(&proposal, Some(&anchored_home.absolute));
        maybe_swap_evaluated_home(&self.home)
            .map_err(|error| approved_apply_error(error, ApprovedApplyFailure::NoWrite))?;
        ensure_expected_resolutions(&outcome.decisions, expected_resolved)
            .map_err(|error| approved_apply_error(error, ApprovedApplyFailure::NoWrite))?;
        let has_reviewed_target = outcome
            .decisions
            .iter()
            .any(|decision| matches!(decision.verdict, Verdict::RequiresCoherenceReview));
        let refused = !has_reviewed_target
            || outcome.decisions.iter().any(|decision| {
                matches!(
                    decision.verdict,
                    Verdict::AuthorizedProtectedChange | Verdict::Blocked { .. }
                )
            });
        if refused {
            let changes = outcome
                .decisions
                .into_iter()
                .map(|decision| AppliedChange {
                    disposition: Disposition::Refused,
                    decision,
                    audit: None,
                })
                .collect();
            return Ok(ApplyReport { changes });
        }
        anchored_home
            .verify_path_unchanged()
            .map_err(|error| approved_apply_error(error, ApprovedApplyFailure::NoWrite))?;
        let changes = write_atomically_if_unchanged(
            &anchored_home,
            edits,
            outcome.decisions,
            expected_before,
            mode,
        )?;
        Ok(ApplyReport { changes })
    }
}

fn ensure_expected_resolutions(
    decisions: &[Decision],
    expected_resolved: &BTreeMap<String, String>,
) -> Result<()> {
    if decisions.len() != expected_resolved.len() {
        bail!("approved Gate-2 resolutions do not match the proposed targets");
    }
    let mut seen = BTreeSet::new();
    for decision in decisions {
        if !seen.insert(decision.target.as_str()) {
            bail!(
                "approved proposal contains duplicate target `{}`",
                decision.target
            );
        }
        let expected = expected_resolved.get(&decision.target).with_context(|| {
            format!(
                "missing approved Gate-2 resolution for `{}`",
                decision.target
            )
        })?;
        if expected != &decision.resolved {
            bail!(
                "approved target `{}` changed Gate-2 resolution from `{expected}` to `{}`",
                decision.target,
                decision.resolved
            );
        }
    }
    Ok(())
}

/// Whether a verdict needs Gate 3 coherence review before it can be applied.
///
/// This includes Gate-1 authorized Tier 0 changes. The direct apply path cannot
/// represent the staged probe evidence and principal decision, so it fails
/// closed — hold rather than write.
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

struct AnchoredHome {
    dir: Arc<Dir>,
    configured: PathBuf,
    absolute: PathBuf,
}

impl AnchoredHome {
    fn open(home: &Path) -> Result<Self> {
        let absolute = home
            .canonicalize()
            .with_context(|| format!("ward home `{}` is not resolvable", home.display()))?;
        let dir = Dir::open_ambient_dir(&absolute, ambient_authority())
            .with_context(|| format!("opening familiar home {}", absolute.display()))?;
        let anchored = Self {
            dir: Arc::new(dir),
            configured: home.to_path_buf(),
            absolute,
        };
        if !anchored.binding_matches()? {
            bail!(
                "ward home `{}` changed while its authority handle was opened",
                home.display()
            );
        }
        Ok(anchored)
    }

    fn verify_path_unchanged(&self) -> Result<()> {
        if self.binding_matches()? {
            Ok(())
        } else {
            bail!(
                "ward home `{}` changed during Gate 2; refusing to write",
                self.configured.display()
            )
        }
    }

    fn binding_matches(&self) -> Result<bool> {
        let Ok(current_absolute) = self.configured.canonicalize() else {
            return Ok(false);
        };
        if current_absolute != self.absolute {
            return Ok(false);
        }
        directory_handle_matches_path(&self.dir, &current_absolute)
    }
}

#[cfg(unix)]
fn directory_handle_matches_path(directory: &Dir, path: &Path) -> Result<bool> {
    use std::os::fd::AsFd;
    use std::os::unix::fs::MetadataExt;

    let opened = rustix::fs::fstat(directory.as_fd())
        .map_err(std::io::Error::from)
        .with_context(|| format!("reading familiar-home handle identity {}", path.display()))?;
    let named = std::fs::symlink_metadata(path)
        .with_context(|| format!("reading familiar-home path identity {}", path.display()))?;
    Ok(named.file_type().is_dir()
        && opened.st_dev as u64 == named.dev()
        && opened.st_ino as u64 == named.ino())
}

#[cfg(windows)]
fn directory_handle_matches_path(directory: &Dir, path: &Path) -> Result<bool> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let opened = directory
        .try_clone()
        .context("duplicating familiar-home authority handle")?
        .into_std_file();
    let named = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .with_context(|| format!("opening familiar-home path identity {}", path.display()))?;
    let metadata = named
        .metadata()
        .with_context(|| format!("reading familiar-home path metadata {}", path.display()))?;
    Ok(metadata.is_dir()
        && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
        && windows_file_identity_from_open_file(&opened)?
            == windows_file_identity_from_open_file(&named)?)
}

#[cfg(not(any(unix, windows)))]
fn directory_handle_matches_path(_directory: &Dir, _path: &Path) -> Result<bool> {
    bail!("familiar-home identity comparison is unsupported on this platform")
}

#[derive(Clone)]
struct AnchoredEntry {
    parent: Arc<Dir>,
    name: OsString,
    absolute: PathBuf,
}

impl AnchoredEntry {
    fn new(parent: Arc<Dir>, parent_path: &Path, name: &OsStr) -> Self {
        Self {
            parent,
            name: name.to_os_string(),
            absolute: parent_path.join(name),
        }
    }

    fn sibling(&self, name: OsString) -> Self {
        Self {
            parent: Arc::clone(&self.parent),
            absolute: self
                .absolute
                .parent()
                .expect("anchored entry has a parent")
                .join(&name),
            name,
        }
    }
}

impl Deref for AnchoredEntry {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.absolute
    }
}

impl AsRef<Path> for AnchoredEntry {
    fn as_ref(&self) -> &Path {
        &self.absolute
    }
}

impl PartialEq for AnchoredEntry {
    fn eq(&self, other: &Self) -> bool {
        self.absolute == other.absolute
    }
}

impl Eq for AnchoredEntry {}

impl std::fmt::Debug for AnchoredEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.absolute.fmt(formatter)
    }
}

impl PartialEq<PathBuf> for AnchoredEntry {
    fn eq(&self, other: &PathBuf) -> bool {
        self.absolute == *other
    }
}

impl PartialEq<&Path> for AnchoredEntry {
    fn eq(&self, other: &&Path) -> bool {
        self.absolute == *other
    }
}

struct AnchoredParent {
    dir: Arc<Dir>,
    absolute: PathBuf,
}

impl AnchoredParent {
    fn entry(&self, name: &OsStr) -> AnchoredEntry {
        AnchoredEntry::new(Arc::clone(&self.dir), &self.absolute, name)
    }
}

#[derive(Clone)]
struct ApprovedWritePaths {
    staged: AnchoredEntry,
    displaced: AnchoredEntry,
}

impl ApprovedWritePaths {
    fn new(target: &AnchoredEntry, staged: AnchoredEntry) -> Result<Self> {
        let displaced = approved_write_displaced_path(target, &staged)?;
        Ok(Self { staged, displaced })
    }
}

struct OpenRegularFile {
    file: std::fs::File,
    contents: Vec<u8>,
}

struct PreparedDirectWrite<'a> {
    path: AnchoredEntry,
    paths: Option<ApprovedWritePaths>,
    before: Option<OpenRegularFile>,
    installed: Option<std::fs::File>,
    displaced: Option<std::fs::File>,
    new_contents: &'a [u8],
    decision: Decision,
}

struct PreparedConditionalWrite<'a> {
    path: AnchoredEntry,
    paths: Option<ApprovedWritePaths>,
    already_applied: bool,
    expected_before: Option<Vec<u8>>,
    observed: Option<OpenRegularFile>,
    installed: Option<std::fs::File>,
    displaced: Option<std::fs::File>,
    audit_before_sha256: Option<String>,
    new_contents: &'a [u8],
    decision: Decision,
}

fn write_direct_batch(
    home: &AnchoredHome,
    edits: &[FileEdit],
    decisions: Vec<Decision>,
) -> Result<ApplyReport> {
    let mut retained_bytes = validate_file_edit_budget(edits)?.retained_content_bytes();
    let mut prepared = Vec::with_capacity(edits.len());
    for (edit, decision) in edits.iter().zip(decisions) {
        let result = (|| -> Result<PreparedDirectWrite> {
            let resolved = join_resolved(&home.absolute, &decision.resolved);
            let parent = resolved
                .parent()
                .ok_or_else(|| anyhow!("target has no parent directory: {}", resolved.display()))?;
            let name = resolved
                .file_name()
                .ok_or_else(|| anyhow!("target has no file name: {}", resolved.display()))?;
            let canonical_parent = prepare_staging_parent(home, parent)?;
            let path = canonical_parent.entry(name);
            maybe_swap_prepared_parent(&path)?;
            let before = open_direct_before_image(&path, &decision.target, retained_bytes)?;
            if let Some(before) = &before {
                retained_bytes = reserve_ward_content_bytes(
                    retained_bytes,
                    u64::try_from(before.contents.len()).map_err(|_| {
                        WardEditBudgetFailure::BatchRetainedMemory {
                            attempted_bytes: u64::MAX,
                            max_bytes: WARD_RETAINED_CONTENT_MAX_BYTES,
                        }
                    })?,
                )?;
            }
            Ok(PreparedDirectWrite {
                path,
                paths: None,
                before,
                installed: None,
                displaced: None,
                new_contents: edit.new_contents.as_slice(),
                decision,
            })
        })();
        match result {
            Ok(write) => prepared.push(write),
            Err(error) => {
                return fail_after_direct_rollback(&prepared, &[], error);
            }
        }
    }

    for index in 0..prepared.len() {
        if let Err(error) = stage_direct_write(&mut prepared[index]) {
            return fail_after_direct_rollback(&prepared, &[], error);
        }
    }

    let mut swapped = Vec::new();
    for index in 0..prepared.len() {
        let commit = {
            let write = &prepared[index];
            let paths = write
                .paths
                .as_ref()
                .context("prepared direct write has no staging paths")?;
            if let Err(error) = maybe_run_conditional_write_hook(&write.path) {
                return fail_after_direct_rollback(&prepared, &swapped, error);
            }
            if write.before.is_some() {
                replace_preserving_target(&write.path, &paths.staged, &paths.displaced)
            } else {
                hard_link_without_replace(&paths.staged, &write.path).with_context(|| {
                    format!(
                        "target `{}` appeared during commit; refusing to overwrite it",
                        write.decision.target
                    )
                })
            }
        };
        if let Err(error) = commit {
            let write = &prepared[index];
            let paths = write
                .paths
                .as_ref()
                .context("prepared direct write has no staging paths")?;
            let staged = write
                .installed
                .as_ref()
                .context("prepared direct write has no staged-file identity")?;
            let staged_at_target = match open_file_matches_path_if_present(staged, &write.path) {
                Ok(matches) => matches,
                Err(identity_error) => {
                    return fail_after_direct_rollback(
                        &prepared,
                        &swapped,
                        anyhow!(
                            "{error:#}; checking failed commit ownership also failed: \
                             {identity_error:#}"
                        ),
                    );
                }
            };
            if staged_at_target
                || (write.before.is_some() && failed_replace_displaced_target(&paths.displaced))
            {
                swapped.push(index);
            }
            return fail_after_direct_rollback(&prepared, &swapped, error);
        }
        swapped.push(index);

        let verification = (|| -> Result<()> {
            let write = &mut prepared[index];
            let paths = write
                .paths
                .as_ref()
                .context("prepared direct write has no staging paths")?;
            let staged = write
                .installed
                .as_ref()
                .context("prepared direct write has no staged-file identity")?;
            let changed = format!("target `{}` changed during commit", write.decision.target);
            verify_installed_regular_target(
                &write.path,
                write.new_contents,
                staged,
                "committed direct target unexpectedly disappeared",
                &changed,
            )?;
            if write.before.is_some() {
                let before = write
                    .before
                    .as_mut()
                    .context("existing direct target lost its before-image")?;
                verify_displaced_regular_target(
                    &mut before.file,
                    &before.contents,
                    &paths.displaced,
                    &mut write.displaced,
                    &changed,
                )?;
            } else if !open_file_matches_path(staged, &paths.staged)? {
                bail!("target `{}` changed during commit", write.decision.target);
            }
            Ok(())
        })();
        if let Err(error) = verification {
            return fail_after_direct_rollback(&prepared, &swapped, error);
        }
    }

    let final_verification = (|| -> Result<()> {
        for write in &mut prepared {
            let installed = write
                .installed
                .as_ref()
                .context("committed direct write has no staged-file handle")?;
            let changed = format!(
                "target `{}` changed before batch finalization",
                write.decision.target
            );
            verify_installed_regular_target(
                &write.path,
                write.new_contents,
                installed,
                "direct target unexpectedly disappeared before batch finalization",
                &changed,
            )?;
            if let Some(before) = write.before.as_mut() {
                let paths = write
                    .paths
                    .as_ref()
                    .context("committed direct write has no staging paths")?;
                verify_displaced_regular_target(
                    &mut before.file,
                    &before.contents,
                    &paths.displaced,
                    &mut write.displaced,
                    &changed,
                )?;
            }
        }
        Ok(())
    })();
    if let Err(error) = final_verification {
        return fail_after_direct_rollback(&prepared, &swapped, error);
    }

    let report = direct_apply_report(&prepared);
    if let Err(error) = cleanup_direct_staging(&prepared) {
        return Err(direct_apply_error(
            error.context("direct batch was applied but backup cleanup failed"),
            DirectApplyFailure::Applied(report),
        ));
    }
    Ok(report)
}

fn reserve_ward_content_bytes(retained: u64, additional: u64) -> Result<u64> {
    let attempted =
        retained
            .checked_add(additional)
            .ok_or(WardEditBudgetFailure::BatchRetainedMemory {
                attempted_bytes: u64::MAX,
                max_bytes: WARD_RETAINED_CONTENT_MAX_BYTES,
            })?;
    if attempted > WARD_RETAINED_CONTENT_MAX_BYTES {
        return Err(WardEditBudgetFailure::BatchRetainedMemory {
            attempted_bytes: attempted,
            max_bytes: WARD_RETAINED_CONTENT_MAX_BYTES,
        }
        .into());
    }
    Ok(attempted)
}

fn direct_apply_report(prepared: &[PreparedDirectWrite<'_>]) -> ApplyReport {
    ApplyReport {
        changes: prepared
            .iter()
            .map(|write| {
                let audit = (write.decision.tier == Tier::Logged).then(|| AuditRecord {
                    target: write.decision.target.clone(),
                    resolved: write.decision.resolved.clone(),
                    tier: write.decision.tier,
                    prev_sha256: write
                        .before
                        .as_ref()
                        .map(|before| sha256_hex(&before.contents)),
                    next_sha256: sha256_hex(write.new_contents),
                    bytes_written: write.new_contents.len(),
                });
                AppliedChange {
                    decision: write.decision.clone(),
                    disposition: Disposition::Applied,
                    audit,
                }
            })
            .collect(),
    }
}

fn fail_after_direct_rollback(
    prepared: &[PreparedDirectWrite<'_>],
    swapped: &[usize],
    error: anyhow::Error,
) -> Result<ApplyReport> {
    let mut known_cleanup_targets = BTreeSet::new();
    if let Some(write) = staging_cleanup_failure_path(&error)
        .and_then(|path| prepared.iter().find(|write| write.path == path))
    {
        known_cleanup_targets.insert(write.decision.target.clone());
    }
    let rollback = rollback_direct_writes(prepared, swapped);
    match rollback {
        Ok(()) => {
            let (unresolved, ownership_errors) = find_unresolved_direct_targets(prepared);
            if !unresolved.is_empty() {
                let targets = unresolved
                    .iter()
                    .map(|&index| prepared[index].decision.target.clone())
                    .collect();
                return Err(direct_apply_error(
                    anyhow!(
                        "{error:#}; direct rollback left potentially concurrent target entries \
                         in place: {}",
                        ownership_errors.join("; ")
                    ),
                    DirectApplyFailure::Ambiguous { targets },
                ));
            }
            match cleanup_direct_rollback_staging(prepared) {
                Ok(()) if known_cleanup_targets.is_empty() => Err(direct_apply_error(
                    error.context("direct batch was rolled back"),
                    DirectApplyFailure::RolledBack,
                )),
                Ok(()) => Err(direct_apply_error(
                    error
                        .context("direct batch was rolled back but staging cleanup was incomplete"),
                    DirectApplyFailure::RolledBackCleanupFailed {
                        targets: prepared
                            .iter()
                            .filter(|write| known_cleanup_targets.contains(&write.decision.target))
                            .map(|write| write.decision.target.clone())
                            .collect(),
                    },
                )),
                Err(cleanup_error) => {
                    known_cleanup_targets.extend(cleanup_error.targets.iter().cloned());
                    for &index in swapped {
                        known_cleanup_targets.insert(prepared[index].decision.target.clone());
                    }
                    Err(direct_apply_error(
                        anyhow!(
                            "{error:#}; direct batch was rolled back but cleanup failed: \
                             {cleanup_error:#}"
                        ),
                        DirectApplyFailure::RolledBackCleanupFailed {
                            targets: prepared
                                .iter()
                                .filter(|write| {
                                    known_cleanup_targets.contains(&write.decision.target)
                                })
                                .map(|write| write.decision.target.clone())
                                .collect(),
                        },
                    ))
                }
            }
        }
        Err(rollback_error) => {
            let targets = swapped
                .iter()
                .map(|&index| prepared[index].decision.target.clone())
                .collect();
            Err(direct_apply_error(
                anyhow!("{error:#}; direct rollback was not fully proven: {rollback_error:#}"),
                DirectApplyFailure::Ambiguous { targets },
            ))
        }
    }
}

fn rollback_direct_writes(prepared: &[PreparedDirectWrite<'_>], swapped: &[usize]) -> Result<()> {
    let mut errors = Vec::new();
    for &index in swapped.iter().rev() {
        let write = &prepared[index];
        let staged = match &write.installed {
            Some(staged) => staged,
            None => {
                errors.push(format!(
                    "{}: committed write has no staged-file identity",
                    write.decision.target
                ));
                continue;
            }
        };
        let paths = match &write.paths {
            Some(paths) => paths,
            None => {
                errors.push(format!(
                    "{}: committed write has no staging paths",
                    write.decision.target
                ));
                continue;
            }
        };
        let rollback = match &write.before {
            Some(_) => rollback_replaced_regular_target(
                &write.path,
                paths,
                write.new_contents,
                staged,
                write.displaced.as_ref(),
            ),
            None => rollback_direct_created_write(write, staged),
        };
        if let Err(error) = rollback {
            errors.push(format!("{}: {error:#}", write.decision.target));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!("direct rollback failed: {}", errors.join("; "))
    }
}

fn find_unresolved_direct_targets(
    prepared: &[PreparedDirectWrite<'_>],
) -> (Vec<usize>, Vec<String>) {
    let mut unresolved = Vec::new();
    let mut errors = Vec::new();
    for (index, write) in prepared.iter().enumerate() {
        let Some(staged) = &write.installed else {
            continue;
        };
        match open_file_matches_path_if_present(staged, &write.path) {
            Ok(false) => continue,
            Ok(true) => {
                unresolved.push(index);
                errors.push(format!(
                    "{}: Ward staging identity remains at the target",
                    write.decision.target
                ));
            }
            Err(error) => {
                unresolved.push(index);
                errors.push(format!(
                    "{}: checking ambiguous target ownership failed: {error:#}",
                    write.decision.target
                ));
            }
        }
    }
    (unresolved, errors)
}

fn rollback_direct_created_write(
    write: &PreparedDirectWrite<'_>,
    staged: &std::fs::File,
) -> Result<()> {
    let captured = rollback_capture_path(&write.path);
    atomic_move_without_replace(&write.path, &captured)
        .with_context(|| format!("capturing created direct target {}", write.path.display()))?;
    let verification = (|| -> Result<()> {
        if !open_file_matches_path(staged, &captured)? {
            bail!(
                "created target changed during direct rollback {}",
                write.path.display()
            );
        }
        let mut captured_file = open_regular_file_handle_without_following_links(&captured)?
            .context("captured direct target unexpectedly disappeared")?;
        if stream_regular_file_matches_and_sha256(&mut captured_file, write.new_contents)?.is_none()
        {
            bail!(
                "created target bytes changed during direct rollback {}",
                write.path.display()
            );
        }
        Ok(())
    })();
    if let Err(error) = verification {
        atomic_move_without_replace(&captured, &write.path)
            .with_context(|| format!("restoring changed direct target: {error:#}"))?;
        return Err(error);
    }
    remove_owned_regular_artifact(
        &captured,
        staged,
        Some(write.new_contents),
        "captured direct target",
    )
}

fn cleanup_direct_rollback_staging(
    prepared: &[PreparedDirectWrite<'_>],
) -> std::result::Result<(), DirectRollbackCleanupError> {
    let mut errors = Vec::new();
    let mut targets = Vec::new();
    for write in prepared {
        let Some(paths) = &write.paths else {
            continue;
        };
        if let Err(error) = maybe_fail_direct_cleanup(&write.path) {
            errors.push(format!("{}: {error:#}", write.path.display()));
            targets.push(write.decision.target.clone());
            continue;
        }
        let Some(installed) = write.installed.as_ref() else {
            errors.push(format!(
                "{}: rollback artifact has no retained staged-file identity",
                write.path.display()
            ));
            targets.push(write.decision.target.clone());
            continue;
        };
        if let Err(error) = maybe_replace_cleanup_artifact(&write.path, &paths.staged) {
            errors.push(format!("{}: {error:#}", paths.staged.display()));
            targets.push(write.decision.target.clone());
            continue;
        }
        if let Err(error) = remove_owned_regular_artifact(
            &paths.staged,
            installed,
            Some(write.new_contents),
            "direct rollback staging",
        ) {
            errors.push(format!("{}: {error:#}", paths.staged.display()));
            targets.push(write.decision.target.clone());
        }
        if paths.displaced != paths.staged {
            if let Err(error) = ensure_artifact_path_absent(
                &paths.displaced,
                "direct rollback displaced-backup path",
            ) {
                errors.push(format!("{}: {error:#}", paths.displaced.display()));
                if !targets.contains(&write.decision.target) {
                    targets.push(write.decision.target.clone());
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(DirectRollbackCleanupError {
            targets,
            messages: errors,
        })
    }
}

fn cleanup_direct_staging(prepared: &[PreparedDirectWrite<'_>]) -> Result<()> {
    let mut errors = Vec::new();
    for write in prepared {
        let Some(paths) = &write.paths else {
            continue;
        };
        let cleanup = if write.before.is_some() {
            &paths.displaced
        } else {
            &paths.staged
        };
        if let Err(error) = maybe_fail_direct_cleanup(&write.path) {
            errors.push(format!("{}: {error:#}", cleanup.display()));
            continue;
        }
        if let Err(error) = maybe_replace_cleanup_artifact(&write.path, cleanup) {
            errors.push(format!("{}: {error:#}", cleanup.display()));
            continue;
        }
        let (identity, expected_contents, description) = if let Some(before) = &write.before {
            let Some(displaced) = write.displaced.as_ref() else {
                errors.push(format!(
                    "{}: committed backup has no retained displaced-file identity",
                    cleanup.display()
                ));
                continue;
            };
            (
                displaced,
                before.contents.as_slice(),
                "direct-write displaced backup",
            )
        } else {
            let Some(installed) = write.installed.as_ref() else {
                errors.push(format!(
                    "{}: create staging has no retained staged-file identity",
                    cleanup.display()
                ));
                continue;
            };
            (installed, write.new_contents, "direct-write create staging")
        };
        if let Err(error) =
            remove_owned_regular_artifact(cleanup, identity, Some(expected_contents), description)
        {
            errors.push(format!("{}: {error:#}", cleanup.display()));
        }
        if paths.displaced != paths.staged {
            let unused = if write.before.is_some() {
                &paths.staged
            } else {
                &paths.displaced
            };
            if let Err(error) = ensure_artifact_path_absent(unused, "unused direct cleanup path") {
                errors.push(format!("{}: {error:#}", unused.display()));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!(
            "removing direct-write backups failed: {}",
            errors.join("; ")
        )
    }
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
    home: &AnchoredHome,
    edits: &[FileEdit],
    decisions: Vec<Decision>,
    expected_before: &BTreeMap<String, Option<Vec<u8>>>,
    mode: ApprovedApplyMode,
) -> Result<Vec<AppliedChange>> {
    validate_approved_edit_budget(edits, expected_before)
        .map_err(|error| approved_apply_error(error, ApprovedApplyFailure::NoWrite))?;
    let mut prepared = Vec::with_capacity(edits.len());
    let mut preparation_error = None;
    for (edit, decision) in edits.iter().zip(decisions) {
        let result = (|| -> Result<PreparedConditionalWrite<'_>> {
            let expected = expected_before
                .get(&edit.target)
                .with_context(|| format!("missing approved before-image for `{}`", edit.target))?
                .clone();
            let resolved = join_resolved(&home.absolute, &decision.resolved);
            let parent = resolved
                .parent()
                .ok_or_else(|| anyhow!("target has no parent directory: {}", resolved.display()))?;
            let name = resolved
                .file_name()
                .ok_or_else(|| anyhow!("target has no file name: {}", resolved.display()))?;
            let canonical_parent = prepare_staging_parent(home, parent)?;
            let path = canonical_parent.entry(name);
            maybe_swap_prepared_parent(&path)?;
            let current = open_regular_file_without_following_links(&path)?;
            let current_contents = current.as_ref().map(|current| current.contents.as_slice());
            let already_applied = mode == ApprovedApplyMode::Recovery
                && current_contents == Some(edit.new_contents.as_slice());
            match (&expected, current_contents) {
                (Some(expected), Some(current)) if current == expected || already_applied => {}
                (Some(_), _) => {
                    bail!(
                        "approved target `{}` changed after review; refusing to overwrite it",
                        edit.target
                    );
                }
                (None, None) => {}
                (None, Some(_)) if already_applied => {}
                (None, Some(_)) => {
                    bail!(
                        "approved target `{}` appeared after review; refusing to overwrite it",
                        edit.target
                    );
                }
            }
            let audit_before_sha256 = if already_applied {
                expected.as_deref().map(sha256_hex)
            } else {
                None
            };
            Ok(PreparedConditionalWrite {
                path,
                paths: None,
                already_applied,
                audit_before_sha256,
                expected_before: expected,
                observed: current,
                installed: None,
                displaced: None,
                new_contents: edit.new_contents.as_slice(),
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
        match stage_contents(&prepared[index].path, prepared[index].new_contents) {
            Ok((paths, installed)) => {
                prepared[index].paths = Some(paths);
                prepared[index].installed = Some(installed);
            }
            Err(error) => return fail_after_conditional_rollback(&prepared, &[], error),
        }
    }

    let mut swapped = Vec::new();
    for index in 0..prepared.len() {
        if prepared[index].already_applied {
            continue;
        }
        let commit = {
            let write = &prepared[index];
            let paths = write
                .paths
                .as_ref()
                .context("prepared approved write has no staging paths")?;
            if let Err(error) = maybe_run_conditional_write_hook(&write.path) {
                return fail_after_conditional_rollback(&prepared, &swapped, error);
            }
            if write.expected_before.is_some() {
                replace_preserving_target(&write.path, &paths.staged, &paths.displaced)
            } else {
                hard_link_without_replace(&paths.staged, &write.path).with_context(|| {
                    format!(
                        "approved target `{}` appeared after review; refusing to overwrite it",
                        write.decision.target
                    )
                })
            }
        };
        if let Err(error) = commit {
            let write = &prepared[index];
            let paths = write
                .paths
                .as_ref()
                .context("prepared approved write has no staging paths")?;
            let installed = write
                .installed
                .as_ref()
                .context("prepared approved write has no staged-file identity")?;
            let staged_at_target = match open_file_matches_path_if_present(installed, &write.path) {
                Ok(matches) => matches,
                Err(identity_error) => {
                    return fail_after_conditional_rollback(
                        &prepared,
                        &swapped,
                        anyhow!(
                            "{error:#}; checking failed approved commit ownership also failed: \
                             {identity_error:#}"
                        ),
                    );
                }
            };
            if staged_at_target
                || (write.expected_before.is_some()
                    && failed_replace_displaced_target(&paths.displaced))
            {
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
            let write = &mut prepared[index];
            let paths = write
                .paths
                .as_ref()
                .context("prepared approved write has no staging paths")?;
            let installed = write
                .installed
                .as_ref()
                .context("prepared approved write has no staged-file identity")?;
            let changed = format!(
                "approved target `{}` changed during commit",
                write.decision.target
            );
            verify_installed_regular_target(
                &write.path,
                write.new_contents,
                installed,
                "committed approved target unexpectedly disappeared",
                &changed,
            )?;
            if write.expected_before.is_some() {
                let observed = write
                    .observed
                    .as_mut()
                    .context("existing approved target lost its retained identity")?;
                let expected_before = write
                    .expected_before
                    .as_deref()
                    .context("existing approved target lost its expected before-image")?;
                let displaced_sha256 = verify_displaced_regular_target(
                    &mut observed.file,
                    expected_before,
                    &paths.displaced,
                    &mut write.displaced,
                    &changed,
                )?;
                write.audit_before_sha256 = Some(displaced_sha256);
            }
            Ok(())
        })();
        if let Err(error) = verification {
            return fail_after_conditional_rollback(&prepared, &swapped, error);
        }
    }

    let final_verification = (|| -> Result<()> {
        for write in &mut prepared {
            let identity = if write.already_applied {
                &write
                    .observed
                    .as_ref()
                    .context("already-applied approved target lost its retained identity")?
                    .file
            } else {
                write
                    .installed
                    .as_ref()
                    .context("committed approved target lost its staged-file identity")?
            };
            let changed = format!(
                "approved target `{}` changed before batch finalization",
                write.decision.target
            );
            verify_installed_regular_target(
                &write.path,
                write.new_contents,
                identity,
                "approved target unexpectedly disappeared before batch finalization",
                &changed,
            )?;
            if write.already_applied {
                continue;
            }
            let Some(expected_before) = write.expected_before.as_deref() else {
                continue;
            };
            let paths = write
                .paths
                .as_ref()
                .context("committed approved write has no staging paths")?;
            let observed = write
                .observed
                .as_mut()
                .context("existing approved target lost its retained identity")?;
            verify_displaced_regular_target(
                &mut observed.file,
                expected_before,
                &paths.displaced,
                &mut write.displaced,
                &changed,
            )?;
        }
        Ok(())
    })();
    if let Err(error) = final_verification {
        return fail_after_conditional_rollback(&prepared, &swapped, error);
    }

    let changes = approved_apply_changes(&prepared);
    for write in &prepared {
        if let Some(paths) = &write.paths {
            let (cleanup, identity, expected_contents, description) =
                if let Some(expected_before) = write.expected_before.as_deref() {
                    let Some(displaced) = write.displaced.as_ref() else {
                        return Err(approved_apply_error(
                            anyhow!("committed approved backup has no retained identity"),
                            ApprovedApplyFailure::Applied(ApplyReport {
                                changes: changes.clone(),
                            }),
                        ));
                    };
                    (
                        &paths.displaced,
                        displaced,
                        expected_before,
                        "approved-write displaced backup",
                    )
                } else {
                    let Some(installed) = write.installed.as_ref() else {
                        return Err(approved_apply_error(
                            anyhow!("approved create staging has no retained identity"),
                            ApprovedApplyFailure::Applied(ApplyReport {
                                changes: changes.clone(),
                            }),
                        ));
                    };
                    (
                        &paths.staged,
                        installed,
                        write.new_contents,
                        "approved-write create staging",
                    )
                };
            if let Err(error) = maybe_replace_cleanup_artifact(&write.path, cleanup) {
                return Err(approved_apply_error(
                    error,
                    ApprovedApplyFailure::Applied(ApplyReport {
                        changes: changes.clone(),
                    }),
                ));
            }
            if let Err(error) = remove_owned_regular_artifact(
                cleanup,
                identity,
                Some(expected_contents),
                description,
            ) {
                return Err(approved_apply_error(
                    error,
                    ApprovedApplyFailure::Applied(ApplyReport {
                        changes: changes.clone(),
                    }),
                ));
            }
            if paths.displaced != paths.staged {
                let unused = if write.expected_before.is_some() {
                    &paths.staged
                } else {
                    &paths.displaced
                };
                if let Err(error) =
                    ensure_artifact_path_absent(unused, "unused approved cleanup path")
                {
                    return Err(approved_apply_error(
                        error,
                        ApprovedApplyFailure::Applied(ApplyReport {
                            changes: changes.clone(),
                        }),
                    ));
                }
            }
        }
    }

    Ok(changes)
}

fn approved_apply_changes(prepared: &[PreparedConditionalWrite<'_>]) -> Vec<AppliedChange> {
    prepared
        .iter()
        .map(|write| {
            let audit = (write.decision.tier == Tier::Logged).then(|| AuditRecord {
                target: write.decision.target.clone(),
                resolved: write.decision.resolved.clone(),
                tier: write.decision.tier,
                prev_sha256: write.audit_before_sha256.clone(),
                next_sha256: sha256_hex(write.new_contents),
                bytes_written: write.new_contents.len(),
            });
            AppliedChange {
                decision: write.decision.clone(),
                disposition: Disposition::Applied,
                audit,
            }
        })
        .collect()
}

fn rollback_conditional_writes(
    prepared: &[PreparedConditionalWrite<'_>],
    swapped: &[usize],
) -> Result<()> {
    let mut errors = Vec::new();
    for &index in swapped.iter().rev() {
        let write = &prepared[index];
        let paths = write
            .paths
            .as_ref()
            .context("swapped conditional write has no approved-write paths")?;
        let rollback = if write.expected_before.is_some() {
            restore_swapped_write(write, paths)
        } else {
            rollback_created_write(write)
        };
        if let Err(error) = rollback {
            errors.push(format!("{}: {error:#}", write.decision.target));
        }
    }
    for write in prepared.iter().rev().filter(|write| write.already_applied) {
        if write.expected_before.as_deref() != Some(write.new_contents) {
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
    prepared: &[PreparedConditionalWrite<'_>],
    swapped: &[usize],
    error: anyhow::Error,
) -> Result<Vec<AppliedChange>> {
    let rollback = rollback_conditional_writes(prepared, swapped);
    match rollback {
        Ok(()) => match cleanup_conditional_staging(prepared) {
            Ok(()) => Err(approved_apply_error(
                error.context("approved proposal was rolled back"),
                ApprovedApplyFailure::RolledBack,
            )),
            Err(cleanup_error) => Err(approved_apply_error(
                anyhow!(
                    "{error:#}; approved proposal was rolled back but cleanup failed: \
                     {cleanup_error:#}"
                ),
                ApprovedApplyFailure::RolledBackCleanupFailed {
                    targets: prepared
                        .iter()
                        .filter(|write| write.paths.is_some())
                        .map(|write| write.decision.target.clone())
                        .collect(),
                },
            )),
        },
        Err(rollback_error) => {
            let mut targets = swapped
                .iter()
                .map(|&index| prepared[index].decision.target.clone())
                .collect::<Vec<_>>();
            targets.extend(
                prepared
                    .iter()
                    .filter(|write| write.already_applied)
                    .map(|write| write.decision.target.clone()),
            );
            targets.sort();
            targets.dedup();
            Err(approved_apply_error(
                anyhow!("{error:#}; conditional rollback also failed: {rollback_error:#}"),
                ApprovedApplyFailure::Ambiguous { targets },
            ))
        }
    }
}

fn restore_swapped_write(
    write: &PreparedConditionalWrite<'_>,
    paths: &ApprovedWritePaths,
) -> Result<()> {
    let installed = write
        .installed
        .as_ref()
        .context("swapped approved write has no staged-file identity")?;
    rollback_replaced_regular_target(
        &write.path,
        paths,
        write.new_contents,
        installed,
        write.displaced.as_ref(),
    )
}

fn rollback_created_write(write: &PreparedConditionalWrite<'_>) -> Result<()> {
    let installed = write
        .installed
        .as_ref()
        .context("swapped approved create has no retained staged-file identity")?;
    let captured = rollback_capture_path(&write.path);
    atomic_move_without_replace(&write.path, &captured).with_context(|| {
        format!(
            "capturing created target {} before rollback",
            write.path.display()
        )
    })?;
    let still_owned = (|| -> Result<bool> {
        let mut captured_file = open_regular_file_handle_without_following_links(&captured)?
            .context("approved-create rollback capture is not a regular file")?;
        Ok(
            stream_regular_file_matches_and_sha256(&mut captured_file, write.new_contents)?
                .is_some()
                && open_file_matches_path(installed, &captured)?,
        )
    })();
    match still_owned {
        Ok(true) => remove_owned_regular_artifact(
            &captured,
            installed,
            Some(write.new_contents),
            "approved-create rollback capture",
        ),
        Ok(false) => {
            restore_unowned_rollback_capture(write, &captured)?;
            bail!(
                "created target changed before rollback; restored concurrent bytes and left \
                 approved-write staging in place"
            )
        }
        Err(identity_error) => {
            restore_unowned_rollback_capture(write, &captured)?;
            Err(identity_error.context(
                "created-target ownership could not be verified; restored captured bytes and \
                 left approved-write staging in place",
            ))
        }
    }
}

fn restore_unowned_rollback_capture(
    write: &PreparedConditionalWrite<'_>,
    captured: &AnchoredEntry,
) -> Result<()> {
    atomic_move_without_replace(captured, &write.path).with_context(|| {
        format!(
            "created target changed during rollback; concurrent bytes remain preserved at {}",
            captured.display()
        )
    })
}

fn rollback_capture_path(target: &AnchoredEntry) -> AnchoredEntry {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    target.sibling(format!(".{name}.ward-rollback-{}", uuid::Uuid::new_v4()).into())
}

fn cleanup_capture_path(artifact: &AnchoredEntry) -> AnchoredEntry {
    let name = artifact
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    artifact.sibling(format!(".{name}.ward-cleanup-{}", uuid::Uuid::new_v4()).into())
}

fn error_has_io_kind(error: &anyhow::Error, kind: ErrorKind) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == kind)
    })
}

fn restore_unowned_cleanup_capture(
    artifact: &AnchoredEntry,
    captured: &AnchoredEntry,
    verification_error: anyhow::Error,
) -> anyhow::Error {
    match atomic_move_without_replace(captured, artifact) {
        Ok(()) => verification_error.context(format!(
            "cleanup preserved the unowned entry at {}",
            artifact.display()
        )),
        Err(restore_error) => anyhow!(
            "{verification_error:#}; cleanup preserved the unowned entry at {} because restoring \
             it to {} failed: {restore_error:#}",
            captured.display(),
            artifact.display()
        ),
    }
}

fn remove_owned_regular_artifact(
    artifact: &AnchoredEntry,
    retained_identity: &std::fs::File,
    expected_contents: Option<&[u8]>,
    description: &str,
) -> Result<()> {
    let retained_metadata = retained_identity
        .metadata()
        .with_context(|| format!("reading retained {description} identity"))?;
    if !retained_metadata.is_file() {
        bail!("retained {description} identity is not a regular file");
    }

    let captured = cleanup_capture_path(artifact);
    if let Err(error) = atomic_move_without_replace(artifact, &captured) {
        if error_has_io_kind(&error, ErrorKind::NotFound) {
            return Ok(());
        }
        return Err(error).with_context(|| {
            format!(
                "capturing {description} {} before cleanup",
                artifact.display()
            )
        });
    }

    if let Err(error) =
        verify_owned_regular_artifact(&captured, retained_identity, expected_contents, description)
    {
        return Err(restore_unowned_cleanup_capture(artifact, &captured, error));
    }

    maybe_replace_verified_cleanup_capture(&captured)?;
    if let Err(error) =
        verify_owned_regular_artifact(&captured, retained_identity, expected_contents, description)
    {
        return Err(restore_unowned_cleanup_capture(artifact, &captured, error));
    }
    if let Err(error) = remove_anchored_file(&captured)
        .with_context(|| format!("removing captured {description} {}", captured.display()))
    {
        return Err(restore_unowned_cleanup_capture(artifact, &captured, error));
    }
    Ok(())
}

fn verify_owned_regular_artifact(
    artifact: &AnchoredEntry,
    retained_identity: &std::fs::File,
    expected_contents: Option<&[u8]>,
    description: &str,
) -> Result<()> {
    let mut current = open_regular_file_handle_without_following_links(artifact)?
        .with_context(|| format!("{description} is not a regular file"))?;
    if !open_file_matches_path(retained_identity, artifact)? {
        bail!("{description} identity changed before cleanup");
    }
    if let Some(expected) = expected_contents {
        if stream_regular_file_matches_and_sha256(&mut current, expected)?.is_none() {
            bail!("{description} bytes changed before cleanup");
        }
    }
    Ok(())
}

fn ensure_artifact_path_absent(path: &AnchoredEntry, description: &str) -> Result<()> {
    if !anchored_entry_exists(path)
        .with_context(|| format!("inspecting {description} {}", path.display()))?
    {
        Ok(())
    } else {
        bail!(
            "{description} {} contains an entry whose ownership is unproven; preserving it",
            path.display()
        )
    }
}

fn anchored_entry_exists(path: &AnchoredEntry) -> Result<bool> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;

        match rustix::fs::statat(
            path.parent.as_fd(),
            &path.name,
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
    #[cfg(not(unix))]
    {
        match path.parent.symlink_metadata(&path.name) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

fn remove_anchored_file(path: &AnchoredEntry) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;

        rustix::fs::unlinkat(
            path.parent.as_fd(),
            &path.name,
            rustix::fs::AtFlags::empty(),
        )
        .map_err(Into::into)
    }
    #[cfg(not(unix))]
    {
        path.parent.remove_file(&path.name).map_err(Into::into)
    }
}

#[derive(Clone, Copy)]
enum RegularFileReadPolicy<'a> {
    WardFile,
    DirectBeforeImage {
        target: &'a str,
        retained_bytes: u64,
    },
}

impl RegularFileReadPolicy<'_> {
    fn max_read_bytes(self) -> u64 {
        match self {
            Self::WardFile => WARD_FILE_CONTENT_MAX_BYTES,
            Self::DirectBeforeImage { retained_bytes, .. } => WARD_FILE_CONTENT_MAX_BYTES
                .min(WARD_RETAINED_CONTENT_MAX_BYTES.saturating_sub(retained_bytes)),
        }
    }

    fn ensure_metadata_length(self, path: &Path, observed_bytes: u64) -> Result<()> {
        if observed_bytes > WARD_FILE_CONTENT_MAX_BYTES {
            return Err(self.limit_error(path, observed_bytes));
        }
        if let Self::DirectBeforeImage { retained_bytes, .. } = self {
            reserve_ward_content_bytes(retained_bytes, observed_bytes)?;
        }
        Ok(())
    }

    fn limit_error(self, path: &Path, observed_bytes: u64) -> anyhow::Error {
        match self {
            Self::WardFile => anyhow!(
                "Ward regular file {} is {observed_bytes} bytes, over the \
                 {WARD_FILE_CONTENT_MAX_BYTES}-byte content limit",
                path.display()
            ),
            Self::DirectBeforeImage {
                target,
                retained_bytes: _,
            } if observed_bytes > WARD_FILE_CONTENT_MAX_BYTES => {
                WardEditBudgetFailure::ExistingBeforeImage {
                    target: target.to_owned(),
                    observed_bytes,
                    max_bytes: WARD_FILE_CONTENT_MAX_BYTES,
                }
                .into()
            }
            Self::DirectBeforeImage { retained_bytes, .. } => {
                let attempted_bytes = retained_bytes.saturating_add(observed_bytes);
                WardEditBudgetFailure::BatchRetainedMemory {
                    attempted_bytes,
                    max_bytes: WARD_RETAINED_CONTENT_MAX_BYTES,
                }
                .into()
            }
        }
    }
}

fn read_open_file_with_policy(
    file: &mut std::fs::File,
    path: &Path,
    metadata_len: u64,
    policy: RegularFileReadPolicy<'_>,
) -> Result<Vec<u8>> {
    policy.ensure_metadata_length(path, metadata_len)?;
    if matches!(policy, RegularFileReadPolicy::DirectBeforeImage { .. }) {
        maybe_grow_direct_read_target(path)?;
    }
    file.seek(SeekFrom::Start(0))
        .context("seeking regular file")?;
    let max_read_bytes = policy.max_read_bytes();
    let capacity = usize::try_from(metadata_len.min(max_read_bytes))
        .context("regular-file read capacity is not representable")?;
    let mut contents = Vec::with_capacity(capacity);
    let max_read_bytes =
        usize::try_from(max_read_bytes).context("regular-file read limit is not representable")?;
    let mut scratch = [0_u8; DIRECT_VERIFICATION_SCRATCH_BYTES];
    while contents.len() < max_read_bytes {
        let remaining = max_read_bytes - contents.len();
        let read = file
            .read(&mut scratch[..remaining.min(DIRECT_VERIFICATION_SCRATCH_BYTES)])
            .context("reading regular file")?;
        if read == 0 {
            return Ok(contents);
        }
        contents.extend_from_slice(&scratch[..read]);
    }
    if file
        .read(&mut scratch[..1])
        .context("checking regular file read limit")?
        != 0
    {
        let observed_bytes = u64::try_from(max_read_bytes)
            .context("regular-file read limit is not representable")?
            .saturating_add(1);
        return Err(policy.limit_error(path, observed_bytes));
    }
    Ok(contents)
}

fn open_regular_file_without_following_links(
    path: &AnchoredEntry,
) -> Result<Option<OpenRegularFile>> {
    open_regular_file_without_following_links_with_policy(path, RegularFileReadPolicy::WardFile)
}

fn open_direct_before_image(
    path: &AnchoredEntry,
    target: &str,
    retained_bytes: u64,
) -> Result<Option<OpenRegularFile>> {
    open_regular_file_without_following_links_with_policy(
        path,
        RegularFileReadPolicy::DirectBeforeImage {
            target,
            retained_bytes,
        },
    )
}

fn open_regular_file_without_following_links_with_policy(
    path: &AnchoredEntry,
    policy: RegularFileReadPolicy<'_>,
) -> Result<Option<OpenRegularFile>> {
    let Some(mut file) = open_regular_file_handle_without_following_links(path)? else {
        return Ok(None);
    };
    let metadata_len = file
        .metadata()
        .with_context(|| format!("reading direct target metadata {}", path.display()))?
        .len();
    let contents = read_open_file_with_policy(&mut file, path, metadata_len, policy)
        .with_context(|| format!("reading direct target {}", path.display()))?;
    Ok(Some(OpenRegularFile { file, contents }))
}

fn reject_known_non_regular_entry(path: &AnchoredEntry) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;

        match rustix::fs::statat(
            path.parent.as_fd(),
            &path.name,
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(metadata)
                if rustix::fs::FileType::from_raw_mode(metadata.st_mode)
                    == rustix::fs::FileType::RegularFile =>
            {
                Ok(())
            }
            Ok(_) => bail!("direct target {} is not a regular file", path.display()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(std::io::Error::from(error))
                .with_context(|| format!("reading direct target metadata {}", path.display())),
        }
    }
    #[cfg(not(unix))]
    {
        match path.parent.symlink_metadata(&path.name) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
            Ok(_) => bail!("direct target {} is not a regular file", path.display()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error)
                .with_context(|| format!("reading direct target metadata {}", path.display())),
        }
    }
}

#[cfg(target_os = "linux")]
fn open_regular_file_handle_without_following_links(
    path: &AnchoredEntry,
) -> Result<Option<std::fs::File>> {
    use std::os::fd::AsFd;

    use rustix::fs::{openat2, Mode, OFlags, ResolveFlags};

    reject_known_non_regular_entry(path)?;
    let file = match openat2(
        path.parent.as_fd(),
        &path.name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_MAGICLINKS | ResolveFlags::NO_SYMLINKS,
    ) {
        Ok(file) => std::fs::File::from(file),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) if error.raw_os_error() == libc::ENOSYS => {
            return open_regular_file_handle_portable(path);
        }
        Err(error) => {
            return Err(std::io::Error::from(error))
                .with_context(|| format!("opening direct target {}", path.display()))
        }
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("reading direct target metadata {}", path.display()))?;
    if !metadata.is_file() {
        bail!("direct target {} is not a regular file", path.display());
    }
    Ok(Some(file))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn open_regular_file_handle_without_following_links(
    path: &AnchoredEntry,
) -> Result<Option<std::fs::File>> {
    open_regular_file_handle_portable(path)
}

#[cfg(unix)]
fn open_regular_file_handle_portable(path: &AnchoredEntry) -> Result<Option<std::fs::File>> {
    use std::os::fd::AsFd;

    reject_known_non_regular_entry(path)?;
    let file = match rustix::fs::openat(
        path.parent.as_fd(),
        &path.name,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::CLOEXEC
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
    ) {
        Ok(file) => std::fs::File::from(file),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("opening direct target {}", path.display()))
        }
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("reading direct target metadata {}", path.display()))?;
    if !metadata.is_file() {
        bail!("direct target {} is not a regular file", path.display());
    }
    Ok(Some(file))
}

#[cfg(windows)]
fn open_regular_file_handle_without_following_links(
    path: &AnchoredEntry,
) -> Result<Option<std::fs::File>> {
    use cap_std::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    reject_known_non_regular_entry(path)?;
    let mut options = CapOpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = match path.parent.open_with(&path.name, &options) {
        Ok(file) => file.into_std(),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("opening direct target {}", path.display()))
        }
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("reading direct target metadata {}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("direct target {} is not a regular file", path.display());
    }
    Ok(Some(file))
}

#[cfg(not(any(unix, windows)))]
fn open_regular_file_handle_without_following_links(
    path: &AnchoredEntry,
) -> Result<Option<std::fs::File>> {
    reject_known_non_regular_entry(path)?;
    match path.parent.open(&path.name) {
        Ok(file) => Ok(Some(file.into_std())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("opening direct target {}", path.display()))
        }
    }
}

fn stream_regular_file_matches_and_sha256(
    file: &mut std::fs::File,
    expected_contents: &[u8],
) -> Result<Option<String>> {
    use sha2::{Digest, Sha256};

    file.seek(SeekFrom::Start(0))
        .context("seeking regular file for verification")?;
    let mut hasher = Sha256::new();
    let mut offset = 0_usize;
    let mut scratch = [0_u8; DIRECT_VERIFICATION_SCRATCH_BYTES];
    loop {
        let read = file
            .read(&mut scratch)
            .context("streaming regular file for verification")?;
        if read == 0 {
            break;
        }
        let Some(end) = offset.checked_add(read) else {
            return Ok(None);
        };
        if expected_contents.get(offset..end) != Some(&scratch[..read]) {
            return Ok(None);
        }
        hasher.update(&scratch[..read]);
        offset = end;
    }
    if offset != expected_contents.len() {
        return Ok(None);
    }
    Ok(Some(
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    ))
}

fn verify_installed_regular_target(
    path: &AnchoredEntry,
    expected_contents: &[u8],
    installed_identity: &std::fs::File,
    missing_message: &str,
    changed_message: &str,
) -> Result<()> {
    let mut installed = open_regular_file_handle_without_following_links(path)?
        .with_context(|| missing_message.to_owned())?;
    if stream_regular_file_matches_and_sha256(&mut installed, expected_contents)?.is_none()
        || !open_file_matches_path(installed_identity, path)?
    {
        bail!("{changed_message}");
    }
    Ok(())
}

fn verify_displaced_regular_target(
    observed: &mut std::fs::File,
    expected_contents: &[u8],
    displaced_path: &AnchoredEntry,
    displaced_identity: &mut Option<std::fs::File>,
    changed_message: &str,
) -> Result<String> {
    let mut displaced = open_regular_file_handle_without_following_links(displaced_path)?
        .context("commit-displaced target is not a regular file")?;
    let displaced_sha256 =
        stream_regular_file_matches_and_sha256(&mut displaced, expected_contents)?;
    *displaced_identity = Some(displaced);
    if !open_file_matches_path(observed, displaced_path)? {
        bail!("{changed_message}");
    }
    let observed_sha256 = stream_regular_file_matches_and_sha256(observed, expected_contents)
        .with_context(|| format!("reading displaced target {}", displaced_path.display()))?;
    if displaced_sha256.is_none() || observed_sha256.is_none() {
        bail!("{changed_message}");
    }
    Ok(displaced_sha256.expect("verified displaced digest is present"))
}

fn rollback_replaced_regular_target(
    target: &AnchoredEntry,
    paths: &ApprovedWritePaths,
    installed_contents: &[u8],
    installed_identity: &std::fs::File,
    displaced_identity: Option<&std::fs::File>,
) -> Result<()> {
    if !open_file_matches_path(installed_identity, target)? {
        bail!("target changed before rollback {}", target.display());
    }
    let displaced_before_rollback = match displaced_identity {
        Some(displaced) => open_file_matches_path(displaced, &paths.displaced)?,
        None => false,
    };
    restore_displaced_target(target, &paths.staged, &paths.displaced)
        .with_context(|| format!("restoring target {}", target.display()))?;
    if open_file_matches_path(installed_identity, target)?
        || !open_file_matches_path(installed_identity, &paths.staged)?
    {
        bail!(
            "installed file identity changed while rolling back {}",
            target.display()
        );
    }
    let displaced_restored = match displaced_identity {
        Some(displaced) => open_file_matches_path(displaced, target)?,
        None => false,
    };
    let mut displaced_installed = open_regular_file_handle_without_following_links(&paths.staged)?
        .context("rollback-displaced installed target unexpectedly disappeared")?;
    if stream_regular_file_matches_and_sha256(&mut displaced_installed, installed_contents)?
        .is_none()
    {
        if displaced_restored {
            replace_preserving_target(target, &paths.staged, &paths.displaced).with_context(
                || {
                    format!(
                        "putting concurrently changed installed inode back at {}",
                        target.display()
                    )
                },
            )?;
        }
        bail!("installed bytes changed during rollback");
    }
    if !displaced_before_rollback || !displaced_restored {
        bail!(
            "commit-displaced file identity changed during rollback {}",
            target.display()
        );
    }
    Ok(())
}

fn open_file_matches_path_if_present(file: &std::fs::File, path: &AnchoredEntry) -> Result<bool> {
    open_file_matches_path(file, path)
}

#[cfg(unix)]
fn open_file_matches_path(file: &std::fs::File, path: &AnchoredEntry) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let Some(current) = open_regular_file_handle_without_following_links(path)? else {
        return Ok(false);
    };
    let open = file.metadata().context("reading open file identity")?;
    let path = current
        .metadata()
        .with_context(|| format!("reading file identity {}", path.display()))?;
    Ok(open.dev() == path.dev() && open.ino() == path.ino())
}

#[cfg(windows)]
fn open_file_matches_path(file: &std::fs::File, path: &AnchoredEntry) -> Result<bool> {
    let Some(current) = open_regular_file_handle_without_following_links(path)? else {
        return Ok(false);
    };
    Ok(windows_file_identity_from_open_file(file)?
        == windows_file_identity_from_open_file(&current)?)
}

#[cfg(not(any(unix, windows)))]
fn open_file_matches_path(_file: &std::fs::File, _path: &AnchoredEntry) -> Result<bool> {
    bail!("open-file identity comparison is unsupported on this platform")
}

#[cfg(all(test, unix))]
fn same_file_identity(left: &Path, right: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let left = std::fs::symlink_metadata(left)
        .with_context(|| format!("reading file identity {}", left.display()))?;
    let right = std::fs::symlink_metadata(right)
        .with_context(|| format!("reading file identity {}", right.display()))?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(all(test, windows))]
fn same_file_identity(left: &Path, right: &Path) -> Result<bool> {
    Ok(windows_file_identity(left)? == windows_file_identity(right)?)
}

#[cfg(all(test, windows))]
fn windows_file_identity(path: &Path) -> Result<(u64, [u8; 16])> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .with_context(|| format!("opening file identity {}", path.display()))?;
    windows_file_identity_from_open_file(&file)
        .with_context(|| format!("reading file identity {}", path.display()))
}

#[cfg(windows)]
fn windows_file_identity_from_open_file(file: &std::fs::File) -> Result<(u64, [u8; 16])> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
    };

    let mut info = FILE_ID_INFO::default();
    // SAFETY: `file` owns a valid handle and `info` is a correctly sized,
    // writable FILE_ID_INFO buffer that remains alive for the call.
    let result = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as _,
            FileIdInfo,
            std::ptr::addr_of_mut!(info).cast(),
            u32::try_from(std::mem::size_of::<FILE_ID_INFO>())
                .expect("FILE_ID_INFO size fits in u32"),
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error()).context("reading file identity");
    }
    validate_windows_file_identity(info.VolumeSerialNumber, info.FileId.Identifier)
}

#[cfg(any(windows, test))]
fn validate_windows_file_identity(
    volume_serial_number: u64,
    file_id: [u8; 16],
) -> Result<(u64, [u8; 16])> {
    if file_id == [0; 16] || file_id == [u8::MAX; 16] {
        bail!("Windows returned an unusable 128-bit file ID");
    }
    Ok((volume_serial_number, file_id))
}

fn hard_link_without_replace(source: &AnchoredEntry, destination: &AnchoredEntry) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;

        rustix::fs::linkat(
            source.parent.as_fd(),
            &source.name,
            destination.parent.as_fd(),
            &destination.name,
            rustix::fs::AtFlags::empty(),
        )
        .map_err(std::io::Error::from)
        .with_context(|| {
            format!(
                "linking {} to {} without replacement",
                source.display(),
                destination.display()
            )
        })?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        source
            .parent
            .hard_link(&source.name, destination.parent.as_ref(), &destination.name)
            .with_context(|| {
                format!(
                    "linking {} to {} without replacement",
                    source.display(),
                    destination.display()
                )
            })
    }
}

#[cfg(all(test, not(any(unix, windows))))]
fn same_file_identity(_left: &Path, _right: &Path) -> Result<bool> {
    bail!("file-identity comparison is unsupported on this platform")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn atomic_move_without_replace(source: &AnchoredEntry, destination: &AnchoredEntry) -> Result<()> {
    use std::os::fd::AsFd;

    rustix::fs::renameat_with(
        source.parent.as_fd(),
        &source.name,
        destination.parent.as_fd(),
        &destination.name,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)
    .context("atomically moving file without replacement")
}

#[cfg(windows)]
fn atomic_move_without_replace(source: &AnchoredEntry, destination: &AnchoredEntry) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;

    use cap_std::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FileRenameInfo, SetFileInformationByHandle, FILE_FLAG_OPEN_REPARSE_POINT, FILE_RENAME_INFO,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    const DELETE_ACCESS: u32 = 0x0001_0000;

    let mut options = CapOpenOptions::new();
    options
        .access_mode(DELETE_ACCESS)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .follow(FollowSymlinks::No);
    let source_file = source
        .parent
        .open_with(&source.name, &options)
        .with_context(|| format!("opening move source {}", source.display()))?;
    let metadata = source_file
        .metadata()
        .with_context(|| format!("reading move source {}", source.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("move source {} is not a regular file", source.display());
    }

    // The Win32 API's absolute-name form is more widely supported than a
    // RootDirectory-relative FILE_RENAME_INFO. Retained non-share-delete
    // handles keep every destination ancestor stable for this call.
    let destination_name = destination
        .absolute
        .as_os_str()
        .encode_wide()
        .collect::<Vec<_>>();
    let file_name_offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let info_size = file_name_offset
        .checked_add((destination_name.len() + 1) * std::mem::size_of::<u16>())
        .context("Windows rename buffer size overflow")?;
    let mut buffer = vec![0_u64; info_size.div_ceil(std::mem::size_of::<u64>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: `buffer` is aligned and large enough for FILE_RENAME_INFO plus
    // the UTF-16 destination name, and all handles remain live for the call.
    let result = unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength =
            u32::try_from(destination_name.len() * 2).context("Windows rename name is too long")?;
        std::ptr::copy_nonoverlapping(
            destination_name.as_ptr(),
            std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            destination_name.len(),
        );
        SetFileInformationByHandle(
            source_file.as_raw_handle() as _,
            FileRenameInfo,
            info.cast(),
            u32::try_from(info_size).context("Windows rename buffer is too large")?,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error()).context("atomically moving file without replacement")
    } else {
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn atomic_move_without_replace(
    _source: &AnchoredEntry,
    _destination: &AnchoredEntry,
) -> Result<()> {
    bail!("atomic no-replace move is unsupported on this platform")
}

fn cleanup_conditional_staging(prepared: &[PreparedConditionalWrite<'_>]) -> Result<()> {
    let mut errors = Vec::new();
    for write in prepared {
        if let Some(paths) = &write.paths {
            let Some(installed) = write.installed.as_ref() else {
                errors.push(format!(
                    "{}: rollback staging has no retained identity",
                    paths.staged.display()
                ));
                continue;
            };
            if let Err(error) = maybe_replace_cleanup_artifact(&write.path, &paths.staged) {
                errors.push(format!("{}: {error:#}", paths.staged.display()));
                continue;
            }
            if let Err(error) = remove_owned_regular_artifact(
                &paths.staged,
                installed,
                Some(write.new_contents),
                "approved rollback staging",
            ) {
                errors.push(format!("{}: {error:#}", paths.staged.display()));
            }
            if paths.displaced != paths.staged {
                if let Err(error) = ensure_artifact_path_absent(
                    &paths.displaced,
                    "approved rollback displaced-backup path",
                ) {
                    errors.push(format!("{}: {error:#}", paths.displaced.display()));
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!(
            "removing approved rollback artifacts failed: {}",
            errors.join("; ")
        )
    }
}

pub(crate) const fn supports_atomic_approved_writes() -> bool {
    cfg!(any(target_os = "linux", target_os = "macos", windows))
}

fn stage_direct_write(write: &mut PreparedDirectWrite<'_>) -> Result<()> {
    let (paths, file) = stage_contents(&write.path, write.new_contents)?;
    write.paths = Some(paths);
    write.installed = Some(file);
    Ok(())
}

fn stage_contents(
    path: &AnchoredEntry,
    contents: &[u8],
) -> Result<(ApprovedWritePaths, std::fs::File)> {
    let (staged, mut file) = create_staging_file(path)?;
    let result = (|| -> Result<()> {
        maybe_fail_staging_write(path, &staged)?;
        file.write_all(contents)
            .with_context(|| format!("staging write to {}", staged.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing staged write to {}", staged.display()))?;
        Ok(())
    })();
    if let Err(error) = result {
        let cleanup = remove_owned_regular_artifact(&staged, &file, None, "failed staging write");
        return match cleanup {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(staging_cleanup_error(
                path,
                anyhow!("{error:#}; failed staging write cleanup also failed: {cleanup_error:#}"),
            )),
        };
    }
    match ApprovedWritePaths::new(path, staged.clone()) {
        Ok(paths) => Ok((paths, file)),
        Err(error) => {
            let cleanup = remove_owned_regular_artifact(
                &staged,
                &file,
                Some(contents),
                "rejected staging write",
            );
            match cleanup {
                Ok(()) => Err(error),
                Err(cleanup_error) => Err(staging_cleanup_error(
                    path,
                    anyhow!(
                        "{error:#}; rejected staging write cleanup also failed: {cleanup_error:#}"
                    ),
                )),
            }
        }
    }
}

#[cfg(test)]
enum ConditionalWriteAction {
    ReplaceRegular {
        target: PathBuf,
        contents: Vec<u8>,
    },
    MutateRegular {
        target: PathBuf,
        contents: Vec<u8>,
    },
    MutateThroughHardLink {
        target: PathBuf,
        alias: PathBuf,
        contents: Vec<u8>,
    },
    #[cfg(unix)]
    ReplaceSymlink {
        target: PathBuf,
        destination: PathBuf,
    },
    ReplaceDirectory {
        target: PathBuf,
    },
    #[cfg(unix)]
    ReplaceFifo {
        target: PathBuf,
    },
    #[cfg(unix)]
    SwapParentDirectory {
        parent: PathBuf,
        moved_parent: PathBuf,
    },
}

#[cfg(test)]
type ConditionalWriteHook = std::sync::Mutex<BTreeMap<PathBuf, Vec<ConditionalWriteAction>>>;

#[cfg(test)]
type DirectCleanupHook = std::sync::Mutex<BTreeSet<PathBuf>>;

#[cfg(test)]
type StagingWriteFailureHook = std::sync::Mutex<BTreeMap<PathBuf, Vec<u8>>>;

#[cfg(test)]
type CleanupArtifactReplacementHook = std::sync::Mutex<BTreeMap<PathBuf, Vec<u8>>>;

#[cfg(test)]
type VerifiedCleanupCaptureReplacementHook = std::sync::Mutex<BTreeMap<PathBuf, Vec<u8>>>;

#[cfg(test)]
type EarlyStagingMoveHook = std::sync::Mutex<BTreeSet<PathBuf>>;

#[cfg(test)]
type DirectReadGrowthHook = std::sync::Mutex<BTreeMap<PathBuf, u64>>;

#[cfg(test)]
struct PreparedParentSwap {
    moved_parent: PathBuf,
    replacement: PreparedParentReplacement,
}

#[cfg(test)]
enum PreparedParentReplacement {
    Directory,
    #[cfg(unix)]
    Symlink(PathBuf),
}

#[cfg(test)]
type PreparedParentSwapHook = std::sync::Mutex<BTreeMap<PathBuf, PreparedParentSwap>>;

#[cfg(all(test, unix))]
struct EvaluatedHomeSwap {
    moved_home: PathBuf,
    destination: PathBuf,
}

#[cfg(all(test, unix))]
type EvaluatedHomeSwapHook = std::sync::Mutex<BTreeMap<PathBuf, EvaluatedHomeSwap>>;

#[cfg(all(test, unix))]
type EvaluatedHomeSymlinkSwapHook = std::sync::Mutex<BTreeMap<PathBuf, PathBuf>>;

#[cfg(test)]
enum RollbackSabotage {
    Remove(PathBuf),
    Replace(PathBuf, Vec<u8>),
}

#[cfg(test)]
type ConditionalRollbackSabotage = std::sync::Mutex<BTreeMap<PathBuf, RollbackSabotage>>;

#[cfg(test)]
type ConditionalAtomicReplacementHook =
    std::sync::Mutex<BTreeMap<PathBuf, Vec<(PathBuf, Vec<u8>)>>>;

#[cfg(test)]
fn conditional_write_hook() -> &'static ConditionalWriteHook {
    static HOOK: std::sync::OnceLock<ConditionalWriteHook> = std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
fn direct_cleanup_hook() -> &'static DirectCleanupHook {
    static HOOK: std::sync::OnceLock<DirectCleanupHook> = std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(BTreeSet::new()))
}

#[cfg(test)]
fn staging_write_failure_hook() -> &'static StagingWriteFailureHook {
    static HOOK: std::sync::OnceLock<StagingWriteFailureHook> = std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
fn cleanup_artifact_replacement_hook() -> &'static CleanupArtifactReplacementHook {
    static HOOK: std::sync::OnceLock<CleanupArtifactReplacementHook> = std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
fn verified_cleanup_capture_replacement_hook() -> &'static VerifiedCleanupCaptureReplacementHook {
    static HOOK: std::sync::OnceLock<VerifiedCleanupCaptureReplacementHook> =
        std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
fn early_staging_move_hook() -> &'static EarlyStagingMoveHook {
    static HOOK: std::sync::OnceLock<EarlyStagingMoveHook> = std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(BTreeSet::new()))
}

#[cfg(test)]
fn direct_read_growth_hook() -> &'static DirectReadGrowthHook {
    static HOOK: std::sync::OnceLock<DirectReadGrowthHook> = std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
fn prepared_parent_swap_hook() -> &'static PreparedParentSwapHook {
    static HOOK: std::sync::OnceLock<PreparedParentSwapHook> = std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

#[cfg(all(test, unix))]
fn evaluated_home_swap_hook() -> &'static EvaluatedHomeSwapHook {
    static HOOK: std::sync::OnceLock<EvaluatedHomeSwapHook> = std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

#[cfg(all(test, unix))]
fn evaluated_home_symlink_swap_hook() -> &'static EvaluatedHomeSymlinkSwapHook {
    static HOOK: std::sync::OnceLock<EvaluatedHomeSymlinkSwapHook> = std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
fn conditional_rollback_sabotage() -> &'static ConditionalRollbackSabotage {
    static HOOK: std::sync::OnceLock<ConditionalRollbackSabotage> = std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
fn conditional_atomic_replacement_hook() -> &'static ConditionalAtomicReplacementHook {
    static HOOK: std::sync::OnceLock<ConditionalAtomicReplacementHook> = std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
pub(crate) fn set_direct_cleanup_failure(path: PathBuf) {
    direct_cleanup_hook()
        .lock()
        .expect("direct cleanup hook lock poisoned")
        .insert(path);
}

#[cfg(test)]
pub(crate) fn set_staging_write_cleanup_failure(path: PathBuf, replacement: Vec<u8>) {
    staging_write_failure_hook()
        .lock()
        .expect("staging write failure hook lock poisoned")
        .insert(path, replacement);
}

#[cfg(test)]
pub(crate) fn set_cleanup_artifact_replacement(path: PathBuf, replacement: Vec<u8>) {
    cleanup_artifact_replacement_hook()
        .lock()
        .expect("cleanup artifact replacement hook lock poisoned")
        .insert(path, replacement);
}

#[cfg(test)]
fn set_verified_cleanup_capture_replacement(parent: PathBuf, replacement: Vec<u8>) {
    verified_cleanup_capture_replacement_hook()
        .lock()
        .expect("verified cleanup capture replacement hook lock poisoned")
        .insert(parent, replacement);
}

#[cfg(test)]
fn set_early_staging_move(path: PathBuf) {
    early_staging_move_hook()
        .lock()
        .expect("early staging move hook lock poisoned")
        .insert(path);
}

#[cfg(test)]
fn set_direct_read_growth(path: PathBuf, length: u64) {
    direct_read_growth_hook()
        .lock()
        .expect("direct read growth hook lock poisoned")
        .insert(path, length);
}

#[cfg(test)]
fn set_prepared_parent_swap(target: PathBuf, moved_parent: PathBuf) {
    prepared_parent_swap_hook()
        .lock()
        .expect("prepared parent swap hook lock poisoned")
        .insert(
            target,
            PreparedParentSwap {
                moved_parent,
                replacement: PreparedParentReplacement::Directory,
            },
        );
}

#[cfg(all(test, unix))]
fn set_evaluated_home_swap(home: PathBuf, moved_home: PathBuf, destination: PathBuf) {
    evaluated_home_swap_hook()
        .lock()
        .expect("evaluated home swap hook lock poisoned")
        .insert(
            home,
            EvaluatedHomeSwap {
                moved_home,
                destination,
            },
        );
}

#[cfg(all(test, unix))]
fn set_evaluated_home_symlink_swap(home: PathBuf, destination: PathBuf) {
    evaluated_home_symlink_swap_hook()
        .lock()
        .expect("evaluated home symlink swap hook lock poisoned")
        .insert(home, destination);
}

#[cfg(all(test, unix))]
fn set_prepared_parent_symlink_swap(target: PathBuf, moved_parent: PathBuf, destination: PathBuf) {
    prepared_parent_swap_hook()
        .lock()
        .expect("prepared parent swap hook lock poisoned")
        .insert(
            target,
            PreparedParentSwap {
                moved_parent,
                replacement: PreparedParentReplacement::Symlink(destination),
            },
        );
}

#[cfg(test)]
fn maybe_swap_prepared_parent(path: &AnchoredEntry) -> Result<()> {
    let Some(swap) = prepared_parent_swap_hook()
        .lock()
        .expect("prepared parent swap hook lock poisoned")
        .remove(path.as_ref())
    else {
        return Ok(());
    };
    let parent = path
        .parent()
        .context("prepared parent swap target has no parent")?;
    std::fs::rename(parent, &swap.moved_parent).with_context(|| {
        format!(
            "moving prepared parent {} to {}",
            parent.display(),
            swap.moved_parent.display()
        )
    })?;
    match swap.replacement {
        PreparedParentReplacement::Directory => std::fs::create_dir(parent)
            .with_context(|| format!("creating replacement parent {}", parent.display()))?,
        #[cfg(unix)]
        PreparedParentReplacement::Symlink(destination) => {
            std::os::unix::fs::symlink(&destination, parent).with_context(|| {
                format!(
                    "linking replacement parent {} to {}",
                    parent.display(),
                    destination.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(not(test))]
fn maybe_swap_prepared_parent(_path: &AnchoredEntry) -> Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
fn maybe_swap_evaluated_home(home: &Path) -> Result<()> {
    if let Some(destination) = evaluated_home_symlink_swap_hook()
        .lock()
        .expect("evaluated home symlink swap hook lock poisoned")
        .remove(home)
    {
        std::fs::remove_file(home)?;
        std::os::unix::fs::symlink(destination, home)?;
        return Ok(());
    }

    let canonical_home = home
        .canonicalize()
        .with_context(|| format!("canonicalizing test home {}", home.display()))?;
    let Some(swap) = evaluated_home_swap_hook()
        .lock()
        .expect("evaluated home swap hook lock poisoned")
        .remove(&canonical_home)
    else {
        return Ok(());
    };
    std::fs::rename(&canonical_home, &swap.moved_home)?;
    std::os::unix::fs::symlink(&swap.destination, &canonical_home)?;
    Ok(())
}

#[cfg(not(all(test, unix)))]
fn maybe_swap_evaluated_home(_home: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
fn maybe_grow_direct_read_target(path: &Path) -> Result<()> {
    let Some(length) = direct_read_growth_hook()
        .lock()
        .expect("direct read growth hook lock poisoned")
        .remove(path)
    else {
        return Ok(());
    };
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("opening direct target {} for test growth", path.display()))?
        .set_len(length)
        .with_context(|| format!("growing direct target {} for test", path.display()))
}

#[cfg(not(test))]
fn maybe_grow_direct_read_target(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
pub(crate) fn set_conditional_rollback_sabotage(trigger: PathBuf, target: PathBuf) {
    conditional_rollback_sabotage()
        .lock()
        .expect("conditional rollback sabotage lock poisoned")
        .insert(trigger, RollbackSabotage::Remove(target));
}

#[cfg(test)]
fn set_conditional_rollback_backup_replacement(
    trigger: PathBuf,
    target: PathBuf,
    replacement: Vec<u8>,
) {
    conditional_rollback_sabotage()
        .lock()
        .expect("conditional rollback sabotage lock poisoned")
        .insert(trigger, RollbackSabotage::Replace(target, replacement));
}

#[cfg(test)]
fn set_conditional_atomic_replacement(trigger: PathBuf, target: PathBuf, replacement: Vec<u8>) {
    conditional_atomic_replacement_hook()
        .lock()
        .expect("conditional atomic replacement hook lock poisoned")
        .entry(trigger)
        .or_default()
        .push((target, replacement));
}

#[cfg(test)]
fn maybe_fail_direct_cleanup(path: &Path) -> Result<()> {
    if direct_cleanup_hook()
        .lock()
        .expect("direct cleanup hook lock poisoned")
        .remove(path)
    {
        bail!("injected direct cleanup failure");
    }
    Ok(())
}

#[cfg(not(test))]
fn maybe_fail_direct_cleanup(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
fn maybe_fail_staging_write(path: &Path, staged: &Path) -> Result<()> {
    let Some(replacement) = staging_write_failure_hook()
        .lock()
        .expect("staging write failure hook lock poisoned")
        .remove(path)
    else {
        return Ok(());
    };
    std::fs::remove_file(staged)
        .with_context(|| format!("removing staged file {}", staged.display()))?;
    std::fs::write(staged, replacement)
        .with_context(|| format!("replacing staged file {}", staged.display()))?;
    bail!("injected staging write failure");
}

#[cfg(not(test))]
fn maybe_fail_staging_write(_path: &Path, _staged: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
fn maybe_replace_cleanup_artifact(path: &Path, artifact: &Path) -> Result<()> {
    let Some(replacement) = cleanup_artifact_replacement_hook()
        .lock()
        .expect("cleanup artifact replacement hook lock poisoned")
        .remove(path)
    else {
        return Ok(());
    };
    std::fs::remove_file(artifact)
        .with_context(|| format!("removing cleanup artifact {}", artifact.display()))?;
    std::fs::write(artifact, replacement)
        .with_context(|| format!("replacing cleanup artifact {}", artifact.display()))
}

#[cfg(not(test))]
fn maybe_replace_cleanup_artifact(_path: &Path, _artifact: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
fn maybe_replace_verified_cleanup_capture(captured: &Path) -> Result<()> {
    let parent = captured
        .parent()
        .context("verified cleanup capture has no parent")?;
    let Some(replacement) = verified_cleanup_capture_replacement_hook()
        .lock()
        .expect("verified cleanup capture replacement hook lock poisoned")
        .remove(parent)
    else {
        return Ok(());
    };
    std::fs::remove_file(captured)
        .with_context(|| format!("removing verified cleanup capture {}", captured.display()))?;
    std::fs::write(captured, replacement)
        .with_context(|| format!("replacing verified cleanup capture {}", captured.display()))
}

#[cfg(not(test))]
fn maybe_replace_verified_cleanup_capture(_captured: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
pub(crate) fn set_conditional_write_hook(path: PathBuf, replacement: Vec<u8>) {
    set_conditional_write_actions(path.clone(), vec![(path, replacement)]);
}

#[cfg(test)]
fn set_conditional_write_actions(trigger: PathBuf, actions: Vec<(PathBuf, Vec<u8>)>) {
    set_conditional_write_test_actions(
        trigger,
        actions
            .into_iter()
            .map(|(target, contents)| ConditionalWriteAction::ReplaceRegular { target, contents })
            .collect(),
    );
}

#[cfg(test)]
fn set_conditional_write_test_actions(trigger: PathBuf, actions: Vec<ConditionalWriteAction>) {
    conditional_write_hook()
        .lock()
        .expect("conditional write hook lock poisoned")
        .insert(trigger, actions);
}

#[cfg(test)]
fn replace_regular_final_component(target: &Path, contents: &[u8]) -> Result<()> {
    let target = anchored_test_entry(target)?;
    let (staged, mut file) = create_staging_file(&target)?;
    let result = (|| -> Result<()> {
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        #[cfg(windows)]
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("removing replaced test target"),
        }
        std::fs::rename(&staged, &target)
            .with_context(|| format!("replacing final component {}", target.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(staged);
    }
    result
}

#[cfg(test)]
fn maybe_run_conditional_write_hook(path: &Path) -> Result<()> {
    if early_staging_move_hook()
        .lock()
        .expect("early staging move hook lock poisoned")
        .remove(path)
    {
        let parent = path.parent().context("staging move target has no parent")?;
        let name = path
            .file_name()
            .context("staging move target has no file name")?
            .to_string_lossy();
        let prefix = format!(".{name}.ward-staged-");
        let staged = std::fs::read_dir(parent)?
            .filter_map(|entry| entry.ok())
            .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .with_context(|| format!("finding staged file for {}", path.display()))?;
        std::fs::rename(staged.path(), path)?;
    }
    let mut hook = conditional_write_hook()
        .lock()
        .expect("conditional write hook lock poisoned");
    if let Some(actions) = hook.remove(path) {
        for action in actions {
            match action {
                ConditionalWriteAction::ReplaceRegular { target, contents } => {
                    replace_regular_final_component(&target, &contents)?;
                }
                ConditionalWriteAction::MutateRegular { target, contents } => {
                    std::fs::write(&target, contents)
                        .with_context(|| format!("mutating {}", target.display()))?;
                }
                ConditionalWriteAction::MutateThroughHardLink {
                    target,
                    alias,
                    contents,
                } => {
                    std::fs::hard_link(&target, &alias).with_context(|| {
                        format!(
                            "hard-linking {} to {} for test mutation",
                            target.display(),
                            alias.display()
                        )
                    })?;
                    std::fs::write(&alias, contents).with_context(|| {
                        format!("mutating {} through a hard link", target.display())
                    })?;
                }
                #[cfg(unix)]
                ConditionalWriteAction::ReplaceSymlink {
                    target,
                    destination,
                } => {
                    use std::os::unix::fs::symlink;

                    std::fs::remove_file(&target)?;
                    symlink(destination, &target).with_context(|| {
                        format!("replacing {} with a symlink", target.display())
                    })?;
                }
                ConditionalWriteAction::ReplaceDirectory { target } => {
                    std::fs::remove_file(&target)?;
                    std::fs::create_dir(&target).with_context(|| {
                        format!("replacing {} with a directory", target.display())
                    })?;
                }
                #[cfg(unix)]
                ConditionalWriteAction::ReplaceFifo { target } => {
                    use std::os::unix::ffi::OsStrExt;

                    std::fs::remove_file(&target)?;
                    let target_c = CString::new(target.as_os_str().as_bytes())
                        .context("FIFO test target contains NUL")?;
                    // SAFETY: `target_c` is a live, NUL-terminated pathname.
                    if unsafe { libc::mkfifo(target_c.as_ptr(), 0o600) } != 0 {
                        return Err(std::io::Error::last_os_error()).with_context(|| {
                            format!("replacing {} with a FIFO", target.display())
                        });
                    }
                }
                #[cfg(unix)]
                ConditionalWriteAction::SwapParentDirectory {
                    parent,
                    moved_parent,
                } => {
                    std::fs::rename(&parent, &moved_parent).with_context(|| {
                        format!(
                            "moving prepared parent {} to {}",
                            parent.display(),
                            moved_parent.display()
                        )
                    })?;
                    std::fs::create_dir(&parent).with_context(|| {
                        format!("creating replacement parent {}", parent.display())
                    })?;
                }
            }
        }
    }
    drop(hook);
    if let Some(sabotage) = conditional_rollback_sabotage()
        .lock()
        .expect("conditional rollback sabotage lock poisoned")
        .remove(path)
    {
        let target = match &sabotage {
            RollbackSabotage::Remove(target) | RollbackSabotage::Replace(target, _) => target,
        };
        let parent = target
            .parent()
            .context("rollback sabotage target has no parent")?;
        let name = target
            .file_name()
            .context("rollback sabotage target has no file name")?
            .to_string_lossy();
        let prefixes = [
            format!(".{name}.ward-staged-"),
            format!(".{name}.ward-displaced-"),
        ];
        let artifact = std::fs::read_dir(parent)?
            .filter_map(|entry| entry.ok())
            .find(|entry| {
                let candidate = entry.file_name();
                let candidate = candidate.to_string_lossy();
                prefixes.iter().any(|prefix| candidate.starts_with(prefix))
            })
            .with_context(|| format!("finding rollback artifact for {}", target.display()))?;
        match sabotage {
            RollbackSabotage::Remove(_) => {
                std::fs::remove_file(artifact.path())
                    .with_context(|| format!("sabotaging rollback for {}", target.display()))?;
            }
            RollbackSabotage::Replace(_, replacement) => {
                let artifact = artifact.path();
                std::fs::remove_file(&artifact)?;
                std::fs::write(&artifact, replacement)?;
            }
        }
    }
    if let Some(actions) = conditional_atomic_replacement_hook()
        .lock()
        .expect("conditional atomic replacement hook lock poisoned")
        .remove(path)
    {
        for (target, replacement) in actions {
            let target = anchored_test_entry(&target)?;
            let (staged, mut file) = create_staging_file(&target)?;
            file.write_all(&replacement)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&staged, &target)?;
        }
    }
    Ok(())
}

#[cfg(not(test))]
fn maybe_run_conditional_write_hook(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
fn anchored_test_entry(path: &Path) -> Result<AnchoredEntry> {
    let parent = path
        .parent()
        .context("test entry has no parent directory")?;
    let name = path.file_name().context("test entry has no file name")?;
    let directory = Dir::open_ambient_dir(parent, ambient_authority())
        .with_context(|| format!("opening test parent {}", parent.display()))?;
    Ok(AnchoredEntry::new(Arc::new(directory), parent, name))
}

#[cfg(not(windows))]
fn replace_preserving_target(
    target: &AnchoredEntry,
    staged: &AnchoredEntry,
    displaced: &AnchoredEntry,
) -> Result<()> {
    atomic_exchange_preserving_target(target, staged, displaced)
}

#[cfg(windows)]
fn replace_preserving_target(
    target: &AnchoredEntry,
    staged: &AnchoredEntry,
    displaced: &AnchoredEntry,
) -> Result<()> {
    atomic_move_without_replace(target, displaced)
        .with_context(|| format!("moving target {} to backup", target.display()))?;
    if let Err(error) = atomic_move_without_replace(staged, target) {
        return match atomic_move_without_replace(displaced, target) {
            Ok(()) => Err(error.context("installing staged write")),
            Err(restore_error) => Err(anyhow!(
                "installing staged write failed: {error:#}; restoring the displaced \
                 target also failed: {restore_error:#}"
            )),
        };
    }
    Ok(())
}

#[cfg(not(windows))]
fn restore_displaced_target(
    target: &AnchoredEntry,
    staged: &AnchoredEntry,
    displaced: &AnchoredEntry,
) -> Result<()> {
    atomic_exchange_preserving_target(target, displaced, staged)
}

#[cfg(windows)]
fn restore_displaced_target(
    target: &AnchoredEntry,
    staged: &AnchoredEntry,
    displaced: &AnchoredEntry,
) -> Result<()> {
    atomic_move_without_replace(target, staged)
        .with_context(|| format!("moving Ward-owned target {} aside", target.display()))?;
    if let Err(error) = atomic_move_without_replace(displaced, target) {
        return match atomic_move_without_replace(staged, target) {
            Ok(()) => Err(error.context("restoring displaced target")),
            Err(restore_error) => Err(anyhow!(
                "restoring displaced target failed: {error:#}; restoring the Ward-owned \
                 target also failed: {restore_error:#}"
            )),
        };
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn atomic_exchange_preserving_target(
    target: &AnchoredEntry,
    replacement: &AnchoredEntry,
    displaced: &AnchoredEntry,
) -> Result<()> {
    if replacement != displaced {
        bail!("approved-write exchange requires a shared replacement/backup path");
    }
    use std::os::fd::AsFd;

    rustix::fs::renameat_with(
        replacement.parent.as_fd(),
        &replacement.name,
        target.parent.as_fd(),
        &target.name,
        rustix::fs::RenameFlags::EXCHANGE,
    )
    .map_err(std::io::Error::from)
    .context("atomically exchanging approved target")
}

#[cfg(windows)]
fn failed_replace_displaced_target(displaced: &AnchoredEntry) -> bool {
    match displaced.parent.symlink_metadata(&displaced.name) {
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

#[cfg(not(windows))]
fn failed_replace_displaced_target(_displaced: &AnchoredEntry) -> bool {
    false
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn atomic_exchange_preserving_target(
    _target: &AnchoredEntry,
    _replacement: &AnchoredEntry,
    _displaced: &AnchoredEntry,
) -> Result<()> {
    bail!("atomic approved-write exchange is unsupported on this platform")
}

#[cfg(windows)]
fn approved_write_displaced_path(
    target: &AnchoredEntry,
    _staged: &AnchoredEntry,
) -> Result<AnchoredEntry> {
    // Windows needs a distinct, unpredictable no-replace backup path.
    for _ in 0..16 {
        let displaced = displaced_path(target);
        match displaced.parent.symlink_metadata(&displaced.name) {
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
fn approved_write_displaced_path(
    _target: &AnchoredEntry,
    staged: &AnchoredEntry,
) -> Result<AnchoredEntry> {
    Ok(staged.clone())
}

#[cfg(windows)]
fn displaced_path(path: &AnchoredEntry) -> AnchoredEntry {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    path.sibling(format!(".{name}.ward-displaced-{}", uuid::Uuid::new_v4()).into())
}

/// Create a fresh sibling staging file for an atomic write.
///
/// The staging path is intentionally unpredictable and opened with
/// `create_new(true)` so a pre-planted symlink or hard link cannot be followed
/// before the final rename commits the edit into the Gate-2-validated target.
fn create_staging_file(path: &AnchoredEntry) -> Result<(AnchoredEntry, std::fs::File)> {
    for _ in 0..16 {
        let staged = staged_path(path);
        #[cfg(unix)]
        let opened = {
            use std::os::fd::AsFd;

            rustix::fs::openat(
                staged.parent.as_fd(),
                &staged.name,
                rustix::fs::OFlags::WRONLY
                    | rustix::fs::OFlags::CREATE
                    | rustix::fs::OFlags::EXCL
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NOFOLLOW,
                rustix::fs::Mode::from_raw_mode(0o666),
            )
            .map(std::fs::File::from)
        };
        #[cfg(not(unix))]
        let opened = {
            let mut options = CapOpenOptions::new();
            options
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            staged
                .parent
                .open_with(&staged.name, &options)
                .map(cap_std::fs::File::into_std)
        };
        match opened {
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
fn staged_path(path: &AnchoredEntry) -> AnchoredEntry {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    path.sibling(format!(".{name}.ward-staged-{}", uuid::Uuid::new_v4()).into())
}

/// Re-create and verify the staging parent directory component-by-component,
/// never following symlinks.
///
/// `parent` must be the Gate-2-resolved target's parent, lexically under
/// `canonical_home`. Each component is created with a single (non-recursive,
/// non-following) `create_dir` and then checked via `symlink_metadata` to be a
/// real directory — so a component swapped for an escaping symlink after
/// adjudication fails closed without creating anything outside the home.
fn prepare_staging_parent(home: &AnchoredHome, parent: &Path) -> Result<AnchoredParent> {
    let rel = parent.strip_prefix(&home.absolute).map_err(|_| {
        anyhow!(
            "staging parent `{}` is not under the familiar home `{}`",
            parent.display(),
            home.absolute.display()
        )
    })?;

    let mut verified = home.absolute.clone();
    let mut directory = home
        .dir
        .try_clone()
        .with_context(|| format!("duplicating familiar home {}", home.absolute.display()))?;
    for component in rel.components() {
        let Component::Normal(part) = component else {
            bail!(
                "staging parent `{}` contains a non-normal path component",
                parent.display()
            );
        };
        verified.push(part);

        let exists = entry_is_directory_nofollow(&directory, part)
            .with_context(|| format!("verifying staging parent {}", verified.display()))?;

        if !exists {
            match create_child_directory(&directory, part) {
                Ok(()) => {}
                // A concurrent apply may have created it; the re-check below rules.
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
                Err(err) => {
                    return Err(err).with_context(|| format!("creating {}", verified.display()))
                }
            }
            let is_directory = entry_is_directory_nofollow(&directory, part)
                .with_context(|| format!("verifying staging parent {}", verified.display()))?;
            if !is_directory {
                bail!(
                    "staging parent component `{}` is not a real directory — refusing to \
                     follow it outside the familiar home",
                    verified.display()
                );
            }
        }
        directory = open_child_dir_nofollow(&directory, part)
            .with_context(|| format!("opening staging parent {}", verified.display()))?;
    }
    Ok(AnchoredParent {
        dir: Arc::new(directory),
        absolute: verified,
    })
}

#[cfg(target_os = "linux")]
fn open_child_dir_nofollow(parent: &Dir, name: &OsStr) -> std::io::Result<Dir> {
    use std::os::fd::AsFd;

    use rustix::fs::{openat2, Mode, OFlags, ResolveFlags};

    match openat2(
        parent.as_fd(),
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_MAGICLINKS | ResolveFlags::NO_SYMLINKS,
    ) {
        Ok(fd) => Ok(Dir::from_std_file(fd.into())),
        Err(error) if error.raw_os_error() == libc::ENOSYS => parent.open_dir_nofollow(name),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(target_os = "linux"))]
fn open_child_dir_nofollow(parent: &Dir, name: &OsStr) -> std::io::Result<Dir> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;

        rustix::fs::openat(
            parent.as_fd(),
            name,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::CLOEXEC
                | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )
        .map(|fd| Dir::from_std_file(fd.into()))
        .map_err(Into::into)
    }
    #[cfg(not(unix))]
    {
        parent.open_dir_nofollow(name)
    }
}

fn entry_is_directory_nofollow(parent: &Dir, name: &OsStr) -> Result<bool> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;

        match rustix::fs::statat(parent.as_fd(), name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
            Ok(metadata) => {
                if rustix::fs::FileType::from_raw_mode(metadata.st_mode)
                    == rustix::fs::FileType::Directory
                {
                    Ok(true)
                } else {
                    bail!("staging parent component is not a real directory")
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
    #[cfg(not(unix))]
    {
        match parent.symlink_metadata(name) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
            Ok(_) => bail!("staging parent component is not a real directory"),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

fn create_child_directory(parent: &Dir, name: &OsStr) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;

        rustix::fs::mkdirat(parent.as_fd(), name, rustix::fs::Mode::from_raw_mode(0o777))
            .map_err(Into::into)
    }
    #[cfg(not(unix))]
    {
        parent.create_dir(name)
    }
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

/// Portable uniqueness key for a Gate-2-resolved surface.
///
/// Familiar proposals must not contain case-only aliases: they name one file
/// on default macOS and Windows filesystems even when they remain distinct on
/// a case-sensitive checkout. Rejecting them everywhere keeps staged evidence
/// portable and follows the Ward's existing case-insensitive collision posture.
pub(crate) fn portable_surface_key(surface: &str) -> String {
    surface.chars().case_fold().nfc().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    const FIFO_WATCHDOG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
    #[cfg(unix)]
    const FIFO_CLEANUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

    #[cfg(unix)]
    fn unblock_fifo_worker(path: &Path) -> Result<()> {
        use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};

        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("inspecting watchdog FIFO {}", path.display()))?;
        anyhow::ensure!(
            metadata.file_type().is_fifo(),
            "watchdog target {} is no longer a FIFO",
            path.display()
        );
        if metadata.permissions().mode() & 0o600 != 0o600 {
            let mut permissions = metadata.permissions();
            permissions.set_mode(permissions.mode() | 0o600);
            std::fs::set_permissions(path, permissions)
                .with_context(|| format!("making watchdog FIFO accessible {}", path.display()))?;
        }
        let mut endpoint = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(path)
            .with_context(|| format!("opening watchdog FIFO endpoint {}", path.display()))?;
        match endpoint.write_all(b"\n") {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("nudging watchdog FIFO {}", path.display()))
            }
        }
    }

    #[cfg(unix)]
    fn run_fifo_operation_with_watchdog<T: Send + 'static>(
        fifo: &Path,
        description: &'static str,
        operation: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation));
            let _ = sender.send(outcome);
        });

        match receiver.recv_timeout(FIFO_WATCHDOG_TIMEOUT) {
            Ok(outcome) => {
                worker
                    .join()
                    .unwrap_or_else(|_| panic!("{description} watchdog worker panicked"));
                match outcome {
                    Ok(result) => result,
                    Err(panic) => std::panic::resume_unwind(panic),
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                worker.join().unwrap_or_else(|_| {
                    panic!("{description} watchdog worker panicked before reporting completion")
                });
                panic!("{description} watchdog worker exited without reporting completion");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let unblock = unblock_fifo_worker(fifo);
                let cleanup = receiver.recv_timeout(FIFO_CLEANUP_TIMEOUT);
                let cleanup_status = match &cleanup {
                    Ok(_) => "worker completed after FIFO unblock",
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        "worker disconnected during FIFO unblock"
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        "worker remained blocked after FIFO unblock"
                    }
                };
                if !matches!(cleanup, Err(std::sync::mpsc::RecvTimeoutError::Timeout)) {
                    worker.join().unwrap_or_else(|_| {
                        panic!("{description} watchdog worker panicked during timeout cleanup")
                    });
                }
                panic!(
                    "{description} timed out after {FIFO_WATCHDOG_TIMEOUT:?}; \
                     FIFO unblock result: {unblock:?}; {cleanup_status}"
                );
            }
        }
    }

    #[cfg(windows)]
    const _: () = assert!(supports_atomic_approved_writes());

    #[test]
    fn windows_approved_commits_avoid_the_symlink_following_replacement_api() {
        let ward_source = include_str!("ward.rs");
        let unsafe_api = ["Replace", "FileW"].concat();

        assert!(
            !ward_source.contains(&unsafe_api),
            "approved Windows commits must use the no-replace displaced-target primitive"
        );
    }

    #[test]
    fn direct_apply_memory_model_has_one_payload_and_fixed_verification_scratch() {
        let ward_source = include_str!("ward.rs");
        let prepared_start = ward_source.find("struct PreparedDirectWrite").unwrap();
        let prepared_end = ward_source[prepared_start..]
            .find("struct PreparedConditionalWrite")
            .map(|offset| prepared_start + offset)
            .unwrap();
        let direct_batch_start = ward_source.find("fn write_direct_batch").unwrap();
        let direct_batch_end = ward_source[direct_batch_start..]
            .find("fn reserve_ward_content_bytes")
            .map(|offset| direct_batch_start + offset)
            .unwrap();
        let stream_start = ward_source
            .find("fn stream_regular_file_matches_and_sha256")
            .unwrap();
        let stream_end = ward_source[stream_start..]
            .find("fn verify_installed_regular_target")
            .map(|offset| stream_start + offset)
            .unwrap();
        let stream_verification = &ward_source[stream_start..stream_end];
        let full_open = ["open_regular_file_without_", "following_links("].concat();
        let full_read = ["read_regular_file_without_", "following_links("].concat();
        let bounded_read = ["read_open_file_with_", "policy("].concat();
        let verification_sections = [
            (
                "fn rollback_direct_created_write",
                "fn cleanup_direct_rollback_staging",
            ),
            (
                "fn rollback_created_write",
                "fn restore_unowned_rollback_capture",
            ),
            (
                "fn verify_owned_regular_artifact",
                "fn ensure_artifact_path_absent",
            ),
            (
                "fn verify_installed_regular_target",
                "fn open_file_matches_path_if_present",
            ),
        ];

        assert!(
            ward_source[prepared_start..prepared_end].contains("new_contents: &'a [u8]")
                && !ward_source[prepared_start..prepared_end].contains("Vec<u8>"),
            "PreparedDirectWrite must borrow the caller's proposed payload"
        );
        assert!(
            !ward_source[direct_batch_start..direct_batch_end]
                .contains("edit.new_contents.clone()"),
            "direct preparation must not clone proposed payloads"
        );
        assert!(
            ward_source.contains("const DIRECT_VERIFICATION_SCRATCH_BYTES: usize = 64 * 1024;"),
            "verification scratch must remain a fixed 64 KiB buffer"
        );
        assert!(
            stream_verification.contains("[0_u8; DIRECT_VERIFICATION_SCRATCH_BYTES]")
                && !stream_verification.contains("Vec<")
                && !stream_verification.contains("read_to_end")
                && !stream_verification.contains(&bounded_read),
            "stream verification must use only the fixed scratch array"
        );
        for (start, end) in verification_sections {
            let start = ward_source.find(start).unwrap();
            let end = ward_source[start..]
                .find(end)
                .map(|offset| start + offset)
                .unwrap();
            let section = &ward_source[start..end];
            assert!(
                !section.contains(&full_open)
                    && !section.contains(&full_read)
                    && !section.contains(&bounded_read),
                "{start} must stream verification instead of allocating a full-file Vec"
            );
        }
    }

    #[test]
    fn approved_apply_memory_model_borrows_proposed_payloads() {
        let ward_source = include_str!("ward.rs");
        let prepared_start = ward_source.find("struct PreparedConditionalWrite").unwrap();
        let prepared_end = ward_source[prepared_start..]
            .find("fn write_direct_batch")
            .map(|offset| prepared_start + offset)
            .unwrap();
        let conditional_start = ward_source
            .find("fn write_atomically_if_unchanged")
            .unwrap();
        let conditional_end = ward_source[conditional_start..]
            .find("fn rollback_conditional_writes")
            .map(|offset| conditional_start + offset)
            .unwrap();

        assert!(
            ward_source[prepared_start..prepared_end].contains("new_contents: &'a [u8]")
                && !ward_source[prepared_start..prepared_end].contains("new_contents: Vec<u8>"),
            "approved preparation must borrow the caller's proposed payload"
        );
        assert!(
            !ward_source[conditional_start..conditional_end].contains("edit.new_contents.clone()"),
            "approved preparation must not clone proposed payloads"
        );
    }

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
            probe: Vec::new(),
        }
    }

    fn ward_in(dir: &Path) -> Ward {
        Ward::new(dir.to_path_buf(), sample_config()).expect("valid ward")
    }

    fn resolved_as_target(edits: &[FileEdit]) -> BTreeMap<String, String> {
        edits
            .iter()
            .map(|edit| (edit.target.clone(), edit.target.clone()))
            .collect()
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

    fn staging_artifact_contents(dir: &Path) -> Vec<Vec<u8>> {
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.contains(".ward-staged") || name.contains(".ward-displaced")
            })
            .filter_map(|entry| fs::read(entry.path()).ok())
            .collect()
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn unix_approved_write_paths_reuse_staging_path_for_exchange() {
        let tmp = tempfile::tempdir().unwrap();
        let target = anchored_test_entry(&tmp.path().join("SOUL.md")).unwrap();
        let staged = target.sibling(".SOUL.md.ward-staged-test".into());
        let paths = ApprovedWritePaths::new(&target, staged.clone()).unwrap();

        assert_eq!(paths.staged, staged);
        assert_eq!(paths.displaced, staged);
    }

    #[cfg(windows)]
    #[test]
    fn windows_approved_write_paths_select_distinct_displaced_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        let target = anchored_test_entry(&tmp.path().join("SOUL.md")).unwrap();
        let staged = target.sibling(".SOUL.md.ward-staged-test".into());
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
    fn windows_no_replace_commit_preserves_displaced_bytes_for_commit_and_rollback() {
        let tmp = tempfile::tempdir().unwrap();
        let target = anchored_test_entry(&tmp.path().join("SOUL.md")).unwrap();
        let staged = target.sibling(".SOUL.md.ward-staged-test".into());
        fs::write(&target, b"old soul").unwrap();
        fs::write(&staged, b"new soul").unwrap();
        let paths = ApprovedWritePaths::new(&target, staged).unwrap();

        replace_preserving_target(&target, &paths.staged, &paths.displaced).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new soul");
        assert_eq!(fs::read(&paths.displaced).unwrap(), b"old soul");
        assert!(!paths.staged.exists());

        restore_displaced_target(&target, &paths.staged, &paths.displaced).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"old soul");
        assert_eq!(fs::read(&paths.staged).unwrap(), b"new soul");
        assert!(!paths.displaced.exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_partial_replace_state_requires_rollback_before_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let displaced =
            anchored_test_entry(&tmp.path().join(".SOUL.md.ward-displaced-test")).unwrap();

        assert!(!failed_replace_displaced_target(&displaced));
        fs::write(&displaced, b"old soul").unwrap();
        assert!(failed_replace_displaced_target(&displaced));
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
        assert!(config.probe.is_empty());
    }

    #[test]
    fn parses_deterministic_probe_declarations() {
        let toml = r#"
principal_key_fingerprint = "SHA256:abc"
protected_surface = []

[[surface]]
path = "reviewed/"
tier = 1

[[probe]]
surface = "reviewed/**"
id = "parse"
format = "markdown-front-matter"

[[probe]]
surface = "reviewed/**"
id = "pattern-lint"
forbidden = ["(?i)ignore previous"]
required = ["(?m)^name:"]
"#;

        let config = WardConfig::from_toml_str(toml).expect("probe config parses");

        assert_eq!(config.probe.len(), 2);
        assert_eq!(config.probe[0].id, ProbeId::Parse);
        assert_eq!(
            config.probe[0].format,
            Some(ProbeFormat::MarkdownFrontMatter)
        );
        assert_eq!(config.probe[1].id, ProbeId::PatternLint);
        assert_eq!(config.probe[1].forbidden, vec!["(?i)ignore previous"]);
        let matcher = config.probe[1].surface_matcher().unwrap();
        assert!(matcher.is_match("reviewed/SKILL.md"));
        assert!(!matcher.is_match("notes/SKILL.md"));
    }

    #[test]
    fn validation_rejects_escaping_or_mistyped_probe_parameters() {
        let escaping = r#"
principal_key_fingerprint = "SHA256:abc"
protected_surface = []

[[probe]]
surface = "../SOUL.md"
id = "size-delta"
"#;
        assert!(WardConfig::from_toml_str(escaping)
            .unwrap_err()
            .to_string()
            .contains("must stay relative"));

        let mistyped = r#"
principal_key_fingerprint = "SHA256:abc"
protected_surface = []

[[probe]]
surface = "reviewed/**"
id = "size-delta"
format = "json"
"#;
        assert!(WardConfig::from_toml_str(mistyped)
            .unwrap_err()
            .to_string()
            .contains("does not accept parameters"));

        let unknown_parameter = r#"
principal_key_fingerprint = "SHA256:abc"
protected_surface = []

[[probe]]
surface = "reviewed/**"
id = "parse"
format = "json"
formatter = "lenient"
"#;
        let error = WardConfig::from_toml_str(unknown_parameter).unwrap_err();
        assert!(format!("{error:#}").contains("unknown field"));
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
        // Valid Gate 1 signature, but the direct apply path cannot represent
        // the authority proposal and explicit principal decision.
        let report = ward
            .apply(&edits, &Authorization::signed_by("SHA256:principal-key"))
            .unwrap();

        // Fail-closed: authorized, but not explicitly approved, so held.
        assert!(report.is_held());
        assert_eq!(
            report.changes[0].decision.verdict,
            Verdict::AuthorizedProtectedChange
        );
        assert!(!tmp.path().join("SOUL.md").exists());
    }

    #[test]
    fn coherence_approval_applies_reviewed_and_cleared_edits_as_a_unit() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir(tmp.path().join("scratch")).unwrap();
        fs::write(tmp.path().join("MEMORY.md"), b"old memory").unwrap();
        fs::write(tmp.path().join("scratch/notes.txt"), b"old notes").unwrap();
        let edits = vec![
            FileEdit::new("MEMORY.md", b"new memory".to_vec()),
            FileEdit::new("scratch/notes.txt", b"new notes".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("MEMORY.md".to_string(), Some(b"old memory".to_vec())),
            ("scratch/notes.txt".to_string(), Some(b"old notes".to_vec())),
        ]);

        let report = ward
            .apply_after_coherence_approval(
                &edits,
                &Authorization::unsigned(),
                &expected,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .unwrap();

        assert!(report.is_applied());
        assert_eq!(
            fs::read(tmp.path().join("MEMORY.md")).unwrap(),
            b"new memory"
        );
        assert_eq!(
            fs::read(tmp.path().join("scratch/notes.txt")).unwrap(),
            b"new notes"
        );
    }

    #[test]
    fn coherence_approval_atomically_creates_a_reviewed_surface() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        let edits = vec![FileEdit::new("MEMORY.md", b"new memory".to_vec())];
        let expected = BTreeMap::from([("MEMORY.md".to_string(), None)]);

        let report = ward
            .apply_after_coherence_approval(
                &edits,
                &Authorization::unsigned(),
                &expected,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .unwrap();

        assert!(report.is_applied());
        assert_eq!(
            fs::read(tmp.path().join("MEMORY.md")).unwrap(),
            b"new memory"
        );
        assert_eq!(staging_litter(tmp.path()), Vec::<String>::new());
    }

    #[test]
    fn coherence_approval_never_overwrites_a_concurrent_create() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        let edits = vec![FileEdit::new("MEMORY.md", b"new memory".to_vec())];
        let expected = BTreeMap::from([("MEMORY.md".to_string(), None)]);
        set_conditional_write_hook(
            tmp.path().canonicalize().unwrap().join("MEMORY.md"),
            b"concurrent memory".to_vec(),
        );

        let error = ward
            .apply_after_coherence_approval(
                &edits,
                &Authorization::unsigned(),
                &expected,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .expect_err("a concurrent create must win without being overwritten");

        assert!(
            format!("{error:#}").contains("appeared after review"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            fs::read(tmp.path().join("MEMORY.md")).unwrap(),
            b"concurrent memory"
        );
        assert_eq!(staging_litter(tmp.path()), Vec::<String>::new());
    }

    #[test]
    fn coherence_initial_approval_rejects_an_existing_same_byte_replacement() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::write(tmp.path().join("MEMORY.md"), b"new memory").unwrap();
        let edits = vec![FileEdit::new("MEMORY.md", b"new memory".to_vec())];
        let expected = BTreeMap::from([("MEMORY.md".to_string(), Some(b"old memory".to_vec()))]);

        let error = ward
            .apply_after_coherence_approval(
                &edits,
                &Authorization::unsigned(),
                &expected,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .expect_err("an initial apply must not infer recovery from matching after-bytes");

        assert!(
            format!("{error:#}").contains("changed after review"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            fs::read(tmp.path().join("MEMORY.md")).unwrap(),
            b"new memory"
        );
    }

    #[test]
    fn coherence_initial_approval_rejects_an_absent_same_byte_create() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::write(tmp.path().join("MEMORY.md"), b"new memory").unwrap();
        let edits = vec![FileEdit::new("MEMORY.md", b"new memory".to_vec())];
        let expected = BTreeMap::from([("MEMORY.md".to_string(), None)]);

        let error = ward
            .apply_after_coherence_approval(
                &edits,
                &Authorization::unsigned(),
                &expected,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .expect_err("an initial create must not infer recovery from matching after-bytes");

        assert!(
            format!("{error:#}").contains("appeared after review"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            fs::read(tmp.path().join("MEMORY.md")).unwrap(),
            b"new memory"
        );
    }

    #[cfg(unix)]
    #[test]
    fn coherence_initial_approval_rejects_a_gate2_retarget() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let mut config = sample_config();
        config.surface.push(SurfaceEntry {
            path: "reviewed/".to_string(),
            tier: Tier::Reviewed,
        });
        let ward = Ward::new(tmp.path(), config).unwrap();
        fs::create_dir_all(tmp.path().join("reviewed/a")).unwrap();
        fs::create_dir_all(tmp.path().join("reviewed/b")).unwrap();
        fs::write(tmp.path().join("reviewed/a/skill.md"), b"old").unwrap();
        fs::write(tmp.path().join("reviewed/b/skill.md"), b"old").unwrap();
        symlink("reviewed/a", tmp.path().join("lane")).unwrap();
        let edits = vec![FileEdit::new("lane/skill.md", b"new".to_vec())];
        let expected = BTreeMap::from([("lane/skill.md".to_string(), Some(b"old".to_vec()))]);
        let initial = ward.evaluate(&Proposal {
            targets: vec!["lane/skill.md".to_string()],
            authorization: Authorization::unsigned(),
        });
        assert_eq!(initial.decisions[0].resolved, "reviewed/a/skill.md");
        let expected_resolved = BTreeMap::from([(
            "lane/skill.md".to_string(),
            initial.decisions[0].resolved.clone(),
        )]);
        fs::remove_file(tmp.path().join("lane")).unwrap();
        symlink("reviewed/b", tmp.path().join("lane")).unwrap();

        ward.apply_after_coherence_approval(
            &edits,
            &Authorization::unsigned(),
            &expected,
            &expected_resolved,
            ApprovedApplyMode::Initial,
        )
        .expect_err("the writer must stay bound to the first Gate-2 resolution");

        assert_eq!(
            fs::read(tmp.path().join("reviewed/a/skill.md")).unwrap(),
            b"old"
        );
        assert_eq!(
            fs::read(tmp.path().join("reviewed/b/skill.md")).unwrap(),
            b"old"
        );
    }

    #[cfg(unix)]
    #[test]
    fn coherence_recovery_rejects_a_gate2_retarget() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let mut config = sample_config();
        config.surface.push(SurfaceEntry {
            path: "reviewed/".to_string(),
            tier: Tier::Reviewed,
        });
        let ward = Ward::new(tmp.path(), config).unwrap();
        fs::create_dir_all(tmp.path().join("reviewed/a")).unwrap();
        fs::create_dir_all(tmp.path().join("reviewed/b")).unwrap();
        fs::write(tmp.path().join("reviewed/a/skill.md"), b"new").unwrap();
        fs::write(tmp.path().join("reviewed/b/skill.md"), b"new").unwrap();
        symlink("reviewed/a", tmp.path().join("lane")).unwrap();
        let edits = vec![FileEdit::new("lane/skill.md", b"new".to_vec())];
        let expected = BTreeMap::from([("lane/skill.md".to_string(), Some(b"old".to_vec()))]);
        let initial = ward.evaluate(&Proposal {
            targets: vec!["lane/skill.md".to_string()],
            authorization: Authorization::unsigned(),
        });
        assert_eq!(initial.decisions[0].resolved, "reviewed/a/skill.md");
        let expected_resolved = BTreeMap::from([(
            "lane/skill.md".to_string(),
            initial.decisions[0].resolved.clone(),
        )]);
        fs::remove_file(tmp.path().join("lane")).unwrap();
        symlink("reviewed/b", tmp.path().join("lane")).unwrap();

        ward.apply_after_coherence_approval(
            &edits,
            &Authorization::unsigned(),
            &expected,
            &expected_resolved,
            ApprovedApplyMode::Recovery,
        )
        .expect_err("recovery must stay bound to its persisted Gate-2 resolution");

        assert_eq!(
            fs::read(tmp.path().join("reviewed/a/skill.md")).unwrap(),
            b"new"
        );
        assert_eq!(
            fs::read(tmp.path().join("reviewed/b/skill.md")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn coherence_approval_preserves_a_concurrent_mutation_while_rolling_back_a_create() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir(tmp.path().join("scratch")).unwrap();
        fs::write(tmp.path().join("scratch/notes.txt"), b"old notes").unwrap();
        let edits = vec![
            FileEdit::new("MEMORY.md", b"new memory".to_vec()),
            FileEdit::new("scratch/notes.txt", b"new notes".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("MEMORY.md".to_string(), None),
            ("scratch/notes.txt".to_string(), Some(b"old notes".to_vec())),
        ]);
        let canonical = tmp.path().canonicalize().unwrap();
        set_conditional_write_actions(
            canonical.join("scratch/notes.txt"),
            vec![
                (canonical.join("MEMORY.md"), b"concurrent memory".to_vec()),
                (
                    canonical.join("scratch/notes.txt"),
                    b"concurrent notes".to_vec(),
                ),
            ],
        );

        let error = ward
            .apply_after_coherence_approval(
                &edits,
                &Authorization::unsigned(),
                &expected,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .expect_err("the second target race must abort the approved unit");

        assert!(
            format!("{error:#}").contains("conditional rollback"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            fs::read(tmp.path().join("MEMORY.md")).unwrap(),
            b"concurrent memory",
            "rollback must not delete bytes written by a concurrent actor"
        );
        assert_eq!(
            fs::read(tmp.path().join("scratch/notes.txt")).unwrap(),
            b"concurrent notes"
        );
    }

    #[test]
    fn coherence_approval_removes_its_owned_create_when_a_later_target_races() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir(tmp.path().join("scratch")).unwrap();
        fs::write(tmp.path().join("scratch/notes.txt"), b"old notes").unwrap();
        let edits = vec![
            FileEdit::new("MEMORY.md", b"new memory".to_vec()),
            FileEdit::new("scratch/notes.txt", b"new notes".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("MEMORY.md".to_string(), None),
            ("scratch/notes.txt".to_string(), Some(b"old notes".to_vec())),
        ]);
        set_conditional_write_hook(
            tmp.path().canonicalize().unwrap().join("scratch/notes.txt"),
            b"concurrent notes".to_vec(),
        );

        let error = ward
            .apply_after_coherence_approval(
                &edits,
                &Authorization::unsigned(),
                &expected,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .expect_err("the later target race must abort the approved unit");

        assert!(
            format!("{error:#}").contains("approved proposal was rolled back"),
            "unexpected error: {error:#}"
        );
        assert!(
            !tmp.path().join("MEMORY.md").exists(),
            "rollback must remove the create owned by this apply attempt"
        );
        assert_eq!(
            fs::read(tmp.path().join("scratch/notes.txt")).unwrap(),
            b"concurrent notes"
        );
        assert_eq!(staging_litter(tmp.path()), Vec::<String>::new());
        assert_eq!(
            staging_litter(&tmp.path().join("scratch")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn coherence_approval_refuses_authorized_protected_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::write(tmp.path().join("SOUL.md"), b"old soul").unwrap();
        let edits = vec![FileEdit::new("SOUL.md", b"new soul".to_vec())];
        let expected = BTreeMap::from([("SOUL.md".to_string(), Some(b"old soul".to_vec()))]);

        let report = ward
            .apply_after_coherence_approval(
                &edits,
                &Authorization::signed_by("SHA256:principal-key"),
                &expected,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .unwrap();

        assert!(report.is_refused());
        assert_eq!(
            report.changes[0].decision.verdict,
            Verdict::AuthorizedProtectedChange
        );
        assert_eq!(fs::read(tmp.path().join("SOUL.md")).unwrap(), b"old soul");
    }

    #[test]
    fn coherence_approval_refuses_a_unit_without_a_reviewed_target() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir(tmp.path().join("scratch")).unwrap();
        fs::write(tmp.path().join("scratch/notes.txt"), b"old notes").unwrap();
        let edits = vec![FileEdit::new("scratch/notes.txt", b"new notes".to_vec())];
        let expected =
            BTreeMap::from([("scratch/notes.txt".to_string(), Some(b"old notes".to_vec()))]);

        let report = ward
            .apply_after_coherence_approval(
                &edits,
                &Authorization::unsigned(),
                &expected,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .unwrap();

        assert!(report.is_refused());
        assert_eq!(
            fs::read(tmp.path().join("scratch/notes.txt")).unwrap(),
            b"old notes"
        );
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
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
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
    fn approved_apply_rejects_same_byte_regular_replacement_at_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir(tmp.path().join("memory")).unwrap();
        fs::write(tmp.path().join("MEMORY.md"), b"old memory").unwrap();
        let logged = tmp.path().join("memory/log.md");
        fs::write(&logged, b"old log").unwrap();
        let edits = vec![
            FileEdit::new("MEMORY.md", b"new memory".to_vec()),
            FileEdit::new("memory/log.md", b"new log".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("MEMORY.md".to_string(), Some(b"old memory".to_vec())),
            ("memory/log.md".to_string(), Some(b"old log".to_vec())),
        ]);
        set_conditional_write_test_actions(
            logged.canonicalize().unwrap(),
            vec![ConditionalWriteAction::ReplaceRegular {
                target: logged.clone(),
                contents: b"old log".to_vec(),
            }],
        );

        let error = ward
            .apply_after_coherence_approval(
                &edits,
                &Authorization::unsigned(),
                &expected,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .expect_err("a same-byte replacement must not become the audited before-image");

        assert!(
            format!("{error:#}").contains("changed during commit"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            approved_apply_error_may_have_committed_write(&error),
            Some(false)
        );
        assert_eq!(
            fs::read(tmp.path().join("MEMORY.md")).unwrap(),
            b"old memory"
        );
        assert_eq!(fs::read(&logged).unwrap(), b"old log");
        assert_eq!(staging_litter(tmp.path()), Vec::<String>::new());
        assert_eq!(
            staging_litter(&tmp.path().join("memory")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn approved_apply_rejects_displaced_inode_mutated_after_audit_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir(tmp.path().join("memory")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let logged = home.join("memory/log.md");
        let alias = home.join("memory/log-alias.md");
        let reviewed = home.join("MEMORY.md");
        fs::write(&logged, b"old log").unwrap();
        fs::hard_link(&logged, &alias).unwrap();
        fs::write(&reviewed, b"old memory").unwrap();
        let edits = vec![
            FileEdit::new("memory/log.md", b"new log".to_vec()),
            FileEdit::new("MEMORY.md", b"new memory".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("memory/log.md".to_string(), Some(b"old log".to_vec())),
            ("MEMORY.md".to_string(), Some(b"old memory".to_vec())),
        ]);
        set_conditional_write_test_actions(
            reviewed,
            vec![ConditionalWriteAction::MutateRegular {
                target: alias.clone(),
                contents: b"concurrent log".to_vec(),
            }],
        );

        let error = ward
            .apply_after_coherence_approval(
                &edits,
                &Authorization::unsigned(),
                &expected,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .expect_err("a mutated displaced inode must not produce a stale successful audit");

        assert!(
            format!("{error:#}").contains("changed before batch finalization"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            approved_apply_error_may_have_committed_write(&error),
            Some(false)
        );
        assert_eq!(fs::read(&logged).unwrap(), b"concurrent log");
        assert_eq!(fs::read(&alias).unwrap(), b"concurrent log");
        assert_eq!(fs::read(home.join("MEMORY.md")).unwrap(), b"old memory");
        assert_eq!(staging_litter(&home), Vec::<String>::new());
        assert_eq!(staging_litter(&home.join("memory")), Vec::<String>::new());
    }

    #[cfg(unix)]
    #[test]
    fn approved_apply_preserves_symlink_replacement_at_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir(tmp.path().join("memory")).unwrap();
        fs::write(tmp.path().join("MEMORY.md"), b"old memory").unwrap();
        let logged = tmp.path().join("memory/log.md");
        let destination = tmp.path().join("elsewhere");
        fs::write(&logged, b"old log").unwrap();
        fs::write(&destination, b"old log").unwrap();
        let edits = vec![
            FileEdit::new("MEMORY.md", b"new memory".to_vec()),
            FileEdit::new("memory/log.md", b"new log".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("MEMORY.md".to_string(), Some(b"old memory".to_vec())),
            ("memory/log.md".to_string(), Some(b"old log".to_vec())),
        ]);
        set_conditional_write_test_actions(
            logged.canonicalize().unwrap(),
            vec![ConditionalWriteAction::ReplaceSymlink {
                target: logged.clone(),
                destination: destination.clone(),
            }],
        );

        ward.apply_after_coherence_approval(
            &edits,
            &Authorization::unsigned(),
            &expected,
            &resolved_as_target(&edits),
            ApprovedApplyMode::Initial,
        )
        .expect_err("an approved commit must not follow a replacement symlink");

        assert_eq!(
            fs::read(tmp.path().join("MEMORY.md")).unwrap(),
            b"old memory"
        );
        assert!(logged.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&logged).unwrap(), destination);
        assert_eq!(fs::read(&destination).unwrap(), b"old log");
    }

    #[test]
    fn approved_apply_preserves_directory_replacement_at_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir(tmp.path().join("memory")).unwrap();
        fs::write(tmp.path().join("MEMORY.md"), b"old memory").unwrap();
        let logged = tmp.path().join("memory/log.md");
        fs::write(&logged, b"old log").unwrap();
        let edits = vec![
            FileEdit::new("MEMORY.md", b"new memory".to_vec()),
            FileEdit::new("memory/log.md", b"new log".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("MEMORY.md".to_string(), Some(b"old memory".to_vec())),
            ("memory/log.md".to_string(), Some(b"old log".to_vec())),
        ]);
        set_conditional_write_test_actions(
            logged.canonicalize().unwrap(),
            vec![ConditionalWriteAction::ReplaceDirectory {
                target: logged.clone(),
            }],
        );

        ward.apply_after_coherence_approval(
            &edits,
            &Authorization::unsigned(),
            &expected,
            &resolved_as_target(&edits),
            ApprovedApplyMode::Initial,
        )
        .expect_err("an approved commit must preserve a replacement directory");

        assert_eq!(
            fs::read(tmp.path().join("MEMORY.md")).unwrap(),
            b"old memory"
        );
        assert!(logged.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn approved_apply_rejects_fifo_replacement_without_blocking_or_reading() {
        use std::os::unix::fs::FileTypeExt;

        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("memory")).unwrap();
        fs::write(tmp.path().join("MEMORY.md"), b"old memory").unwrap();
        let logged = tmp.path().canonicalize().unwrap().join("memory/log.md");
        fs::write(&logged, b"old log").unwrap();
        set_conditional_write_test_actions(
            logged.clone(),
            vec![ConditionalWriteAction::ReplaceFifo {
                target: logged.clone(),
            }],
        );

        let ward = ward_in(tmp.path());
        let edits = vec![
            FileEdit::new("MEMORY.md", b"new memory".to_vec()),
            FileEdit::new("memory/log.md", b"new log".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("MEMORY.md".to_string(), Some(b"old memory".to_vec())),
            ("memory/log.md".to_string(), Some(b"old log".to_vec())),
        ]);
        let fifo = logged.clone();
        let result = run_fifo_operation_with_watchdog(
            &fifo,
            "approved apply against a concurrent FIFO replacement",
            move || {
                ward.apply_after_coherence_approval(
                    &edits,
                    &Authorization::unsigned(),
                    &expected,
                    &resolved_as_target(&edits),
                    ApprovedApplyMode::Initial,
                )
            },
        );

        result.expect_err("an approved commit must reject a replacement FIFO");
        assert_eq!(
            fs::read(tmp.path().join("MEMORY.md")).unwrap(),
            b"old memory"
        );
        assert!(logged.symlink_metadata().unwrap().file_type().is_fifo());
    }

    #[cfg(unix)]
    #[test]
    fn approved_create_rollback_does_not_follow_symlink_replacement() {
        use std::os::unix::ffi::OsStrExt;

        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("memory")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let reviewed = home.join("MEMORY.md");
        let blocked = home.join("memory/log.md");
        let fifo = home.join("symlink-destination");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `fifo_c` is a live, NUL-terminated pathname.
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        set_conditional_write_test_actions(
            blocked.clone(),
            vec![
                ConditionalWriteAction::ReplaceSymlink {
                    target: reviewed.clone(),
                    destination: fifo.clone(),
                },
                ConditionalWriteAction::ReplaceRegular {
                    target: blocked.clone(),
                    contents: b"concurrent blocker".to_vec(),
                },
            ],
        );

        let ward = ward_in(&home);
        let edits = vec![
            FileEdit::new("MEMORY.md", b"new memory".to_vec()),
            FileEdit::new("memory/log.md", b"new log".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("MEMORY.md".to_string(), None),
            ("memory/log.md".to_string(), None),
        ]);
        let result = ward.apply_after_coherence_approval(
            &edits,
            &Authorization::unsigned(),
            &expected,
            &resolved_as_target(&edits),
            ApprovedApplyMode::Initial,
        );

        let error = result.expect_err("concurrent replacement must make rollback ambiguous");
        assert_eq!(
            approved_apply_error_may_have_committed_write(&error),
            Some(true)
        );
        assert!(reviewed
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&reviewed).unwrap(), fifo);
        assert_eq!(fs::read(&blocked).unwrap(), b"concurrent blocker");
    }

    #[cfg(unix)]
    #[test]
    fn approved_create_rollback_does_not_read_fifo_replacement() {
        use std::os::unix::fs::FileTypeExt;

        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("memory")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let reviewed = home.join("MEMORY.md");
        let blocked = home.join("memory/log.md");
        set_conditional_write_test_actions(
            blocked.clone(),
            vec![
                ConditionalWriteAction::ReplaceFifo {
                    target: reviewed.clone(),
                },
                ConditionalWriteAction::ReplaceRegular {
                    target: blocked.clone(),
                    contents: b"concurrent blocker".to_vec(),
                },
            ],
        );

        let ward = ward_in(&home);
        let edits = vec![
            FileEdit::new("MEMORY.md", b"new memory".to_vec()),
            FileEdit::new("memory/log.md", b"new log".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("MEMORY.md".to_string(), None),
            ("memory/log.md".to_string(), None),
        ]);
        let result = ward.apply_after_coherence_approval(
            &edits,
            &Authorization::unsigned(),
            &expected,
            &resolved_as_target(&edits),
            ApprovedApplyMode::Initial,
        );

        let error = result.expect_err("concurrent replacement must make rollback ambiguous");
        assert_eq!(
            approved_apply_error_may_have_committed_write(&error),
            Some(true)
        );
        assert!(reviewed.symlink_metadata().unwrap().file_type().is_fifo());
        assert_eq!(fs::read(&blocked).unwrap(), b"concurrent blocker");
    }

    #[test]
    fn approved_success_preserves_replaced_backup_and_reports_committed_cleanup_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        let home = tmp.path().canonicalize().unwrap();
        let reviewed = home.join("MEMORY.md");
        fs::write(&reviewed, b"old memory").unwrap();
        set_cleanup_artifact_replacement(
            reviewed.clone(),
            b"concurrent backup replacement".to_vec(),
        );
        let edits = vec![FileEdit::new("MEMORY.md", b"new memory".to_vec())];
        let expected = BTreeMap::from([("MEMORY.md".to_string(), Some(b"old memory".to_vec()))]);

        let error = ward
            .apply_after_coherence_approval(
                &edits,
                &Authorization::unsigned(),
                &expected,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .expect_err("unowned backup cleanup must not report success");

        assert_eq!(
            approved_apply_error_may_have_committed_write(&error),
            Some(true)
        );
        assert_eq!(fs::read(&reviewed).unwrap(), b"new memory");
        assert_eq!(
            staging_artifact_contents(&home),
            vec![b"concurrent backup replacement".to_vec()]
        );
    }

    #[test]
    fn approved_proven_rollback_preserves_replaced_staging_and_reports_cleanup_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir(tmp.path().join("memory")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let reviewed = home.join("MEMORY.md");
        let blocked = home.join("memory/log.md");
        fs::write(&reviewed, b"old memory").unwrap();
        set_conditional_write_hook(blocked.clone(), b"concurrent blocker".to_vec());
        set_cleanup_artifact_replacement(
            reviewed.clone(),
            b"concurrent staging replacement".to_vec(),
        );
        let edits = vec![
            FileEdit::new("MEMORY.md", b"new memory".to_vec()),
            FileEdit::new("memory/log.md", b"new log".to_vec()),
        ];
        let expected = BTreeMap::from([
            ("MEMORY.md".to_string(), Some(b"old memory".to_vec())),
            ("memory/log.md".to_string(), None),
        ]);

        let error = ward
            .apply_after_coherence_approval(
                &edits,
                &Authorization::unsigned(),
                &expected,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .expect_err("the second target must force a proven rollback");

        assert!(
            format!("{error:#}").contains("cleanup failed"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            approved_apply_error_may_have_committed_write(&error),
            Some(false)
        );
        assert_eq!(fs::read(&reviewed).unwrap(), b"old memory");
        assert_eq!(fs::read(&blocked).unwrap(), b"concurrent blocker");
        assert_eq!(
            staging_artifact_contents(&home),
            vec![b"concurrent staging replacement".to_vec()]
        );
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
                &resolved_as_target(&edits),
                ApprovedApplyMode::Recovery,
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
                &resolved_as_target(&edits),
                ApprovedApplyMode::Recovery,
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
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
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

    #[cfg(any(unix, windows))]
    #[test]
    fn file_identity_matches_hard_links_and_distinguishes_siblings() {
        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join("original");
        let alias = tmp.path().join("alias");
        let sibling = tmp.path().join("sibling");
        fs::write(&original, b"same bytes").unwrap();
        fs::hard_link(&original, &alias).unwrap();
        fs::write(&sibling, b"same bytes").unwrap();

        assert!(same_file_identity(&original, &alias).unwrap());
        assert!(!same_file_identity(&original, &sibling).unwrap());
    }

    #[test]
    fn windows_file_identity_rejects_unusable_sentinels() {
        assert!(validate_windows_file_identity(7, [0; 16]).is_err());
        assert!(validate_windows_file_identity(7, [u8::MAX; 16]).is_err());

        let mut usable = [0; 16];
        usable[15] = 1;
        assert_eq!(
            validate_windows_file_identity(7, usable).unwrap(),
            (7, usable)
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
        assert!(tmp.path().join("scratch/notes.txt").is_dir());
    }

    #[test]
    fn direct_apply_refuses_target_replaced_immediately_before_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        let target = tmp.path().canonicalize().unwrap().join("memory/log.md");
        fs::write(&target, b"reviewed before-image").unwrap();
        set_conditional_write_hook(target.clone(), b"concurrent replacement".to_vec());

        let error = ward
            .apply(
                &[FileEdit::new("memory/log.md", b"ward contents".to_vec())],
                &Authorization::unsigned(),
            )
            .expect_err("a concurrent final-component replacement must win");

        assert!(
            format!("{error:#}").contains("changed during commit"),
            "unexpected error: {error:#}"
        );
        assert_eq!(fs::read(&target).unwrap(), b"concurrent replacement");
        assert_eq!(
            staging_litter(&tmp.path().join("memory")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn direct_apply_fails_closed_on_same_byte_final_component_replacement() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        let target = tmp.path().canonicalize().unwrap().join("memory/log.md");
        fs::write(&target, b"same bytes").unwrap();
        set_conditional_write_hook(target.clone(), b"same bytes".to_vec());

        let error = ward
            .apply(
                &[FileEdit::new("memory/log.md", b"ward contents".to_vec())],
                &Authorization::unsigned(),
            )
            .expect_err("byte equality must not hide a final-component replacement");

        assert!(
            format!("{error:#}").contains("changed during commit"),
            "unexpected error: {error:#}"
        );
        assert_eq!(fs::read(&target).unwrap(), b"same bytes");
        assert_eq!(
            staging_litter(&tmp.path().join("memory")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn direct_apply_rejects_displaced_inode_mutated_after_audit_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let logged = home.join("memory/log.md");
        let alias = home.join("memory/log-alias.md");
        let free = home.join("scratch/output.txt");
        fs::write(&logged, b"old log").unwrap();
        fs::hard_link(&logged, &alias).unwrap();
        set_conditional_write_test_actions(
            free,
            vec![ConditionalWriteAction::MutateRegular {
                target: alias.clone(),
                contents: b"concurrent log".to_vec(),
            }],
        );

        let error = ward
            .apply(
                &[
                    FileEdit::new("memory/log.md", b"new log".to_vec()),
                    FileEdit::new("scratch/output.txt", b"new output".to_vec()),
                ],
                &Authorization::unsigned(),
            )
            .expect_err("a mutated displaced inode must not produce a stale successful audit");

        assert!(
            format!("{error:#}").contains("changed before batch finalization"),
            "unexpected error: {error:#}"
        );
        assert!(
            matches!(
                direct_apply_failure(&error),
                Some(DirectApplyFailure::RolledBack)
            ),
            "unexpected direct failure: {error:#}"
        );
        assert_eq!(fs::read(&logged).unwrap(), b"concurrent log");
        assert_eq!(fs::read(&alias).unwrap(), b"concurrent log");
        assert!(!home.join("scratch/output.txt").exists());
        assert_eq!(staging_litter(&home.join("memory")), Vec::<String>::new());
        assert_eq!(staging_litter(&home.join("scratch")), Vec::<String>::new());
    }

    #[cfg(unix)]
    #[test]
    fn direct_apply_preserves_symlink_replacement_immediately_before_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        let target = tmp.path().canonicalize().unwrap().join("memory/log.md");
        let destination = tmp.path().canonicalize().unwrap().join("elsewhere");
        fs::write(&target, b"reviewed before-image").unwrap();
        set_conditional_write_test_actions(
            target.clone(),
            vec![ConditionalWriteAction::ReplaceSymlink {
                target: target.clone(),
                destination: destination.clone(),
            }],
        );

        let error = ward
            .apply(
                &[FileEdit::new("memory/log.md", b"ward contents".to_vec())],
                &Authorization::unsigned(),
            )
            .expect_err("a concurrent symlink replacement must not be overwritten");

        assert!(
            matches!(
                direct_apply_failure(&error),
                Some(DirectApplyFailure::Ambiguous { .. })
            ),
            "unexpected error: {error:#}"
        );
        assert!(target.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&target).unwrap(), destination);
    }

    #[test]
    fn direct_apply_preserves_directory_replacement_immediately_before_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        let target = tmp.path().canonicalize().unwrap().join("memory/log.md");
        fs::write(&target, b"reviewed before-image").unwrap();
        set_conditional_write_test_actions(
            target.clone(),
            vec![ConditionalWriteAction::ReplaceDirectory {
                target: target.clone(),
            }],
        );

        ward.apply(
            &[FileEdit::new("memory/log.md", b"ward contents".to_vec())],
            &Authorization::unsigned(),
        )
        .expect_err("a concurrent directory replacement must not be overwritten");

        assert!(target.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn direct_apply_preserves_fifo_replacement_immediately_before_commit() {
        use std::os::unix::fs::FileTypeExt;

        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        let target = tmp.path().canonicalize().unwrap().join("memory/log.md");
        fs::write(&target, b"reviewed before-image").unwrap();
        set_conditional_write_test_actions(
            target.clone(),
            vec![ConditionalWriteAction::ReplaceFifo {
                target: target.clone(),
            }],
        );

        let fifo = target.clone();
        run_fifo_operation_with_watchdog(
            &fifo,
            "direct apply against a concurrent FIFO replacement",
            move || {
                ward.apply(
                    &[FileEdit::new("memory/log.md", b"ward contents".to_vec())],
                    &Authorization::unsigned(),
                )
            },
        )
        .expect_err("a concurrent FIFO replacement must not be overwritten");

        assert!(target.symlink_metadata().unwrap().file_type().is_fifo());
    }

    #[test]
    fn direct_apply_never_overwrites_a_concurrent_create_as_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        let target = tmp.path().canonicalize().unwrap().join("memory/log.md");
        set_conditional_write_hook(target.clone(), b"concurrent create".to_vec());

        let error = ward
            .apply(
                &[FileEdit::new("memory/log.md", b"ward contents".to_vec())],
                &Authorization::unsigned(),
            )
            .expect_err("a concurrent create must not be overwritten");

        assert!(
            format!("{error:#}").contains("appeared during commit"),
            "unexpected error: {error:#}"
        );
        assert_eq!(fs::read(&target).unwrap(), b"concurrent create");
        assert_eq!(
            staging_litter(&tmp.path().join("memory")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn direct_apply_preserves_concurrent_replacement_of_owned_creation() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let created = home.join("memory/log.md");
        let blocked = home.join("scratch/blocked.txt");
        set_conditional_write_actions(
            blocked.clone(),
            vec![
                (created.clone(), b"concurrent replacement".to_vec()),
                (blocked.clone(), b"concurrent create".to_vec()),
            ],
        );

        let error = ward
            .apply(
                &[
                    FileEdit::new("memory/log.md", b"ward contents".to_vec()),
                    FileEdit::new("scratch/blocked.txt", b"free contents".to_vec()),
                ],
                &Authorization::unsigned(),
            )
            .expect_err("a replacement of an owned creation must survive rollback");

        let DirectApplyFailure::Ambiguous { targets } =
            direct_apply_failure(&error).expect("typed direct failure")
        else {
            panic!("expected ambiguous direct failure: {error:#}");
        };
        assert_eq!(targets, &vec!["memory/log.md".to_string()]);
        assert_eq!(fs::read(created).unwrap(), b"concurrent replacement");
        assert_eq!(fs::read(blocked).unwrap(), b"concurrent create");
    }

    #[test]
    fn direct_apply_rolls_back_a_staging_inode_moved_to_target_before_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        let target = tmp.path().canonicalize().unwrap().join("memory/log.md");
        set_early_staging_move(target.clone());

        let error = ward
            .apply(
                &[FileEdit::new("memory/log.md", b"ward contents".to_vec())],
                &Authorization::unsigned(),
            )
            .expect_err("an early staging move must be detected and rolled back");

        assert!(
            matches!(
                direct_apply_failure(&error),
                Some(DirectApplyFailure::RolledBack)
            ),
            "unexpected error: {error:#}"
        );
        assert!(!target.exists());
        assert_eq!(
            staging_litter(&tmp.path().join("memory")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn direct_apply_rolls_back_logged_write_when_later_free_write_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        fs::write(tmp.path().join("memory/log.md"), b"old log").unwrap();
        let blocked = tmp
            .path()
            .canonicalize()
            .unwrap()
            .join("scratch/blocked.txt");
        set_conditional_write_hook(blocked.clone(), b"concurrent create".to_vec());
        let edits = vec![
            FileEdit::new("memory/log.md", b"new log".to_vec()),
            FileEdit::new("scratch/blocked.txt", b"free contents".to_vec()),
        ];

        let error = ward
            .apply(&edits, &Authorization::unsigned())
            .expect_err("the direct batch must fail as a unit");

        assert!(
            format!("{error:#}").contains("appeared during commit"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            fs::read(tmp.path().join("memory/log.md")).unwrap(),
            b"old log"
        );
        assert_eq!(fs::read(blocked).unwrap(), b"concurrent create");
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
    fn direct_apply_cleanup_failure_preserves_applied_audit_report() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        let target = tmp.path().canonicalize().unwrap().join("memory/log.md");
        fs::write(&target, b"before cleanup").unwrap();
        set_direct_cleanup_failure(target.clone());

        let error = ward
            .apply(
                &[FileEdit::new("memory/log.md", b"after cleanup".to_vec())],
                &Authorization::unsigned(),
            )
            .expect_err("cleanup failure must preserve the applied report");
        let DirectApplyFailure::Applied(report) =
            direct_apply_failure(&error).expect("typed direct failure")
        else {
            panic!("expected applied cleanup failure: {error:#}");
        };

        assert_eq!(fs::read(&target).unwrap(), b"after cleanup");
        assert_eq!(report.changes.len(), 1);
        let audit = report.changes[0].audit.as_ref().expect("logged audit");
        assert_eq!(
            audit.prev_sha256.as_deref(),
            Some(sha256_hex(b"before cleanup").as_str())
        );
        assert_eq!(audit.next_sha256, sha256_hex(b"after cleanup"));
    }

    #[test]
    fn direct_apply_classifies_unproven_rollback_as_ambiguous() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let logged = home.join("memory/log.md");
        let free = home.join("scratch/output.txt");
        fs::write(&logged, b"before").unwrap();
        set_conditional_write_hook(free.clone(), b"concurrent create".to_vec());
        set_conditional_rollback_sabotage(free.clone(), logged.clone());

        let error = ward
            .apply(
                &[
                    FileEdit::new("memory/log.md", b"after".to_vec()),
                    FileEdit::new("scratch/output.txt", b"free".to_vec()),
                ],
                &Authorization::unsigned(),
            )
            .expect_err("unproven rollback must be classified as ambiguous");
        let DirectApplyFailure::Ambiguous { targets } =
            direct_apply_failure(&error).expect("typed direct failure")
        else {
            panic!("expected ambiguous direct failure: {error:#}");
        };

        assert_eq!(targets, &vec!["memory/log.md".to_string()]);
        assert_eq!(fs::read(logged).unwrap(), b"after");
        assert_eq!(fs::read(free).unwrap(), b"concurrent create");
    }

    #[test]
    fn direct_apply_preserves_same_byte_concurrent_replacement_during_rollback() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let logged = home.join("memory/log.md");
        let free = home.join("scratch/output.txt");
        fs::write(&logged, b"before").unwrap();
        set_conditional_write_hook(free.clone(), b"concurrent create".to_vec());
        set_conditional_atomic_replacement(free.clone(), logged.clone(), b"after".to_vec());

        let error = ward
            .apply(
                &[
                    FileEdit::new("memory/log.md", b"after".to_vec()),
                    FileEdit::new("scratch/output.txt", b"free".to_vec()),
                ],
                &Authorization::unsigned(),
            )
            .expect_err("a byte-equal concurrent inode must not be deleted during rollback");

        assert!(
            matches!(
                direct_apply_failure(&error),
                Some(DirectApplyFailure::Ambiguous { .. })
            ),
            "unexpected error: {error:#}"
        );
        assert_eq!(fs::read(logged).unwrap(), b"after");
        assert_eq!(fs::read(free).unwrap(), b"concurrent create");
    }

    #[test]
    fn direct_apply_preserves_bytes_mutated_through_hard_link_during_rollback() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let logged = home.join("memory/log.md");
        let alias = home.join("memory/concurrent-alias.md");
        let blocked = home.join("scratch/blocked.txt");
        fs::write(&logged, b"before").unwrap();
        set_conditional_write_test_actions(
            blocked.clone(),
            vec![
                ConditionalWriteAction::MutateThroughHardLink {
                    target: logged.clone(),
                    alias: alias.clone(),
                    contents: b"concurrent bytes".to_vec(),
                },
                ConditionalWriteAction::ReplaceRegular {
                    target: blocked.clone(),
                    contents: b"concurrent create".to_vec(),
                },
            ],
        );

        let error = ward
            .apply(
                &[
                    FileEdit::new("memory/log.md", b"ward contents".to_vec()),
                    FileEdit::new("scratch/blocked.txt", b"free contents".to_vec()),
                ],
                &Authorization::unsigned(),
            )
            .expect_err("unrecognized concurrent bytes must survive an unproven rollback");

        let DirectApplyFailure::Ambiguous { targets } =
            direct_apply_failure(&error).expect("typed direct failure")
        else {
            panic!("expected ambiguous direct failure: {error:#}");
        };
        assert_eq!(targets, &vec!["memory/log.md".to_string()]);
        assert_eq!(fs::read(&logged).unwrap(), b"concurrent bytes");
        assert_eq!(fs::read(&alias).unwrap(), b"concurrent bytes");
        assert_eq!(fs::read(blocked).unwrap(), b"concurrent create");
    }

    #[test]
    fn direct_apply_cleans_staging_after_success() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        fs::write(tmp.path().join("memory/log.md"), b"before").unwrap();

        let report = ward
            .apply(
                &[
                    FileEdit::new("memory/log.md", b"after".to_vec()),
                    FileEdit::new("scratch/output.txt", b"free".to_vec()),
                ],
                &Authorization::unsigned(),
            )
            .unwrap();

        assert!(report.is_applied());
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
    fn direct_success_preserves_replaced_staging_and_reports_applied_cleanup_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let target = home.join("scratch/output.txt");
        set_cleanup_artifact_replacement(
            target.clone(),
            b"concurrent staging replacement".to_vec(),
        );

        let error = ward
            .apply(
                &[FileEdit::new("scratch/output.txt", b"new output".to_vec())],
                &Authorization::unsigned(),
            )
            .expect_err("unowned staging cleanup must not report success");

        assert!(
            matches!(
                direct_apply_failure(&error),
                Some(DirectApplyFailure::Applied(_))
            ),
            "unexpected error: {error:#}"
        );
        assert_eq!(fs::read(&target).unwrap(), b"new output");
        assert_eq!(
            staging_artifact_contents(&home.join("scratch")),
            vec![b"concurrent staging replacement".to_vec()]
        );
    }

    #[test]
    fn direct_success_preserves_cleanup_capture_replaced_after_identity_verification() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let target = home.join("scratch/output.txt");
        set_verified_cleanup_capture_replacement(
            home.join("scratch"),
            b"post-verification replacement".to_vec(),
        );

        let error = ward
            .apply(
                &[FileEdit::new("scratch/output.txt", b"new output".to_vec())],
                &Authorization::unsigned(),
            )
            .expect_err("post-verification replacement must survive cleanup");

        assert!(
            matches!(
                direct_apply_failure(&error),
                Some(DirectApplyFailure::Applied(_))
            ),
            "unexpected error: {error:#}"
        );
        assert_eq!(fs::read(&target).unwrap(), b"new output");
        assert_eq!(
            staging_artifact_contents(&home.join("scratch")),
            vec![b"post-verification replacement".to_vec()]
        );
    }

    #[test]
    fn direct_apply_does_not_report_successful_rollback_after_backup_replacement() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let logged = home.join("memory/log.md");
        let free = home.join("scratch/output.txt");
        fs::write(&logged, b"before").unwrap();
        set_conditional_write_hook(free.clone(), b"concurrent create".to_vec());
        set_conditional_rollback_backup_replacement(
            free.clone(),
            logged.clone(),
            b"replacement backup".to_vec(),
        );

        let error = ward
            .apply(
                &[
                    FileEdit::new("memory/log.md", b"after".to_vec()),
                    FileEdit::new("scratch/output.txt", b"free".to_vec()),
                ],
                &Authorization::unsigned(),
            )
            .expect_err("a replaced backup must make rollback ambiguous");

        assert!(
            matches!(
                direct_apply_failure(&error),
                Some(DirectApplyFailure::Ambiguous { .. })
            ),
            "unexpected error: {error:#}"
        );
        assert_eq!(fs::read(logged).unwrap(), b"replacement backup");
        assert_eq!(fs::read(free).unwrap(), b"concurrent create");
    }

    #[test]
    fn direct_apply_reports_rollback_cleanup_failure_separately() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let logged = home.join("memory/log.md");
        let free = home.join("scratch/output.txt");
        fs::write(&logged, b"before").unwrap();
        set_conditional_write_hook(free.clone(), b"concurrent create".to_vec());
        set_direct_cleanup_failure(logged.clone());

        let error = ward
            .apply(
                &[
                    FileEdit::new("memory/log.md", b"after".to_vec()),
                    FileEdit::new("scratch/output.txt", b"free".to_vec()),
                ],
                &Authorization::unsigned(),
            )
            .expect_err("rollback cleanup failure must not look fully clean");

        assert!(
            matches!(
                direct_apply_failure(&error),
                Some(DirectApplyFailure::RolledBackCleanupFailed { .. })
            ),
            "unexpected error: {error:#}"
        );
        assert_eq!(fs::read(logged).unwrap(), b"before");
        assert_eq!(fs::read(free).unwrap(), b"concurrent create");
    }

    #[test]
    fn direct_staging_cleanup_failure_is_not_reported_as_a_clean_rollback() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let target = home.join("scratch/output.txt");
        set_staging_write_cleanup_failure(
            target.clone(),
            b"concurrent staging replacement".to_vec(),
        );

        let error = ward
            .apply(
                &[FileEdit::new("scratch/output.txt", b"new output".to_vec())],
                &Authorization::unsigned(),
            )
            .expect_err("failed staging cleanup must not look fully rolled back");

        assert!(
            matches!(
                direct_apply_failure(&error),
                Some(DirectApplyFailure::RolledBackCleanupFailed { targets })
                    if targets == &vec!["scratch/output.txt".to_string()]
            ),
            "unexpected error: {error:#}"
        );
        assert!(!target.exists());
        assert_eq!(
            staging_artifact_contents(&home.join("scratch")),
            vec![b"concurrent staging replacement".to_vec()]
        );
    }

    #[test]
    fn direct_staging_failure_reports_every_unclean_artifact_target() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let first = home.join("scratch/first.txt");
        let second = home.join("scratch/second.txt");
        set_direct_cleanup_failure(first);
        set_staging_write_cleanup_failure(second, b"concurrent staging replacement".to_vec());

        let error = ward
            .apply(
                &[
                    FileEdit::new("scratch/first.txt", b"first".to_vec()),
                    FileEdit::new("scratch/second.txt", b"second".to_vec()),
                ],
                &Authorization::unsigned(),
            )
            .expect_err("every incomplete cleanup must be reported");

        let Some(DirectApplyFailure::RolledBackCleanupFailed { targets }) =
            direct_apply_failure(&error)
        else {
            panic!("unexpected error: {error:#}");
        };
        assert_eq!(
            targets,
            &vec![
                "scratch/first.txt".to_string(),
                "scratch/second.txt".to_string()
            ]
        );
    }

    #[test]
    fn direct_proven_rollback_preserves_replaced_artifact_and_reports_cleanup_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let ward = ward_in(tmp.path());
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let logged = home.join("memory/log.md");
        let free = home.join("scratch/output.txt");
        fs::write(&logged, b"before").unwrap();
        set_conditional_write_hook(free.clone(), b"concurrent create".to_vec());
        set_cleanup_artifact_replacement(
            logged.clone(),
            b"concurrent rollback replacement".to_vec(),
        );

        let error = ward
            .apply(
                &[
                    FileEdit::new("memory/log.md", b"after".to_vec()),
                    FileEdit::new("scratch/output.txt", b"free".to_vec()),
                ],
                &Authorization::unsigned(),
            )
            .expect_err("the second target must force a proven rollback");

        assert!(
            matches!(
                direct_apply_failure(&error),
                Some(DirectApplyFailure::RolledBackCleanupFailed { .. })
            ),
            "unexpected error: {error:#}"
        );
        assert_eq!(fs::read(&logged).unwrap(), b"before");
        assert_eq!(fs::read(&free).unwrap(), b"concurrent create");
        assert_eq!(
            staging_artifact_contents(&home.join("memory")),
            vec![b"concurrent rollback replacement".to_vec()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn opening_a_fifo_target_fails_without_blocking() {
        use std::os::unix::ffi::OsStrExt;

        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("target");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `fifo_c` is a live, NUL-terminated pathname.
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o000) }, 0);

        let fifo_entry = anchored_test_entry(&fifo).unwrap();
        let error = run_fifo_operation_with_watchdog(
            &fifo,
            "opening a FIFO as a regular Ward target",
            move || match open_regular_file_without_following_links(&fifo_entry) {
                Err(error) => error,
                Ok(_) => panic!("FIFO metadata must be rejected before opening it"),
            },
        );
        assert!(
            format!("{error:#}").contains("not a regular file"),
            "unexpected error: {error:#}"
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
    fn direct_batch_fails_closed_when_parent_swapped_for_symlink() {
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
        let anchored_home = AnchoredHome::open(&canonical_home).unwrap();
        let err = write_direct_batch(
            &anchored_home,
            &[FileEdit {
                target: decision.target.clone(),
                new_contents: b"leak".to_vec(),
            }],
            vec![decision],
        )
        .unwrap_err();

        assert!(err.to_string().contains("not a real directory"));
        // Fail-closed with zero side effects outside the home.
        assert!(!outside.join("notes").exists());
        assert!(!outside.join("log.md").exists());
        assert_eq!(staging_litter(&outside), Vec::<String>::new());
    }

    #[test]
    fn direct_write_stays_bound_to_parent_opened_before_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let home = home.canonicalize().unwrap();
        let parent = home.join("scratch/nested");
        let moved_parent = home.join("scratch/nested-before-swap");
        fs::create_dir_all(&parent).unwrap();

        let target = parent.join("output.txt");
        set_prepared_parent_swap(target.clone(), moved_parent.clone());

        let result = ward_in(&home).apply(
            &[FileEdit::new("scratch/nested/output.txt", b"safe".to_vec())],
            &Authorization::unsigned(),
        );

        #[cfg(not(windows))]
        {
            let report = result.unwrap();
            assert_eq!(report.changes.len(), 1);
            assert!(!target.exists());
            assert_eq!(fs::read(moved_parent.join("output.txt")).unwrap(), b"safe");
            assert_eq!(staging_litter(&parent), Vec::<String>::new());
        }
        #[cfg(windows)]
        {
            result.expect_err("the retained Windows parent handle must block its rename");
            assert!(!target.exists());
            assert!(!moved_parent.exists());
            assert_eq!(staging_litter(&parent), Vec::<String>::new());
        }
    }

    #[test]
    fn approved_write_stays_bound_to_parent_opened_before_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let home = home.canonicalize().unwrap();
        let parent = home.join("memory/nested");
        let moved_parent = home.join("memory/nested-before-swap");
        fs::create_dir_all(&parent).unwrap();
        let target = parent.join("log.md");
        fs::write(&target, b"before").unwrap();
        set_prepared_parent_swap(target.clone(), moved_parent.clone());

        let edit = FileEdit::new("memory/nested/log.md", b"after".to_vec());
        let decision = Decision {
            target: edit.target.clone(),
            resolved: edit.target.clone(),
            tier: Tier::Logged,
            verdict: Verdict::Allow,
        };
        let anchored_home = AnchoredHome::open(&home).unwrap();
        let result = write_atomically_if_unchanged(
            &anchored_home,
            std::slice::from_ref(&edit),
            vec![decision],
            &BTreeMap::from([(edit.target.clone(), Some(b"before".to_vec()))]),
            ApprovedApplyMode::Initial,
        );

        #[cfg(not(windows))]
        {
            let changes = result.unwrap();
            assert_eq!(changes.len(), 1);
            let expected_before_digest = sha256_hex(b"before");
            assert_eq!(
                changes[0]
                    .audit
                    .as_ref()
                    .and_then(|audit| audit.prev_sha256.as_deref()),
                Some(expected_before_digest.as_str())
            );
            assert_eq!(fs::read(moved_parent.join("log.md")).unwrap(), b"after");
            assert!(!target.exists());
            assert_eq!(staging_litter(&parent), Vec::<String>::new());
        }
        #[cfg(windows)]
        {
            result.expect_err("the retained Windows parent handle must block its rename");
            assert_eq!(fs::read(&target).unwrap(), b"before");
            assert!(!moved_parent.exists());
            assert_eq!(staging_litter(&parent), Vec::<String>::new());
        }
    }

    #[cfg(unix)]
    #[test]
    fn direct_write_does_not_follow_parent_symlink_installed_after_open() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let home = home.canonicalize().unwrap();
        let outside = outside.canonicalize().unwrap();
        let parent = home.join("scratch/nested");
        let moved_parent = home.join("scratch/nested-before-swap");
        fs::create_dir_all(&parent).unwrap();

        let target = parent.join("output.txt");
        set_prepared_parent_symlink_swap(target, moved_parent.clone(), outside.clone());

        let report = ward_in(&home)
            .apply(
                &[FileEdit::new("scratch/nested/output.txt", b"safe".to_vec())],
                &Authorization::unsigned(),
            )
            .unwrap();

        assert_eq!(report.changes.len(), 1);
        assert_eq!(fs::read(moved_parent.join("output.txt")).unwrap(), b"safe");
        assert!(!outside.join("output.txt").exists());
        assert_eq!(staging_litter(&outside), Vec::<String>::new());
    }

    #[cfg(unix)]
    #[test]
    fn direct_write_stays_bound_to_home_evaluated_before_root_swap() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let moved_home = tmp.path().join("home-before-swap");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(home.join("scratch")).unwrap();
        fs::create_dir_all(outside.join("scratch")).unwrap();
        let home = home.canonicalize().unwrap();
        let outside = outside.canonicalize().unwrap();
        set_evaluated_home_swap(home.clone(), moved_home.clone(), outside.clone());

        let error = ward_in(&home)
            .apply(
                &[FileEdit::new("scratch/output.txt", b"safe".to_vec())],
                &Authorization::unsigned(),
            )
            .unwrap_err();

        assert!(error.to_string().contains("changed during Gate 2"));
        assert!(!moved_home.join("scratch/output.txt").exists());
        assert!(!outside.join("scratch/output.txt").exists());
        assert_eq!(
            staging_litter(&outside.join("scratch")),
            Vec::<String>::new()
        );
    }

    #[cfg(unix)]
    #[test]
    fn direct_write_rejects_retargeted_symlink_home_after_gate2() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join("original");
        let outside = tmp.path().join("outside");
        let configured = tmp.path().join("workspace");
        fs::create_dir_all(original.join("scratch")).unwrap();
        fs::create_dir_all(outside.join("scratch")).unwrap();
        symlink(&original, &configured).unwrap();
        set_evaluated_home_symlink_swap(configured.clone(), outside.clone());

        let error = ward_in(&configured)
            .apply(
                &[FileEdit::new("scratch/output.txt", b"safe".to_vec())],
                &Authorization::unsigned(),
            )
            .unwrap_err();

        assert!(error.to_string().contains("changed during Gate 2"));
        assert!(!original.join("scratch/output.txt").exists());
        assert!(!outside.join("scratch/output.txt").exists());
        assert_eq!(
            staging_litter(&outside.join("scratch")),
            Vec::<String>::new()
        );
    }

    #[cfg(unix)]
    #[test]
    fn approved_write_stays_bound_to_home_evaluated_before_root_swap() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let moved_home = tmp.path().join("home-before-swap");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(home.join("SOUL.md"), b"before").unwrap();
        fs::write(outside.join("SOUL.md"), b"before").unwrap();
        let home = home.canonicalize().unwrap();
        let outside = outside.canonicalize().unwrap();
        set_evaluated_home_swap(home.clone(), moved_home.clone(), outside.clone());
        let edit = FileEdit::new("SOUL.md", b"after".to_vec());

        let error = ward_in(&home)
            .apply_after_threads_approval(
                std::slice::from_ref(&edit),
                &Authorization::signed_by("SHA256:principal-key"),
                &BTreeMap::from([(edit.target.clone(), b"before".to_vec())]),
                &resolved_as_target(std::slice::from_ref(&edit)),
                ApprovedApplyMode::Initial,
            )
            .unwrap_err();

        assert!(format!("{error:#}").contains("changed during Gate 2"));
        assert_eq!(fs::read(moved_home.join("SOUL.md")).unwrap(), b"before");
        assert_eq!(fs::read(outside.join("SOUL.md")).unwrap(), b"before");
        assert_eq!(staging_litter(&outside), Vec::<String>::new());
    }

    #[cfg(unix)]
    #[test]
    fn direct_rollback_stays_bound_to_parent_renamed_before_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let home = home.canonicalize().unwrap();
        let parent = home.join("memory/nested");
        let moved_parent = home.join("memory/nested-before-swap");
        fs::create_dir_all(&parent).unwrap();
        let first = parent.join("first.md");
        let second = parent.join("second.md");
        fs::write(&first, b"before").unwrap();

        set_conditional_write_test_actions(
            first.clone(),
            vec![ConditionalWriteAction::SwapParentDirectory {
                parent: parent.clone(),
                moved_parent: moved_parent.clone(),
            }],
        );
        set_conditional_write_test_actions(
            second.clone(),
            vec![ConditionalWriteAction::ReplaceRegular {
                target: moved_parent.join("second.md"),
                contents: b"concurrent".to_vec(),
            }],
        );

        let error = ward_in(&home)
            .apply(
                &[
                    FileEdit::new("memory/nested/first.md", b"after".to_vec()),
                    FileEdit::new("memory/nested/second.md", b"ward".to_vec()),
                ],
                &Authorization::unsigned(),
            )
            .expect_err("the second target must force rollback");

        assert!(
            matches!(
                direct_apply_failure(&error),
                Some(DirectApplyFailure::RolledBack)
            ),
            "unexpected error: {error:#}"
        );
        assert_eq!(fs::read(moved_parent.join("first.md")).unwrap(), b"before");
        assert_eq!(
            fs::read(moved_parent.join("second.md")).unwrap(),
            b"concurrent"
        );
        assert!(!first.exists());
        assert!(!second.exists());
        assert_eq!(staging_litter(&moved_parent), Vec::<String>::new());
        assert_eq!(staging_litter(&parent), Vec::<String>::new());
    }

    const EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX: u64 = 16 * 1024 * 1024;
    const EXPECTED_DIRECT_BATCH_EDIT_COUNT_MAX: usize = 32;

    #[test]
    fn ward_edit_budget_reservation_is_overflow_safe() {
        let mut budget =
            WardEditBudget::for_edit_count(1).expect("one edit is within the Ward limit");
        budget
            .reserve_retained_content(1)
            .expect("one retained byte is within the Ward limit");

        let error = budget
            .reserve_retained_content(u64::MAX)
            .expect_err("overflowing retained content must be rejected");

        assert!(matches!(
            ward_edit_budget_failure(&error),
            Some(WardEditBudgetFailure::BatchRetainedMemory {
                attempted_bytes: u64::MAX,
                max_bytes: EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX,
            })
        ));
    }

    #[test]
    fn apply_rejects_direct_batch_above_descriptor_safe_edit_limit_before_preparation() {
        let tmp = tempfile::tempdir().unwrap();
        let edits = (0..=EXPECTED_DIRECT_BATCH_EDIT_COUNT_MAX)
            .map(|index| {
                let target = if index % 2 == 0 {
                    format!("memory/log-{index}.md")
                } else {
                    format!("scratch/output-{index}.txt")
                };
                FileEdit::new(target, Vec::new())
            })
            .collect::<Vec<_>>();

        let error = ward_in(tmp.path())
            .apply(&edits, &Authorization::unsigned())
            .expect_err("a direct batch above the edit-count limit must be rejected");

        assert!(
            format!("{error:#}").contains("33 edits"),
            "unexpected error: {error:#}"
        );
        assert!(
            !tmp.path().join("memory").exists() && !tmp.path().join("scratch").exists(),
            "the count limit must run before preparing target parents"
        );
    }

    #[test]
    fn apply_accepts_descriptor_safe_edit_limit_with_mixed_tier2_tier3_existing_targets() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("memory")).unwrap();
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        let edits = (0..EXPECTED_DIRECT_BATCH_EDIT_COUNT_MAX)
            .map(|index| {
                let target = if index % 2 == 0 {
                    format!("memory/log-{index}.md")
                } else {
                    format!("scratch/output-{index}.txt")
                };
                fs::write(tmp.path().join(&target), []).unwrap();
                FileEdit::new(target, format!("after-{index}").into_bytes())
            })
            .collect::<Vec<_>>();

        let report = ward_in(tmp.path())
            .apply(&edits, &Authorization::unsigned())
            .expect("the exact direct edit-count boundary must remain accepted");

        assert_eq!(report.changes.len(), EXPECTED_DIRECT_BATCH_EDIT_COUNT_MAX);
        assert_eq!(
            report
                .changes
                .iter()
                .filter(|change| change.decision.tier == Tier::Logged)
                .count(),
            EXPECTED_DIRECT_BATCH_EDIT_COUNT_MAX / 2
        );
        assert_eq!(
            report
                .changes
                .iter()
                .filter(|change| change.decision.tier == Tier::Free)
                .count(),
            EXPECTED_DIRECT_BATCH_EDIT_COUNT_MAX / 2
        );
        for (index, edit) in edits.iter().enumerate() {
            assert_eq!(
                fs::read(tmp.path().join(&edit.target)).unwrap(),
                format!("after-{index}").as_bytes()
            );
        }
    }

    #[test]
    fn apply_counts_held_tier1_edits_in_the_shared_ward_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let edits = (0..=EXPECTED_DIRECT_BATCH_EDIT_COUNT_MAX)
            .map(|index| FileEdit::new(format!("reviewed/edit-{index}.md"), Vec::new()))
            .collect::<Vec<_>>();
        let mut config = sample_config();
        config.surface.push(SurfaceEntry {
            path: "reviewed/".into(),
            tier: Tier::Reviewed,
        });
        let ward = Ward::new(tmp.path(), config).unwrap();

        let error = ward
            .apply(&edits, &Authorization::unsigned())
            .expect_err("held Tier 1 edits must use the shared Ward edit-count limit");

        assert!(matches!(
            ward_edit_budget_failure(&error),
            Some(WardEditBudgetFailure::BatchEditCount {
                attempted_edits: 33,
                max_edits: EXPECTED_DIRECT_BATCH_EDIT_COUNT_MAX,
            })
        ));
        assert!(
            !tmp.path().join("reviewed").exists(),
            "budget rejection must precede target preparation"
        );
    }

    #[test]
    fn apply_accepts_exact_shared_count_boundary_with_one_tier1_edit() {
        let tmp = tempfile::tempdir().unwrap();
        let mut edits = vec![FileEdit::new("reviewed/held.md", Vec::new())];
        edits.extend(
            (0..EXPECTED_DIRECT_BATCH_EDIT_COUNT_MAX - 1)
                .map(|index| FileEdit::new(format!("memory/log-{index}.md"), Vec::new())),
        );
        let mut config = sample_config();
        config.surface.push(SurfaceEntry {
            path: "reviewed/".into(),
            tier: Tier::Reviewed,
        });
        let ward = Ward::new(tmp.path(), config).unwrap();

        let report = ward
            .apply(&edits, &Authorization::unsigned())
            .expect("the exact shared Ward edit-count boundary must be accepted");

        assert_eq!(report.changes.len(), EXPECTED_DIRECT_BATCH_EDIT_COUNT_MAX);
        assert!(report
            .changes
            .iter()
            .all(|change| change.disposition == Disposition::HeldForCoherence));
        assert!(
            !tmp.path().join("memory").exists(),
            "a held mixed-tier proposal must not write its lower-tier edits"
        );
    }

    #[test]
    fn apply_rejects_one_tier1_plus_32_or_33_lower_tier_edits() {
        for lower_tier_edits in [32, 33] {
            let tmp = tempfile::tempdir().unwrap();
            let mut edits = vec![FileEdit::new("reviewed/held.md", Vec::new())];
            edits.extend(
                (0..lower_tier_edits)
                    .map(|index| FileEdit::new(format!("memory/log-{index}.md"), Vec::new())),
            );
            let mut config = sample_config();
            config.surface.push(SurfaceEntry {
                path: "reviewed/".into(),
                tier: Tier::Reviewed,
            });
            let ward = Ward::new(tmp.path(), config).unwrap();

            let error = ward
                .apply(&edits, &Authorization::unsigned())
                .expect_err("Tier 1 must not bypass the shared Ward edit-count limit");

            assert!(matches!(
                ward_edit_budget_failure(&error),
                Some(WardEditBudgetFailure::BatchEditCount {
                    attempted_edits,
                    max_edits: EXPECTED_DIRECT_BATCH_EDIT_COUNT_MAX,
                }) if *attempted_edits == lower_tier_edits + 1
            ));
            assert!(!tmp.path().join("reviewed").exists());
            assert!(!tmp.path().join("memory").exists());
        }
    }

    #[test]
    fn apply_rejects_oversized_proposed_content_even_when_tier1_holds_the_batch() {
        let tmp = tempfile::tempdir().unwrap();
        let mut config = sample_config();
        config.surface.push(SurfaceEntry {
            path: "reviewed/".into(),
            tier: Tier::Reviewed,
        });
        let ward = Ward::new(tmp.path(), config).unwrap();
        let edits = vec![
            FileEdit::new("reviewed/held.md", vec![b'x']),
            FileEdit::new(
                "memory/log.md",
                vec![b'y'; EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX as usize],
            ),
        ];

        let error = ward
            .apply(&edits, &Authorization::unsigned())
            .expect_err("Tier 1 must not bypass the shared retained-content budget");

        assert!(matches!(
            ward_edit_budget_failure(&error),
            Some(WardEditBudgetFailure::BatchRetainedMemory {
                attempted_bytes,
                max_bytes: EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX,
            }) if *attempted_bytes == EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX + 1
        ));
        assert!(!tmp.path().join("reviewed").exists());
        assert!(!tmp.path().join("memory").exists());
    }

    #[test]
    fn approved_apply_combines_proposed_and_before_image_bytes_in_shared_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("MEMORY.md");
        let before = vec![b'b'; EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX as usize - 1];
        fs::write(&target, &before).unwrap();
        let edits = vec![FileEdit::new("MEMORY.md", b"xx".to_vec())];
        let expected_before = BTreeMap::from([("MEMORY.md".to_string(), Some(before.clone()))]);

        let error = ward_in(tmp.path())
            .apply_after_coherence_approval(
                &edits,
                &Authorization::unsigned(),
                &expected_before,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .expect_err("approved before-images and proposed bytes share one budget");

        assert!(matches!(
            ward_edit_budget_failure(&error),
            Some(WardEditBudgetFailure::BatchRetainedMemory {
                attempted_bytes,
                max_bytes: EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX,
            }) if *attempted_bytes == EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX + 1
        ));
        assert_eq!(fs::read(&target).unwrap(), before);
        assert_eq!(staging_litter(tmp.path()), Vec::<String>::new());
    }

    #[test]
    fn approved_apply_accepts_exact_combined_content_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("MEMORY.md");
        let before = vec![b'b'; EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX as usize - 1];
        fs::write(&target, &before).unwrap();
        let edits = vec![FileEdit::new("MEMORY.md", b"x".to_vec())];
        let expected_before = BTreeMap::from([("MEMORY.md".to_string(), Some(before))]);

        let report = ward_in(tmp.path())
            .apply_after_coherence_approval(
                &edits,
                &Authorization::unsigned(),
                &expected_before,
                &resolved_as_target(&edits),
                ApprovedApplyMode::Initial,
            )
            .expect("the exact combined retained-content boundary must be accepted");

        assert!(report.is_applied());
        assert_eq!(fs::read(&target).unwrap(), b"x");
        assert_eq!(staging_litter(tmp.path()), Vec::<String>::new());
    }

    #[test]
    fn apply_rejects_oversized_existing_tier3_file_before_mutation() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join("scratch")).unwrap();
        let target = home.join("scratch/large.bin");
        let oversized = vec![b'x'; EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX as usize + 1];
        fs::write(&target, &oversized).unwrap();

        let err = ward_in(home)
            .apply(
                &[FileEdit::new("scratch/large.bin", b"replacement".to_vec())],
                &Authorization::unsigned(),
            )
            .expect_err("an oversized Tier 3 before-image must be rejected");

        assert!(
            format!("{err:#}").contains("retained before-image"),
            "unexpected error: {err:#}"
        );
        assert_eq!(fs::metadata(&target).unwrap().len(), oversized.len() as u64);
        assert_eq!(fs::read(&target).unwrap(), oversized);
        assert_eq!(
            staging_litter(home.join("scratch").as_path()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn apply_rejects_sparse_existing_file_by_logical_metadata_length() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join("scratch")).unwrap();
        let target = home.join("scratch/sparse.bin");
        let file = std::fs::File::create(&target).unwrap();
        file.set_len(EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX + 1)
            .unwrap();
        drop(file);

        let err = ward_in(home)
            .apply(
                &[FileEdit::new("scratch/sparse.bin", Vec::new())],
                &Authorization::unsigned(),
            )
            .expect_err("a metadata-large sparse before-image must be rejected");

        assert!(
            format!("{err:#}").contains("retained before-image"),
            "unexpected error: {err:#}"
        );
        assert_eq!(
            fs::metadata(&target).unwrap().len(),
            EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX + 1
        );
        assert_eq!(
            staging_litter(home.join("scratch").as_path()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn apply_rejects_aggregate_retained_bytes_before_mutation() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join("scratch")).unwrap();
        let each = EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX / 2 + 1;
        let first = home.join("scratch/first.bin");
        let second = home.join("scratch/second.bin");
        std::fs::File::create(&first)
            .unwrap()
            .set_len(each)
            .unwrap();
        std::fs::File::create(&second)
            .unwrap()
            .set_len(each)
            .unwrap();

        let err = ward_in(home)
            .apply(
                &[
                    FileEdit::new("scratch/first.bin", Vec::new()),
                    FileEdit::new("scratch/second.bin", Vec::new()),
                ],
                &Authorization::unsigned(),
            )
            .expect_err("individually valid files must not exceed the batch memory limit");

        assert!(
            format!("{err:#}").contains("batch retained-memory"),
            "unexpected error: {err:#}"
        );
        assert_eq!(fs::metadata(&first).unwrap().len(), each);
        assert_eq!(fs::metadata(&second).unwrap().len(), each);
        assert_eq!(
            staging_litter(home.join("scratch").as_path()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn apply_accepts_exact_direct_retained_bytes_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join("scratch")).unwrap();
        let target = home.join("scratch/boundary.bin");
        std::fs::File::create(&target)
            .unwrap()
            .set_len(EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX)
            .unwrap();

        let report = ward_in(home)
            .apply(
                &[FileEdit::new("scratch/boundary.bin", Vec::new())],
                &Authorization::unsigned(),
            )
            .expect("the exact retained-byte boundary must remain valid");

        assert!(report.is_applied());
        assert_eq!(fs::read(&target).unwrap(), Vec::<u8>::new());
        assert_eq!(
            staging_litter(home.join("scratch").as_path()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn apply_rejects_file_that_grows_after_metadata_before_bounded_read() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join("scratch")).unwrap();
        let target = home.join("scratch/growing.bin");
        fs::write(&target, b"small").unwrap();
        let target = target.canonicalize().unwrap();
        set_direct_read_growth(target.clone(), EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX + 1);

        let err = ward_in(home)
            .apply(
                &[FileEdit::new("scratch/growing.bin", Vec::new())],
                &Authorization::unsigned(),
            )
            .expect_err("growth after metadata must be caught by the bounded read");

        assert!(
            format!("{err:#}").contains("retained before-image"),
            "unexpected error: {err:#}"
        );
        assert_eq!(
            fs::metadata(&target).unwrap().len(),
            EXPECTED_DIRECT_BATCH_RETAINED_BYTES_MAX + 1
        );
        assert_eq!(
            staging_litter(home.join("scratch").as_path()),
            Vec::<String>::new()
        );
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

    #[test]
    fn portable_surface_key_uses_full_unicode_case_folding() {
        assert_eq!(
            portable_surface_key("reviewed/stra\u{00df}e.md"),
            portable_surface_key("reviewed/STRASSE.md")
        );
    }
}
