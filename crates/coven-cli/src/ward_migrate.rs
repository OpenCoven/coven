use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use coven_threads_core::IdentityInvariantSet;
use serde::Deserialize;

use crate::ward::{SurfaceEntry, Tier, WardConfig, WARD_CONFIG_FILE};

const V01_BACKUP_FILE: &str = "ward.toml.v01.bak";

#[derive(Debug, Clone)]
pub struct WardMigrateOptions {
    pub familiar: Option<String>,
    pub fingerprint: String,
    pub apply: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationStatus {
    NoWard,
    AlreadyMigrated,
    WouldMigrate,
    Migrated,
    BackupExists,
    Unmigratable,
    ValidationFailed,
}

#[derive(Debug, Clone)]
pub struct MigrationEntry {
    pub familiar_id: String,
    pub workspace: PathBuf,
    pub status: MigrationStatus,
    pub protected_files: Vec<String>,
    pub editable_paths: Vec<String>,
    pub translated_globs: Vec<(String, String)>,
    /// Per-declaration disposition for v0.1 `[protected].invariants`:
    /// each retired declaration either compiled deterministically into a
    /// typed identity invariant or was rejected explicitly. Never silent.
    pub invariant_dispositions: Vec<InvariantDisposition>,
    pub generated_toml: Option<String>,
    pub message: String,
}

/// Fidelity record for one retired v0.1 invariant declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvariantDisposition {
    /// Compiled deterministically by `coven-threads-core`. Records the
    /// typed fact and operator, not the principal's expected value.
    Compiled { fact: String, operator: String },
    /// Rejected explicitly with the compiler's reason.
    Rejected { reason: String },
}

#[derive(Debug, Clone)]
pub struct MigrationReport {
    pub entries: Vec<MigrationEntry>,
}

impl MigrationReport {
    pub fn has_errors(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(
                entry.status,
                MigrationStatus::BackupExists
                    | MigrationStatus::Unmigratable
                    | MigrationStatus::ValidationFailed
            )
        })
    }
}

#[derive(Debug, Deserialize)]
struct LegacyWardConfig {
    protected: Option<LegacyProtected>,
    editable: Option<LegacyEditable>,
}

