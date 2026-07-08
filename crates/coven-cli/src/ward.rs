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
//! 4. **Audit logging** — all Tier 0/1 modifications are recorded to an
//!    append-only log. *(Requires store wiring; follow-up.)*
//!
//! This module implements the two **deterministic** gates — 1 and 2 — which are
//! the load-bearing structural checks. It has no dependency on the language
//! model, and every decision it makes is a pure function of the declared
//! surface and the proposal. Gates 3 and 4 are surfaced as verdicts to be
//! discharged by later stages of the pipeline.
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
}
