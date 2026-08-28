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
            // install. On Windows the comparison must be case-insensitive:
            // C:\\Tools and C:\\tools are the same directory, so a PATH listing
            // both would otherwise report one file as two competing installs.
            let already_found = found.iter().any(|existing| match platform {
                Platform::Windows => existing
                    .path
                    .as_os_str()
                    .eq_ignore_ascii_case(candidate.as_os_str()),
                Platform::Unix => existing.path == candidate,
            });
            if already_found {
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

    /// Build a candidate exactly the way `installations_on_path` does, so the
    /// host's separator is used on both sides. An earlier version of these
    /// tests hardcoded joined paths as string literals; that passes on the
    /// platform the literals were written for and fails on the other, which is
    /// precisely the bug this module exists to catch in production code.
    fn at(directory: &str, file: &str) -> PathBuf {
        Path::new(directory).join(file)
    }

    /// Case-sensitive probe, matching Unix filesystem semantics.
    fn probe(paths: Vec<PathBuf>) -> impl Fn(&Path) -> bool {
        let set: HashSet<PathBuf> = paths.into_iter().collect();
        move |candidate: &Path| set.contains(candidate)
    }

    /// Case-insensitive probe, matching Windows filesystem semantics. PATHEXT
    /// is conventionally uppercase (".EXE") while files on disk are lowercase
    /// ("coven.exe"), and Windows resolves them to each other.
    fn windows_probe(paths: Vec<PathBuf>) -> impl Fn(&Path) -> bool {
        let set: HashSet<String> = paths
            .into_iter()
            .map(|path| path.display().to_string().to_ascii_lowercase())
            .collect();
        move |candidate: &Path| set.contains(&candidate.display().to_string().to_ascii_lowercase())
    }

    fn rendered(found: &[Installation]) -> Vec<PathBuf> {
        found.iter().map(|install| install.path.clone()).collect()
    }

    #[test]
    fn a_single_unix_install_is_not_a_conflict() {
        let found = installations_on_path(
            "coven",
            Some("/usr/local/bin:/usr/bin"),
            None,
            Platform::Unix,
            &probe(vec![at("/usr/local/bin", "coven")]),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(conflict_report(&found), None);
    }

    #[test]
    fn nothing_installed_is_not_a_conflict() {
        let found = installations_on_path(
            "coven",
            Some("/usr/bin:/bin"),
            None,
            Platform::Unix,
            &probe(Vec::new()),
        );
        assert!(found.is_empty());
        assert_eq!(conflict_report(&found), None);
    }

    #[test]
    fn unix_reports_every_install_in_path_order() {
        // The real shape this exists for: a cargo build, a user-local copy, and
        // an npm/nvm global all answering to `coven`.
        let cargo = at("/fixture/cargo/bin", "coven");
        let local = at("/fixture/local/bin", "coven");
        let nvm = at("/fixture/nvm/bin", "coven");
        let found = installations_on_path(
            "coven",
            Some("/fixture/local/bin:/fixture/nvm/bin:/fixture/cargo/bin"),
            None,
            Platform::Unix,
            &probe(vec![cargo.clone(), local.clone(), nvm.clone()]),
        );
        assert_eq!(
            rendered(&found),
            vec![local.clone(), nvm, cargo.clone()],
            "installs must be reported in PATH order so the first is the one that runs"
        );
        let report = conflict_report(&found).expect("three installs is a conflict");
        assert!(report.contains(&format!("{} (active)", local.display())));
        assert!(report.contains(&format!("{} (shadowed)", cargo.display())));
    }

    #[test]
    fn a_repeated_path_entry_is_not_a_second_install() {
        let found = installations_on_path(
            "coven",
            Some("/usr/local/bin:/usr/bin:/usr/local/bin"),
            None,
            Platform::Unix,
            &probe(vec![at("/usr/local/bin", "coven")]),
        );
        assert_eq!(found.len(), 1, "a duplicated PATH entry is not a conflict");
        assert_eq!(conflict_report(&found), None);
    }

    #[test]
    fn unix_ignores_a_candidate_the_probe_rejects() {
        // The probe stands in for the executable-bit check the real one does.
        let found = installations_on_path(
            "coven",
            Some("/opt/broken/bin:/usr/bin"),
            None,
            Platform::Unix,
            &probe(vec![at("/usr/bin", "coven")]),
        );
        assert_eq!(rendered(&found), vec![at("/usr/bin", "coven")]);
    }

    #[test]
    fn windows_treats_a_case_differing_repeat_as_one_install() {
        // Windows PATH lookups are case-insensitive, so C:/Tools and C:/tools
        // name the same directory. A case-sensitive de-dupe would report one
        // file as two competing installs and send an operator hunting for a
        // second copy that does not exist.
        let exe = at("C:/tools", "coven.exe");
        let found = installations_on_path(
            "coven",
            Some("C:/Tools;C:/tools"),
            Some(".EXE"),
            Platform::Windows,
            &windows_probe(vec![exe]),
        );
        assert_eq!(
            found.len(),
            1,
            "same directory in two spellings is one install"
        );
        assert_eq!(conflict_report(&found), None);
    }

    #[test]
    fn unix_treats_a_case_differing_repeat_as_two_installs() {
        // Unix filesystems are case-sensitive, so /Opt/bin and /opt/bin really
        // are different directories and both count.
        let upper = at("/Opt/bin", "coven");
        let lower = at("/opt/bin", "coven");
        let found = installations_on_path(
            "coven",
            Some("/Opt/bin:/opt/bin"),
            None,
            Platform::Unix,
            &probe(vec![upper.clone(), lower.clone()]),
        );
        assert_eq!(rendered(&found), vec![upper, lower]);
        assert!(conflict_report(&found).is_some());
    }

    #[test]
    fn windows_splits_path_on_semicolons_and_applies_pathext() {
        let tools = at("C:/tools", "coven.exe");
        let npm = at("C:/npm", "coven.cmd");
        let found = installations_on_path(
            "coven",
            Some("C:/tools;C:/npm"),
            Some(".COM;.EXE;.BAT;.CMD"),
            Platform::Windows,
            &windows_probe(vec![tools.clone(), npm.clone()]),
        );
        assert_eq!(rendered(&found), vec![tools, npm]);
        assert!(conflict_report(&found).is_some());
    }

    #[test]
    fn windows_prefers_exe_over_cmd_in_the_same_directory() {
        // Both spellings in one directory is still two installs, and PATHEXT
        // order decides which one Windows actually runs.
        let exe = at("C:/npm", "coven.exe");
        let cmd = at("C:/npm", "coven.cmd");
        let found = installations_on_path(
            "coven",
            Some("C:/npm"),
            Some(".COM;.EXE;.BAT;.CMD"),
            Platform::Windows,
            &windows_probe(vec![cmd.clone(), exe.clone()]),
        );
        assert_eq!(rendered(&found), vec![exe.clone(), cmd.clone()]);
        let report = conflict_report(&found).expect("two spellings is a conflict");
        assert!(report.contains(&format!("{} (active)", exe.display())));
        assert!(report.contains(&format!("{} (shadowed)", cmd.display())));
    }

    #[test]
    fn windows_honors_a_reordered_pathext() {
        let exe = at("C:/npm", "coven.exe");
        let cmd = at("C:/npm", "coven.cmd");
        let found = installations_on_path(
            "coven",
            Some("C:/npm"),
            Some(".CMD;.EXE"),
            Platform::Windows,
            &windows_probe(vec![cmd.clone(), exe]),
        );
        assert_eq!(
            found[0].path, cmd,
            "PATHEXT order decides which spelling wins, not a hardcoded preference"
        );
    }

    #[test]
    fn windows_falls_back_to_the_default_pathext() {
        let exe = at("C:/tools", "coven.exe");
        for pathext in [None, Some(""), Some("   ")] {
            let found = installations_on_path(
                "coven",
                Some("C:/tools"),
                pathext,
                Platform::Windows,
                &windows_probe(vec![exe.clone()]),
            );
            assert_eq!(
                rendered(&found),
                vec![exe.clone()],
                "pathext {pathext:?} should fall back to the Windows default"
            );
        }
    }

    #[test]
    fn pathext_entries_without_a_leading_dot_still_match() {
        let exe = at("C:/tools", "coven.exe");
        let found = installations_on_path(
            "coven",
            Some("C:/tools"),
            Some("COM;EXE"),
            Platform::Windows,
            &windows_probe(vec![exe.clone()]),
        );
        assert_eq!(rendered(&found), vec![exe]);
    }

    #[test]
    fn an_absent_or_empty_path_reports_nothing() {
        let p = probe(vec![at("/usr/bin", "coven")]);
        assert!(installations_on_path("coven", None, None, Platform::Unix, &p).is_empty());
        assert!(installations_on_path("coven", Some(""), None, Platform::Unix, &p).is_empty());
        assert!(
            installations_on_path("coven", Some("::"), None, Platform::Unix, &p).is_empty(),
            "empty PATH segments must not be probed as the current directory"
        );
    }
}