#[derive(Debug, Deserialize)]
struct LegacyProtected {
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    invariants: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct LegacyEditable {
    #[serde(default)]
    paths: Vec<String>,
}

pub fn run_migration(coven_home: &Path, options: WardMigrateOptions) -> Result<MigrationReport> {
    if options.fingerprint.trim().is_empty() {
        bail!("--fingerprint is required and cannot be empty");
    }

    let mut entries = Vec::new();
    for familiar in crate::cockpit_sources::read_familiars(coven_home)? {
        if options
            .familiar
            .as_deref()
            .is_some_and(|wanted| wanted != familiar.id)
        {
            continue;
        }
        let workspace = crate::cockpit_sources::familiar_workspace(coven_home, &familiar.id);
        entries.push(migrate_one(
            &familiar.id,
            &workspace,
            &options.fingerprint,
            options.apply,
        )?);
    }

    Ok(MigrationReport { entries })
}

fn migrate_one(
    familiar_id: &str,
    workspace: &Path,
    fingerprint: &str,
    apply: bool,
) -> Result<MigrationEntry> {
    let path = workspace.join(WARD_CONFIG_FILE);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(MigrationEntry {
                familiar_id: familiar_id.to_string(),
                workspace: workspace.to_path_buf(),
                status: MigrationStatus::NoWard,
                protected_files: Vec::new(),
                editable_paths: Vec::new(),
                translated_globs: Vec::new(),
                invariant_dispositions: Vec::new(),
                generated_toml: None,
                message: "no ward".to_string(),
            });
        }
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };

    let phase2_error = match WardConfig::from_toml_str(&raw) {
        Ok(_) => {
            // A-1 guard: WardConfig tolerates unknown fields, so a file can
            // parse as valid Phase-2 while still carrying a v0.1
            // `[protected].invariants` remnant. Phase-2 has no invariants
            // surface — blessing such a hybrid as AlreadyMigrated would
            // silently ignore identity declarations. Fail closed instead.
            let remnants = v01_invariant_remnants(&raw);
            if let Some(remnants) = remnants {
                return Ok(MigrationEntry {
                    familiar_id: familiar_id.to_string(),
                    workspace: workspace.to_path_buf(),
                    status: MigrationStatus::Unmigratable,
                    protected_files: Vec::new(),
                    editable_paths: Vec::new(),
                    translated_globs: Vec::new(),
                    invariant_dispositions: Vec::new(),
                    generated_toml: None,
                    message: format!(
                        "ward.toml parses as Phase-2 but retains a v0.1 [protected].invariants remnant ({}); Phase-2 has no invariants surface, so these would be silently inert — remove the remnant or restore the v0.1 file and re-run migration",
                        if remnants == 0 {
                            "empty list".to_string()
                        } else {
                            format!("{remnants} declaration(s)")
                        }
                    ),
                });
            }
            return Ok(MigrationEntry {
                familiar_id: familiar_id.to_string(),
                workspace: workspace.to_path_buf(),
                status: MigrationStatus::AlreadyMigrated,
                protected_files: Vec::new(),
                editable_paths: Vec::new(),
                translated_globs: Vec::new(),
                invariant_dispositions: Vec::new(),
                generated_toml: None,
                message: "already migrated".to_string(),
            });
        }
        Err(error) => error,
    };

    let value: toml::Value = match toml::from_str(&raw) {
        Ok(value) => value,
        Err(error) => {
            return Ok(MigrationEntry {
                familiar_id: familiar_id.to_string(),
                workspace: workspace.to_path_buf(),
                status: MigrationStatus::Unmigratable,
                protected_files: Vec::new(),
                editable_paths: Vec::new(),
                translated_globs: Vec::new(),
                invariant_dispositions: Vec::new(),
                generated_toml: None,
                message: format!("unmigratable ward.toml: {error}"),
            });
        }
    };

    let is_v01 = value.get("meta").is_some() || value.get("protected").is_some();
    if !is_v01 {
        return Ok(MigrationEntry {
            familiar_id: familiar_id.to_string(),
            workspace: workspace.to_path_buf(),
            status: MigrationStatus::Unmigratable,
            protected_files: Vec::new(),
            editable_paths: Vec::new(),
            translated_globs: Vec::new(),
            invariant_dispositions: Vec::new(),
            generated_toml: None,
            message: format!(
                "unmigratable ward.toml: not Phase-2 ({phase2_error:#}) and not recognized Ward v0.1"
            ),
        });
    }

    let legacy: LegacyWardConfig = match value.try_into() {
        Ok(legacy) => legacy,
        Err(error) => {
            return Ok(MigrationEntry {
                familiar_id: familiar_id.to_string(),
                workspace: workspace.to_path_buf(),
                status: MigrationStatus::Unmigratable,
                protected_files: Vec::new(),
                editable_paths: Vec::new(),
                translated_globs: Vec::new(),
                invariant_dispositions: Vec::new(),
                generated_toml: None,
                message: format!("unmigratable Ward v0.1 tables: {error:#}"),
            });
        }
    };
    let (protected_files, legacy_invariants) = legacy
        .protected
        .map(|protected| (protected.files, protected.invariants))
        .unwrap_or_default();
    let editable_paths = legacy
        .editable
        .map(|editable| editable.paths)
        .unwrap_or_default();

    // Fidelity gate for retired v0.1 identity invariants: every declaration
    // must compile deterministically through coven-threads-core or the
    // migration fails closed with each rejection spelled out. Dropping them
    // silently is not an option.
    let invariant_dispositions = compile_invariant_dispositions(&legacy_invariants);
    let rejection_reasons: Vec<String> = invariant_dispositions
        .iter()
        .filter_map(|disposition| match disposition {
            InvariantDisposition::Rejected { reason } => Some(reason.clone()),
            InvariantDisposition::Compiled { .. } => None,
        })
        .collect();
    if !rejection_reasons.is_empty() {
        let message = format!(
            "v0.1 [protected].invariants rejected explicitly ({} rejection(s) across {} declaration(s)): {}",
            rejection_reasons.len(),
            legacy_invariants.len(),
            rejection_reasons.join("; "),
        );
        return Ok(MigrationEntry {
            familiar_id: familiar_id.to_string(),
            workspace: workspace.to_path_buf(),
            status: MigrationStatus::Unmigratable,
            protected_files,
            editable_paths,
            translated_globs: Vec::new(),
            invariant_dispositions,
            generated_toml: None,
            message,
        });
    }

    let mut translated_globs = Vec::new();
    let translated_editable: Vec<String> = editable_paths
        .iter()
        .map(|path| {
            let translated = translate_legacy_editable_path(path);
            if translated != *path {
                translated_globs.push((path.clone(), translated.clone()));
            }
            translated
        })
        .collect();

    let config = WardConfig {
        principal_key_fingerprint: fingerprint.to_string(),
        protected_surface: protected_files.clone(),
        surface: protected_files
            .iter()
            .cloned()
            .map(|path| SurfaceEntry {
                path,
                tier: Tier::Protected,
            })
            .chain(
                translated_editable
                    .iter()
                    .cloned()
                    .map(|path| SurfaceEntry {
                        path,
                        tier: Tier::Logged,
                    }),
            )
            .collect(),
        default_tier: Tier::Logged,
    };
    let generated_toml = render_phase2_toml(&config)?;
    if let Err(error) = WardConfig::from_toml_str(&generated_toml) {
        return Ok(MigrationEntry {
            familiar_id: familiar_id.to_string(),
            workspace: workspace.to_path_buf(),
            status: MigrationStatus::ValidationFailed,
            protected_files,
            editable_paths,
            translated_globs,
            invariant_dispositions: invariant_dispositions.clone(),
            generated_toml: Some(generated_toml),
            message: format!("generated Phase-2 ward.toml failed validation: {error:#}"),
        });
    }

    if !apply {
        return Ok(MigrationEntry {
            familiar_id: familiar_id.to_string(),
            workspace: workspace.to_path_buf(),
            status: MigrationStatus::WouldMigrate,
            protected_files,
            editable_paths,
            translated_globs,
            invariant_dispositions: invariant_dispositions.clone(),
            generated_toml: Some(generated_toml),
            message: format!(
                "would migrate Ward v0.1{}",
                invariant_summary_suffix(&invariant_dispositions)
            ),
        });
    }

    let backup = workspace.join(V01_BACKUP_FILE);
    if backup.exists() {
        return Ok(MigrationEntry {
            familiar_id: familiar_id.to_string(),
            workspace: workspace.to_path_buf(),
            status: MigrationStatus::BackupExists,
            protected_files,
            editable_paths,
            translated_globs,
            invariant_dispositions: invariant_dispositions.clone(),
            generated_toml: Some(generated_toml),
            message: format!("refusing to clobber existing {}", backup.display()),
        });
    }

    fs::write(&backup, raw.as_bytes()).with_context(|| format!("writing {}", backup.display()))?;
    if let Err(error) = write_and_verify(&path, &raw, &generated_toml, workspace) {
        let _ = fs::write(&path, raw.as_bytes());
        return Ok(MigrationEntry {
            familiar_id: familiar_id.to_string(),
            workspace: workspace.to_path_buf(),
            status: MigrationStatus::ValidationFailed,
            protected_files,
            editable_paths,
            translated_globs,
            invariant_dispositions: invariant_dispositions.clone(),
            generated_toml: Some(generated_toml),
            message: format!("failed to write valid migrated ward.toml: {error:#}"),
        });
    }

    Ok(MigrationEntry {
        familiar_id: familiar_id.to_string(),
        workspace: workspace.to_path_buf(),
        status: MigrationStatus::Migrated,
        protected_files,
        editable_paths,
        translated_globs,
        invariant_dispositions: invariant_dispositions.clone(),
        generated_toml: Some(generated_toml),
        message: format!(
            "migrated Ward v0.1{}",
            invariant_summary_suffix(&invariant_dispositions)
        ),
    })
}

