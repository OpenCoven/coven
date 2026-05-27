use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use serde::Deserialize;

pub const SETTINGS_FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub coven_cli: CovenCliSettings,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CovenCliSettings {
    pub privacy: Option<PrivacySettings>,
    pub repos: BTreeMap<String, RepoSettings>,
    pub default_repo: Option<String>,
    pub fuzzy: FuzzySettings,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrivacySettings {
    pub persist_raw_artifacts: Option<bool>,
    pub raw_artifact_retention_days: Option<u64>,
    pub log_retention_days: Option<u64>,
    pub extra_patterns: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RepoSettings {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FuzzySettings {
    pub always_include_paths: Vec<String>,
}

pub fn user_settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("coven").join(SETTINGS_FILE_NAME))
}

/// Returns the set of dotted-path keys that appear in both inputs.
///
/// Both inputs are expected to be **dotted JSONC paths** (e.g. `"repos.alpha"`,
/// `"defaultRepo"`, `"privacy.logRetentionDays"`), not raw TOML field names or
/// bare repo names. Callers building these lists from a TOML loader must prefix
/// nested fields with their parent (e.g. `"repos.{name}"` for entries inside the
/// `[repos.*]` table).
#[allow(dead_code)] // Wired into run_doctor in a follow-up; see plan
                    // docs/superpowers/plans/2026-05-26-coven-cli-p0-coven-code-parity.md
                    // Task 1.5 (deferred wire-up after PrivacySettings lands).
pub fn shadowed_keys(toml_keys: &[String], jsonc_keys: &[String]) -> Vec<String> {
    let mut out: Vec<String> = toml_keys
        .iter()
        .filter(|k| jsonc_keys.iter().any(|j| j == *k))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

#[allow(dead_code)] // See shadowed_keys above.
pub fn warn_if_shadowed(shadowed: &[String], toml_path: &Path, jsonc_path: &Path) {
    if shadowed.is_empty() {
        return;
    }
    eprintln!(
        "coven: {} keys in {} are shadowed by {}. Consider removing them from the TOML file.",
        shadowed.len(),
        toml_path.display(),
        jsonc_path.display()
    );
    for key in shadowed {
        eprintln!("  - {key}");
    }
}

static CACHED: OnceLock<Option<Settings>> = OnceLock::new();

/// Initialize the process-wide cached Settings. Call exactly once at startup.
/// Subsequent calls are no-ops (the first init wins).
pub fn init_cached(value: Option<Settings>) {
    let _ = CACHED.set(value);
}

/// Get the cached Settings, if any. Returns `None` if init has not run or if
/// the loader returned None at startup.
pub fn cached() -> Option<&'static Settings> {
    CACHED.get().and_then(|opt| opt.as_ref())
}

pub fn load_from(path: &Path) -> Result<Option<Settings>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let parsed: Settings =
        json5::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(parsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        assert_eq!(load_from(&path).unwrap(), None);
    }

    #[test]
    fn loads_jsonc_with_comments_and_trailing_commas() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{
              // user comment
              "covenCli": {
                "defaultRepo": "alpha",
                "repos": {
                  "alpha": { "path": "/abs/alpha" },
                },
              },
            }"#,
        )
        .unwrap();
        let loaded = load_from(&path).unwrap().unwrap();
        assert_eq!(loaded.coven_cli.default_repo.as_deref(), Some("alpha"));
        assert_eq!(
            loaded.coven_cli.repos.get("alpha").unwrap().path,
            PathBuf::from("/abs/alpha")
        );
    }

    #[test]
    fn empty_object_returns_default_settings() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, "{}").unwrap();
        assert_eq!(load_from(&path).unwrap().unwrap(), Settings::default());
    }

    #[test]
    fn detect_shadowed_keys_lists_overrides() {
        let toml_keys = ["repos.alpha".to_string(), "defaultRepo".to_string()];
        let jsonc_keys = ["repos.alpha".to_string()];
        let shadowed = shadowed_keys(&toml_keys, &jsonc_keys);
        assert_eq!(shadowed, vec!["repos.alpha".to_string()]);
    }

    #[test]
    fn cached_returns_the_initialized_value() {
        // Note: OnceLock is process-wide and cannot be reset between tests. This
        // test is the only one that should ever call init_cached. If you are adding
        // another test that also calls init_cached, replace this assertion with a
        // helper that asserts the OnceLock state without re-initializing it.
        let settings = Settings::default();
        init_cached(Some(settings.clone()));
        assert_eq!(cached(), Some(&settings));
    }
}
