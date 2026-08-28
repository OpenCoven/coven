//! Detect multiple `coven` executables visible on PATH.
//!
//! Several installs of Coven coexisting is one of the more expensive support
//! failures: the shadowed copy answers every command, so an operator upgrades,
//! sees no change, and concludes the upgrade is broken. It is silent by
//! construction -- nothing errors, the wrong binary simply wins -- and it
//! misleads for as long as it goes unnoticed.
//!
//! The search rules are deliberately *not* `cfg`-gated. Windows resolution
//! (PATHEXT precedence, `;` separator) differs enough from Unix that it needs
//! its own tests, and gating the implementation would mean those tests only
//! ever run on a Windows runner. Everything here takes the platform, the
//! environment, and a filesystem probe as arguments so every rule is exercised
//! on every platform.

use std::path::{Path, PathBuf};

/// Default PATHEXT when Windows does not supply one, in the order Windows
/// itself prefers.
const DEFAULT_PATHEXT: &[&str] = &[".com", ".exe", ".bat", ".cmd"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Unix,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }

    fn separator(self) -> char {
        match self {
            Self::Windows => ';',
            Self::Unix => ':',
        }
    }
}

/// One resolved executable, in the order PATH would consult it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    pub path: PathBuf,
    /// The PATH entry it came from, so an operator can see which directory to
    /// reorder or clean up.
    pub directory: PathBuf,
}

/// Every `name` executable reachable through `path_var`, in resolution order.
///
/// `probe` reports whether a candidate exists and is runnable; injecting it
/// keeps the ordering rules testable without touching a real filesystem.
pub fn installations_on_path(
    name: &str,
    path_var: Option<&str>,
    pathext: Option<&str>,
    platform: Platform,
    probe: &dyn Fn(&Path) -> bool,
) -> Vec<Installation> {
    let Some(path_var) = path_var else {
        return Vec::new();
    };

    let extensions: Vec<String> = match platform {
        Platform::Unix => vec![String::new()],
        Platform::Windows => match pathext {
            Some(raw) if !raw.trim().is_empty() => raw
                .split(';')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                // PATHEXT is conventionally uppercase (".EXE") but files on
                // disk are conventionally lowercase ("coven.exe"). Windows
                // resolves either way, so lowercase the extension for the
                // reported path -- printing "coven.EXE" in a diagnostic sends
                // an operator looking for a filename that is not what they
                // will see in Explorer or what npm installed.
                .map(|entry| {
                    let entry = entry.to_ascii_lowercase();
                    if entry.starts_with('.') {
                        entry
                    } else {
                        format!(".{entry}")
                    }
                })
                .collect(),
            _ => DEFAULT_PATHEXT.iter().map(|e| (*e).to_string()).collect(),
        },
    };

    let mut found: Vec<Installation> = Vec::new();
    for directory in path_var.split(platform.separator()) {
        let directory = directory.trim();
        if directory.is_empty() {
            continue;
        }
        let directory = Path::new(directory);
        // Within one directory Windows consults PATHEXT in order, so a
        // co-located coven.exe and coven.cmd are two distinct installs and the
        // .exe wins. Reporting both is the point: that pair is itself a
        // conflict an operator needs to see.
        for extension in &extensions {
            let candidate = directory.join(format!("{name}{extension}"));
            if !probe(&candidate) {
                continue;
            }
            // PATH routinely repeats directories; a repeat is not a second
            // install.
            if found.iter().any(|existing| existing.path == candidate) {
                continue;
            }
            found.push(Installation {
                path: candidate,
                directory: directory.to_path_buf(),
            });
        }
    }
    found
}

/// Human-readable summary for `coven doctor`. `None` when there is nothing to
/// report -- zero or one install is the healthy case.
pub fn conflict_report(installations: &[Installation]) -> Option<String> {
    if installations.len() < 2 {
        return None;
    }
    let mut lines = Vec::new();
    for (index, installation) in installations.iter().enumerate() {
        let marker = if index == 0 { "active" } else { "shadowed" };
        lines.push(format!("{} ({marker})", installation.path.display()));
    }
    Some(lines.join("; "))
}