/// Detects a v0.1 `[protected].invariants` remnant in a raw ward.toml
/// document. Returns `Some(declaration_count)` when the key is present
/// (a non-array value counts as one remnant), `None` when absent.
fn v01_invariant_remnants(raw: &str) -> Option<usize> {
    let value: toml::Value = toml::from_str(raw).ok()?;
    let invariants = value.get("protected")?.get("invariants")?;
    Some(match invariants.as_array() {
        Some(array) => array.len(),
        None => 1,
    })
}

/// Compiles retired v0.1 `[protected].invariants` declarations through the
/// authoritative coven-threads-core compiler. Returns one disposition per
/// outcome: every declaration in a compiling set is recorded as `Compiled`
/// (typed fact + operator); a set that fails to compile yields the compiler's
/// explicit `Rejected` reasons (parse errors are indexed per declaration;
/// set-level violations such as duplicate facts or a missing mandatory
/// name/person binding reject the set as a unit). An absent or empty list
/// yields no dispositions: there is nothing to preserve and nothing to drop.
fn compile_invariant_dispositions(declarations: &[String]) -> Vec<InvariantDisposition> {
    if declarations.is_empty() {
        return Vec::new();
    }
    match IdentityInvariantSet::compile(declarations) {
        Ok(set) => set
            .declarations()
            .iter()
            .map(|declaration| InvariantDisposition::Compiled {
                fact: format!("{:?}", declaration.fact),
                operator: format!("{:?}", declaration.operator),
            })
            .collect(),
        Err(errors) => errors
            .into_iter()
            .map(|reason| InvariantDisposition::Rejected { reason })
            .collect(),
    }
}

