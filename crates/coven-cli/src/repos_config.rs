use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

pub const REPOS_CONFIG_FILE_NAME: &str = "repos.toml";

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ReposConfig {
    pub default: Option<String>,
    #[serde(default)]
    pub repos: BTreeMap<String, RepoEntry>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct RepoEntry {
    pub path: PathBuf,
}

pub fn config_path(coven_home: &Path) -> PathBuf {
    coven_home.join(REPOS_CONFIG_FILE_NAME)
}

pub fn load(coven_home: &Path) -> Result<ReposConfig> {
    load_from(&config_path(coven_home))
}

pub fn load_with_settings(
    coven_home: &Path,
    settings: Option<&crate::settings::Settings>,
) -> Result<ReposConfig> {
    let mut config = load(coven_home)?;
    let Some(settings) = settings else {
        return Ok(config);
    };
    for (name, repo) in &settings.coven_cli.repos {
        config.repos.insert(
            name.clone(),
            RepoEntry {
                path: repo.path.clone(),
            },
        );
    }
    if let Some(name) = &settings.coven_cli.default_repo {
        config.default = Some(name.clone());
    }
    Ok(config)
}

fn load_from(path: &Path) -> Result<ReposConfig> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(ReposConfig::default()),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

impl ReposConfig {
    pub fn resolve(&self, name: &str) -> Option<PathBuf> {
        self.repos.get(name).map(|entry| expand_tilde(&entry.path))
    }

    pub fn default_name(&self) -> Option<&str> {
        self.default.as_deref()
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, PathBuf)> + '_ {
        self.repos
            .iter()
            .map(|(name, entry)| (name.as_str(), expand_tilde(&entry.path)))
    }
}

fn expand_tilde(path: &Path) -> PathBuf {
    expand_tilde_with_home(path, std::env::var_os("HOME"))
}

/// Injectable core of [`expand_tilde`], so tests can pin the expansion rules
/// without swapping the process-global `HOME` variable.
fn expand_tilde_with_home(path: &Path, home: Option<OsString>) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    let home = home.filter(|v| !v.is_empty());
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = home {
            return PathBuf::from(home).join(rest);
        }
    } else if s == "~" {
        if let Some(home) = home {
            return PathBuf::from(home);
        }
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn missing_file_returns_empty_config() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config = load(temp.path())?;
        assert_eq!(config, ReposConfig::default());
        assert!(config.entries().next().is_none());
        assert!(config.default_name().is_none());
        Ok(())
    }

    #[test]
    fn loads_named_repos() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            config_path(temp.path()),
            r#"
default = "alpha"

[repos.alpha]
path = "/abs/alpha"

[repos.beta]
path = "/abs/beta"
"#,
        )?;

        let config = load(temp.path())?;

        assert_eq!(config.default_name(), Some("alpha"));
        assert_eq!(config.resolve("alpha"), Some(PathBuf::from("/abs/alpha")));
        assert_eq!(config.resolve("beta"), Some(PathBuf::from("/abs/beta")));
        assert_eq!(config.resolve("missing"), None);
        Ok(())
    }

    #[test]
    fn resolve_expands_leading_tilde() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            config_path(temp.path()),
            r#"
[repos.home_relative]
path = "~/projects/example"
"#,
        )?;

        // Inject HOME instead of swapping the process env: set_var races with
        // parallel tests and a panic before restore leaks the fake value.
        let config = load(temp.path())?;
        let raw = &config
            .repos
            .get("home_relative")
            .expect("entry parsed")
            .path;
        let home = || Some(OsString::from("/tmp/fake-home"));

        assert_eq!(
            expand_tilde_with_home(raw, home()),
            PathBuf::from("/tmp/fake-home/projects/example")
        );
        assert_eq!(
            expand_tilde_with_home(Path::new("~"), home()),
            PathBuf::from("/tmp/fake-home")
        );
        // Missing or empty HOME leaves the path untouched.
        assert_eq!(
            expand_tilde_with_home(raw, None),
            Path::new("~/projects/example")
        );
        assert_eq!(
            expand_tilde_with_home(raw, Some(OsString::new())),
            Path::new("~/projects/example")
        );
        Ok(())
    }

    #[test]
    fn entries_iterates_in_sorted_order() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            config_path(temp.path()),
            r#"
[repos.beta]
path = "/abs/beta"
[repos.alpha]
path = "/abs/alpha"
"#,
        )?;
        let config = load(temp.path())?;
        let names: Vec<&str> = config.entries().map(|(name, _)| name).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        Ok(())
    }

    #[test]
    fn invalid_toml_surfaces_parse_error() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(config_path(temp.path()), "this is not = valid toml [")?;
        let err = load(temp.path()).unwrap_err();
        assert!(
            err.to_string().contains("failed to parse"),
            "unexpected error: {err:?}"
        );
        Ok(())
    }

    #[test]
    fn settings_override_toml_when_both_present() -> Result<()> {
        let temp = tempfile::tempdir()?;
        // legacy TOML
        std::fs::write(
            config_path(temp.path()),
            r#"
default = "from_toml"
[repos.from_toml]
path = "/abs/toml"
"#,
        )?;
        // new JSONC settings (constructed in-memory; this test does not exercise the json5 loader)
        let settings = crate::settings::Settings {
            coven_cli: crate::settings::CovenCliSettings {
                default_repo: Some("from_jsonc".to_string()),
                repos: std::collections::BTreeMap::from([(
                    "from_jsonc".to_string(),
                    crate::settings::RepoSettings {
                        path: std::path::PathBuf::from("/abs/jsonc"),
                    },
                )]),
                ..Default::default()
            },
        };
        let merged = load_with_settings(temp.path(), Some(&settings))?;
        assert_eq!(merged.default_name(), Some("from_jsonc"));
        assert_eq!(
            merged.resolve("from_jsonc"),
            Some(std::path::PathBuf::from("/abs/jsonc"))
        );
        // legacy entry still resolvable as a fallback
        assert_eq!(
            merged.resolve("from_toml"),
            Some(std::path::PathBuf::from("/abs/toml"))
        );
        Ok(())
    }
}