/// Resolve against the real environment and filesystem.
pub fn current_installations(name: &str) -> Vec<Installation> {
    let path_var = std::env::var("PATH").ok();
    let pathext = std::env::var("PATHEXT").ok();
    installations_on_path(
        name,
        path_var.as_deref(),
        pathext.as_deref(),
        Platform::current(),
        &is_runnable,
    )
}

#[cfg(unix)]
fn is_runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_runnable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Case-sensitive probe, matching Unix filesystem semantics.
    fn probe_from(paths: &[&str]) -> impl Fn(&Path) -> bool {
        let set: HashSet<PathBuf> = paths.iter().map(PathBuf::from).collect();
        move |candidate: &Path| set.contains(candidate)
    }

    /// Case-insensitive probe, matching Windows filesystem semantics. This
    /// matters: PATHEXT is conventionally uppercase (".EXE") while files on
    /// disk are conventionally lowercase ("coven.exe"), and Windows resolves
    /// them to each other. A case-sensitive fake would make these tests pass
    /// only for spellings that never occur in practice.
    fn windows_probe_from(paths: &[&str]) -> impl Fn(&Path) -> bool {
        let set: HashSet<String> = paths.iter().map(|path| path.to_ascii_lowercase()).collect();
        move |candidate: &Path| set.contains(&candidate.display().to_string().to_ascii_lowercase())
    }

    #[test]
    fn a_single_unix_install_is_not_a_conflict() {
        let probe = probe_from(&["/usr/local/bin/coven"]);
        let found = installations_on_path(
            "coven",
            Some("/usr/local/bin:/usr/bin"),
            None,
            Platform::Unix,
            &probe,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(conflict_report(&found), None);
    }

    #[test]
    fn nothing_installed_is_not_a_conflict() {
        let probe = probe_from(&[]);
        let found =
            installations_on_path("coven", Some("/usr/bin:/bin"), None, Platform::Unix, &probe);
        assert!(found.is_empty());
        assert_eq!(conflict_report(&found), None);
    }

    #[test]
    fn unix_reports_every_install_in_path_order() {
        // The real shape this exists for: a cargo build, a user-local copy,
        // and an npm/nvm global all answering to `coven`. Paths use a neutral
        // fixture root because the privacy guard rejects absolute home paths
        // in source, and rightly so.
        let probe = probe_from(&[
            "/fixture/cargo/bin/coven",
            "/fixture/local/bin/coven",
            "/fixture/nvm/bin/coven",
        ]);
        let found = installations_on_path(
            "coven",
            Some("/fixture/local/bin:/fixture/nvm/bin:/fixture/cargo/bin"),
            None,
            Platform::Unix,
            &probe,
        );
        let rendered: Vec<String> = found
            .iter()
            .map(|install| install.path.display().to_string())
            .collect();
        assert_eq!(
            rendered,
            vec![
                "/fixture/local/bin/coven",
                "/fixture/nvm/bin/coven",
                "/fixture/cargo/bin/coven",
            ],
            "installs must be reported in PATH order so the first is the one that runs"
        );
        let report = conflict_report(&found).expect("three installs is a conflict");
        assert!(report.contains("/fixture/local/bin/coven (active)"));
        assert!(report.contains("/fixture/cargo/bin/coven (shadowed)"));
    }

    #[test]
    fn a_repeated_path_entry_is_not_a_second_install() {
        let probe = probe_from(&["/usr/local/bin/coven"]);
        let found = installations_on_path(
            "coven",
            Some("/usr/local/bin:/usr/bin:/usr/local/bin"),
            None,
            Platform::Unix,
            &probe,
        );
        assert_eq!(found.len(), 1, "a duplicated PATH entry is not a conflict");
        assert_eq!(conflict_report(&found), None);
    }

    #[test]
    fn unix_ignores_a_non_executable_file() {
        // probe_from only reports the paths given, standing in for the
        // executable-bit check the real probe performs.
        let probe = probe_from(&["/usr/bin/coven"]);
        let found = installations_on_path(
            "coven",
            Some("/opt/broken/bin:/usr/bin"),
            None,
            Platform::Unix,
            &probe,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, PathBuf::from("/usr/bin/coven"));
    }

    // Windows paths here use forward slashes so `Path::join` produces the same
    // string on whatever host runs the test; Windows accepts '/' as a separator,
    // and the rules under test are the separator and PATHEXT handling, not the
    // spelling of the join.
    #[test]
    fn windows_splits_path_on_semicolons_and_applies_pathext() {
        let probe = windows_probe_from(&["C:/tools/coven.exe", "C:/npm/coven.cmd"]);
        let found = installations_on_path(
            "coven",
            Some("C:/tools;C:/npm"),
            Some(".COM;.EXE;.BAT;.CMD"),
            Platform::Windows,
            &probe,
        );
        let rendered: Vec<String> = found
            .iter()
            .map(|install| install.path.display().to_string())
            .collect();
        assert_eq!(rendered, vec!["C:/tools/coven.exe", "C:/npm/coven.cmd"]);
        assert!(conflict_report(&found).is_some());
    }

    #[test]
    fn windows_prefers_exe_over_cmd_in_the_same_directory() {
        // Both spellings in one directory is still two installs, and PATHEXT
        // order decides which one Windows actually runs.
        let probe = windows_probe_from(&["C:/npm/coven.cmd", "C:/npm/coven.exe"]);
        let found = installations_on_path(
            "coven",
            Some("C:/npm"),
            Some(".COM;.EXE;.BAT;.CMD"),
            Platform::Windows,
            &probe,
        );
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].path, PathBuf::from("C:/npm/coven.exe"));
        let report = conflict_report(&found).expect("two spellings is a conflict");
        assert!(report.contains("C:/npm/coven.exe (active)"));
        assert!(report.contains("C:/npm/coven.cmd (shadowed)"));
    }

    #[test]
    fn windows_honors_a_reordered_pathext() {
        let probe = windows_probe_from(&["C:/npm/coven.cmd", "C:/npm/coven.exe"]);
        let found = installations_on_path(
            "coven",
            Some("C:/npm"),
            Some(".CMD;.EXE"),
            Platform::Windows,
            &probe,
        );
        assert_eq!(
            found[0].path,
            PathBuf::from("C:/npm/coven.cmd"),
            "PATHEXT order decides which spelling wins, not a hardcoded preference"
        );
    }

    #[test]
    fn windows_falls_back_to_the_default_pathext() {
        let probe = windows_probe_from(&["C:/tools/coven.exe"]);
        for pathext in [None, Some(""), Some("   ")] {
            let found = installations_on_path(
                "coven",
                Some("C:/tools"),
                pathext,
                Platform::Windows,
                &probe,
            );
            assert_eq!(found.len(), 1, "pathext {pathext:?} should fall back");
            assert_eq!(found[0].path, PathBuf::from("C:/tools/coven.exe"));
        }
    }

    #[test]
    fn pathext_entries_without_a_leading_dot_still_match() {
        let probe = windows_probe_from(&["C:/tools/coven.exe"]);
        let found = installations_on_path(
            "coven",
            Some("C:/tools"),
            Some("COM;EXE"),
            Platform::Windows,
            &probe,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, PathBuf::from("C:/tools/coven.exe"));
    }

    #[test]
    fn an_absent_or_empty_path_reports_nothing() {
        let probe = probe_from(&["/usr/bin/coven"]);
        assert!(installations_on_path("coven", None, None, Platform::Unix, &probe).is_empty());
        assert!(installations_on_path("coven", Some(""), None, Platform::Unix, &probe).is_empty());
        assert!(
            installations_on_path("coven", Some("::"), None, Platform::Unix, &probe).is_empty(),
            "empty PATH segments must not be probed as the current directory"
        );
    }
}