fn invariant_summary_suffix(dispositions: &[InvariantDisposition]) -> String {
    if dispositions.is_empty() {
        return String::new();
    }
    format!(
        "; {} retired identity invariant(s) compiled deterministically (preserved in {}, not carried into Phase-2 ward.toml)",
        dispositions.len(),
        V01_BACKUP_FILE,
    )
}

fn render_phase2_toml(config: &WardConfig) -> Result<String> {
    let mut out = format!(
        "# Migrated from Ward v0.1 by `coven ward migrate` on {}; original preserved at ward.toml.v01.bak\n\n",
        crate::api::current_timestamp()
    );
    out.push_str(&toml::to_string(config).context("serializing Phase-2 WardConfig")?);
    Ok(out)
}

fn translate_legacy_editable_path(path: &str) -> String {
    let normalized = path.trim().trim_start_matches("./").replace('\\', "/");
    if normalized.ends_with('/') {
        normalized
    } else if let Some(prefix) = normalized.strip_suffix("/*") {
        format!("{prefix}/**")
    } else {
        normalized
    }
}

fn write_and_verify(path: &Path, original: &str, generated: &str, workspace: &Path) -> Result<()> {
    let temp = path.with_file_name(format!(
        "{}.migrate.tmp.{}",
        WARD_CONFIG_FILE,
        std::process::id()
    ));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .with_context(|| format!("creating {}", temp.display()))?;
        file.write_all(generated.as_bytes())
            .with_context(|| format!("writing {}", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing {}", temp.display()))?;
    }
    fs::rename(&temp, path)
        .with_context(|| format!("renaming {} to {}", temp.display(), path.display()))?;
    match WardConfig::load(workspace) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => {
            fs::write(path, original.as_bytes()).ok();
            Err(anyhow!("migrated ward.toml disappeared after write"))
        }
        Err(error) => {
            fs::write(path, original.as_bytes()).ok();
            Err(error)
        }
    }
}

pub fn print_report(report: &MigrationReport) {
    for entry in &report.entries {
        println!(
            "{}: {:?} — {} ({})",
            entry.familiar_id,
            entry.status,
            entry.message,
            entry.workspace.display()
        );
        println!(
            "  target: {}",
            entry.workspace.join(WARD_CONFIG_FILE).display()
        );
        if !entry.protected_files.is_empty() {
            println!(
                "  protected -> tier 0: {}",
                entry.protected_files.join(", ")
            );
        }
        if !entry.editable_paths.is_empty() {
            println!("  editable -> tier 2: {}", entry.editable_paths.join(", "));
        }
        for (from, to) in &entry.translated_globs {
            println!("  translated glob: {from} -> {to}");
        }
        for disposition in &entry.invariant_dispositions {
            match disposition {
                InvariantDisposition::Compiled { fact, operator } => {
                    println!("  invariant compiled: {fact} {operator}");
                }
                InvariantDisposition::Rejected { reason } => {
                    println!("  invariant rejected: {reason}");
                }
            }
        }
        if entry.generated_toml.is_some() {
            let validation = if entry.status == MigrationStatus::ValidationFailed {
                "failed"
            } else {
                "passed"
            };
            println!("  round-trip validation: {validation}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ward::{Authorization, Proposal, Tier, Ward, WardConfig};
    use anyhow::Result;
    use std::fs;
    use std::path::Path;

    fn seed_familiars(home: &Path) -> Result<std::path::PathBuf> {
        let workspace = home.join("workspaces").join("familiars").join("nova");
        fs::create_dir_all(&workspace)?;
        fs::write(
            home.join("familiars.toml"),
            familiar_fixture_toml(&workspace),
        )?;
        Ok(workspace)
    }

    fn familiar_fixture_toml(workspace: &Path) -> String {
        let workspace = workspace.display().to_string().replace('\\', "/");
        format!(
            r#"[[familiar]]
id = "nova"
display_name = "Nova"
role = "Integration"
description = "Synthetic migration fixture."
workspace = "{}"
"#,
            workspace
        )
    }

    fn synthetic_v01() -> &'static str {
        r#"[meta]
version = "0.1"
owner = "nova"

[protected]
files = ["SOUL.md", "IDENTITY.md"]
invariants = [
    "familiar.name == 'Nova'",
    "familiar.person == \"Val Alexander\"",
    "familiar.pronouns == 'they/them'",
    "familiar.purpose includes 'authority boundary'",
    "familiar.coven includes \"OpenCoven\"",
]

[editable]
paths = ["skills/*/", "memory/*", "notes/"]
harness_blocks = ["synthetic-harness"]

[approval_tiers.tier0]
required = ["principal"]

[audit]
append_only = true
"#
    }

    fn load_migrated_config(workspace: &Path) -> Result<WardConfig> {
        WardConfig::load(workspace)?.ok_or_else(|| anyhow::anyhow!("missing ward config"))
    }

    #[test]
    fn familiar_fixture_toml_escapes_windows_workspace_paths() -> Result<()> {
        let raw = familiar_fixture_toml(Path::new(
            r"C:\Users\RUNNER~1\AppData\Local\Temp\.tmpqHnJfu\workspaces\familiars\nova",
        ));

        let parsed: toml::Value = toml::from_str(&raw)?;
        assert_eq!(parsed["familiar"][0]["id"].as_str(), Some("nova"));
        Ok(())
    }

    #[test]
    fn dry_run_migrates_v01_to_valid_phase2_without_writing_and_matches_tiers() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = seed_familiars(temp.path())?;
        let original = synthetic_v01();
        fs::write(workspace.join("ward.toml"), original)?;

        let report = run_migration(
            temp.path(),
            WardMigrateOptions {
                familiar: None,
                fingerprint: "SHA256:test-principal".to_string(),
                apply: false,
            },
        )?;

        assert!(!report.has_errors());
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].status, MigrationStatus::WouldMigrate);
        assert!(report.entries[0]
            .translated_globs
            .contains(&("memory/*".to_string(), "memory/**".to_string())));
        assert_eq!(
            report.entries[0].invariant_dispositions,
            [
                ("Name", "Equals"),
                ("Person", "Equals"),
                ("Pronouns", "Equals"),
                ("Purpose", "Includes"),
                ("Coven", "Includes"),
            ]
            .map(|(fact, operator)| InvariantDisposition::Compiled {
                fact: fact.to_string(),
                operator: operator.to_string(),
            })
        );
        assert!(report.entries[0]
            .message
            .contains("5 retired identity invariant(s) compiled deterministically"));
        assert_eq!(fs::read_to_string(workspace.join("ward.toml"))?, original);
        assert!(!workspace.join("ward.toml.v01.bak").exists());

        let generated = report.entries[0]
            .generated_toml
            .as_deref()
            .expect("dry run includes generated toml");
        assert!(generated.contains("Migrated from Ward v0.1"));
        // Retired invariants are compiled for fidelity and preserved in the
        // v0.1 backup; the generated Phase-2 ward.toml has no invariants
        // surface, so they must not leak into it.
        assert!(!generated.contains("invariants"));
        assert!(!generated.contains("harness_blocks"));
        assert!(!generated.contains("approval_tiers"));
        assert!(!generated.contains("[audit]"));
        let config = WardConfig::from_toml_str(generated)?;
        assert_eq!(config.principal_key_fingerprint, "SHA256:test-principal");
        assert_eq!(
            config.protected_surface,
            vec!["SOUL.md".to_string(), "IDENTITY.md".to_string()]
        );
        let tier0: Vec<_> = config
            .surface
            .iter()
            .filter(|entry| entry.tier == Tier::Protected)
            .map(|entry| entry.path.clone())
            .collect();
        assert_eq!(config.protected_surface, tier0);

        let ward = Ward::new(workspace, config)?;
        let outcome = ward.evaluate(&Proposal {
            targets: vec![
                "skills/rust/SKILL.md".to_string(),
                "memory/deep/fact.md".to_string(),
                "notes/today.md".to_string(),
            ],
            authorization: Authorization::default(),
        });
        assert!(outcome.decisions.iter().all(|d| d.tier == Tier::Logged));
        Ok(())
    }

    #[test]
    fn apply_writes_backup_and_valid_config_then_second_run_is_already_migrated() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = seed_familiars(temp.path())?;
        let original = synthetic_v01();
        fs::write(workspace.join("ward.toml"), original)?;

        let applied = run_migration(
            temp.path(),
            WardMigrateOptions {
                familiar: Some("nova".to_string()),
                fingerprint: "SHA256:test-principal".to_string(),
                apply: true,
            },
        )?;

        assert!(!applied.has_errors());
        assert_eq!(applied.entries[0].status, MigrationStatus::Migrated);
        assert_eq!(
            fs::read_to_string(workspace.join("ward.toml.v01.bak"))?,
            original
        );
        let config = load_migrated_config(&workspace)?;
        assert_eq!(config.principal_key_fingerprint, "SHA256:test-principal");

        let second = run_migration(
            temp.path(),
            WardMigrateOptions {
                familiar: Some("nova".to_string()),
                fingerprint: "SHA256:test-principal".to_string(),
                apply: true,
            },
        )?;
        assert!(!second.has_errors());
        assert_eq!(second.entries[0].status, MigrationStatus::AlreadyMigrated);
        Ok(())
    }

    fn synthetic_v01_with_invariants(invariants_toml: &str) -> String {
        format!(
            r#"[meta]
version = "0.1"
owner = "nova"

[protected]
files = ["SOUL.md"]
{invariants_toml}

[editable]
paths = ["notes/"]
"#
        )
    }

    #[test]
    fn migration_rejects_unparseable_v01_invariants_explicitly_without_writing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = seed_familiars(temp.path())?;
        let original = synthetic_v01_with_invariants(
            r#"invariants = ["synthetic invariant one", "synthetic invariant two"]"#,
        );
        fs::write(workspace.join("ward.toml"), &original)?;

        let report = run_migration(
            temp.path(),
            WardMigrateOptions {
                familiar: Some("nova".to_string()),
                fingerprint: "SHA256:test-principal".to_string(),
                apply: true,
            },
        )?;

        assert!(report.has_errors());
        let entry = &report.entries[0];
        assert_eq!(entry.status, MigrationStatus::Unmigratable);
        assert!(entry
            .message
            .contains("v0.1 [protected].invariants rejected explicitly (2 rejection(s) across 2 declaration(s))"));
        assert!(entry.message.contains("invariant[0]"));
        assert!(entry.message.contains("invariant[1]"));
        assert!(entry
            .invariant_dispositions
            .iter()
            .all(|d| matches!(d, InvariantDisposition::Rejected { .. })));
        assert_eq!(entry.invariant_dispositions.len(), 2);
        assert!(entry.generated_toml.is_none());

        // Fail closed: nothing written, nothing dropped.
        assert_eq!(fs::read_to_string(workspace.join("ward.toml"))?, original);
        assert!(!workspace.join("ward.toml.v01.bak").exists());
        Ok(())
    }

    #[test]
    fn migration_rejects_each_invalid_invariant_shape_explicitly() -> Result<()> {
        // One case per rejection lane in the coven-threads-core compiler;
        // mandatory name/person cases mirror the retired-Ward corpus grammar.
        let cases: &[(&str, &str)] = &[
            (
                r#"invariants = ["familiar.mood == 'sunny'", "familiar.name == 'Nova'", "familiar.person == 'Val'"]"#,
                "unsupported identity fact",
            ),
            (
                r#"invariants = ["familiar.name matches 'Nova'", "familiar.person == 'Val'"]"#,
                "expected `==` or `includes` operator",
            ),
            (
                r#"invariants = ["familiar.name == 'Nova'", "familiar.name == 'Supernova'", "familiar.person == 'Val'"]"#,
                "duplicate Name identity invariant",
            ),
            (
                r#"invariants = ["familiar.name == ''", "familiar.person == 'Val'"]"#,
                "expected value must not be empty",
            ),
            (
                r#"invariants = ["familiar.person == 'Val'"]"#,
                "missing mandatory Name identity invariant",
            ),
            (
                r#"invariants = ["familiar.name == 'Nova'"]"#,
                "missing mandatory Person identity invariant",
            ),
        ];

        for (invariants_toml, expected_reason) in cases {
            let temp = tempfile::tempdir()?;
            let workspace = seed_familiars(temp.path())?;
            fs::write(
                workspace.join("ward.toml"),
                synthetic_v01_with_invariants(invariants_toml),
            )?;

            let report = run_migration(
                temp.path(),
                WardMigrateOptions {
                    familiar: Some("nova".to_string()),
                    fingerprint: "SHA256:test-principal".to_string(),
                    apply: false,
                },
            )?;

            let entry = &report.entries[0];
            assert_eq!(
                entry.status,
                MigrationStatus::Unmigratable,
                "case {invariants_toml:?} must fail closed"
            );
            assert!(
                entry.message.contains(expected_reason),
                "case {invariants_toml:?}: message {:?} must contain {expected_reason:?}",
                entry.message
            );
        }
        Ok(())
    }

    #[test]
    fn migration_without_invariants_migrates_with_no_dispositions() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = seed_familiars(temp.path())?;
        fs::write(
            workspace.join("ward.toml"),
            synthetic_v01_with_invariants(""),
        )?;

        let report = run_migration(
            temp.path(),
            WardMigrateOptions {
                familiar: Some("nova".to_string()),
                fingerprint: "SHA256:test-principal".to_string(),
                apply: true,
            },
        )?;

        assert!(!report.has_errors());
        let entry = &report.entries[0];
        assert_eq!(entry.status, MigrationStatus::Migrated);
        assert!(entry.invariant_dispositions.is_empty());
        assert!(!entry.message.contains("identity invariant"));
        Ok(())
    }

    #[test]
    fn hybrid_phase2_ward_with_v01_invariant_remnant_fails_closed() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = seed_familiars(temp.path())?;
        fs::write(workspace.join("ward.toml"), synthetic_v01())?;
        run_migration(
            temp.path(),
            WardMigrateOptions {
                familiar: Some("nova".to_string()),
                fingerprint: "SHA256:test-principal".to_string(),
                apply: true,
            },
        )?;

        // Graft a v0.1 invariants remnant onto the migrated Phase-2 file;
        // WardConfig tolerates unknown fields, so it still parses as Phase-2.
        let migrated = fs::read_to_string(workspace.join("ward.toml"))?;
        let hybrid =
            format!("{migrated}\n[protected]\ninvariants = [\"familiar.name == 'Nova'\"]\n");
        fs::write(workspace.join("ward.toml"), &hybrid)?;

        let report = run_migration(
            temp.path(),
            WardMigrateOptions {
                familiar: Some("nova".to_string()),
                fingerprint: "SHA256:test-principal".to_string(),
                apply: true,
            },
        )?;

        assert!(report.has_errors());
        let entry = &report.entries[0];
        assert_eq!(entry.status, MigrationStatus::Unmigratable);
        assert!(entry
            .message
            .contains("retains a v0.1 [protected].invariants remnant (1 declaration(s))"));
        // Fail closed: hybrid file left untouched for the principal to fix.
        assert_eq!(fs::read_to_string(workspace.join("ward.toml"))?, hybrid);
        Ok(())
    }

    #[test]
    fn apply_preserves_compiled_invariants_in_backup_only() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = seed_familiars(temp.path())?;
        let original = synthetic_v01();
        fs::write(workspace.join("ward.toml"), original)?;

        let report = run_migration(
            temp.path(),
            WardMigrateOptions {
                familiar: Some("nova".to_string()),
                fingerprint: "SHA256:test-principal".to_string(),
                apply: true,
            },
        )?;

        assert!(!report.has_errors());
        let entry = &report.entries[0];
        assert_eq!(entry.status, MigrationStatus::Migrated);
        assert_eq!(entry.invariant_dispositions.len(), 5);
        assert!(entry
            .message
            .contains("preserved in ward.toml.v01.bak, not carried into Phase-2 ward.toml"));

        // The declarations survive verbatim in the backup and only there.
        let backup = fs::read_to_string(workspace.join("ward.toml.v01.bak"))?;
        assert!(backup.contains("familiar.name == 'Nova'"));
        let migrated = fs::read_to_string(workspace.join("ward.toml"))?;
        assert!(!migrated.contains("invariants"));
        Ok(())
    }

    #[test]
    fn existing_backup_refuses_without_clobbering() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = seed_familiars(temp.path())?;
        let original = synthetic_v01();
        fs::write(workspace.join("ward.toml"), original)?;
        fs::write(workspace.join("ward.toml.v01.bak"), "existing backup")?;

        let report = run_migration(
            temp.path(),
            WardMigrateOptions {
                familiar: None,
                fingerprint: "SHA256:test-principal".to_string(),
                apply: true,
            },
        )?;

        assert!(report.has_errors());
        assert_eq!(report.entries[0].status, MigrationStatus::BackupExists);
        assert_eq!(fs::read_to_string(workspace.join("ward.toml"))?, original);
        assert_eq!(
            fs::read_to_string(workspace.join("ward.toml.v01.bak"))?,
            "existing backup"
        );
        Ok(())
    }

    #[test]
    fn garbage_ward_is_unmigratable_without_writing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = seed_familiars(temp.path())?;
        fs::write(workspace.join("ward.toml"), "not = [valid")?;

        let report = run_migration(
            temp.path(),
            WardMigrateOptions {
                familiar: None,
                fingerprint: "SHA256:test-principal".to_string(),
                apply: true,
            },
        )?;

        assert!(report.has_errors());
        assert_eq!(report.entries[0].status, MigrationStatus::Unmigratable);
        assert_eq!(
            fs::read_to_string(workspace.join("ward.toml"))?,
            "not = [valid"
        );
        assert!(!workspace.join("ward.toml.v01.bak").exists());
        Ok(())
    }

    #[test]
    fn missing_ward_is_reported_as_no_ward_skip() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = seed_familiars(temp.path())?;

        let report = run_migration(
            temp.path(),
            WardMigrateOptions {
                familiar: None,
                fingerprint: "SHA256:test-principal".to_string(),
                apply: true,
            },
        )?;

        assert!(!report.has_errors());
        assert_eq!(report.entries[0].status, MigrationStatus::NoWard);
        assert!(!workspace.join("ward.toml").exists());
        assert!(!workspace.join("ward.toml.v01.bak").exists());
        Ok(())
    }
}
