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

use std::path::{Component, Path, PathBuf};

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
    /// Where the executable really lives once symlinks are followed (equal to
    /// `path` when it is not a link). An npm shim is a link into the prefix's
    /// `node_modules`, and that target is what identifies the install.
    pub target: PathBuf,
    /// How it got there; `Unknown` until [`classify_all`] runs.
    pub origin: InstallOrigin,
}

/// How a `coven` executable got onto PATH. Each variant maps to one exact
/// removal command and, where the origin has an upgrade path, one exact
/// upgrade command -- "remove the others" is not actionable advice when the
/// operator does not know which tool put each copy there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOrigin {
    /// A global npm install. `prefix` is the npm prefix whose tree holds it,
    /// which is the one fact that makes `npm install -g` / `npm uninstall -g`
    /// target *this* copy rather than whichever prefix is active today.
    Npm {
        prefix: PathBuf,
    },
    /// `cargo install --path crates/coven-cli` into `$CARGO_HOME/bin`.
    Cargo,
    /// A `target/debug` or `target/release` build referenced straight from a
    /// source checkout.
    SourceBuild,
    /// `~/.coven-code/bin`, where older standalone coven-code installers put a
    /// `coven` alias.
    EngineInstaller,
    Unknown,
}

/// The environment `classify` needs, injected so every rule runs on every
/// platform in tests.
pub struct ClassifyContext<'a> {
    pub platform: Platform,
    /// `$CARGO_HOME`, defaulting to `~/.cargo`.
    pub cargo_home: Option<&'a Path>,
    /// `None` when the path is not a symlink.
    pub read_link: &'a dyn Fn(&Path) -> Option<PathBuf>,
    pub exists: &'a dyn Fn(&Path) -> bool,
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
                target: candidate.clone(),
                path: candidate,
                directory: directory.to_path_buf(),
                origin: InstallOrigin::Unknown,
            });
        }
    }
    found
}

/// Fill in `target` and `origin` for every install.
pub fn classify_all(installations: &mut [Installation], ctx: &ClassifyContext<'_>) {
    for installation in installations.iter_mut() {
        installation.target = follow_links(&installation.path, ctx.read_link);
        installation.origin = classify(&installation.target, &installation.directory, ctx);
    }
}

/// Decide how one executable was installed. `target` is the link-resolved
/// location; `directory` is the PATH entry it was found through.
pub fn classify(target: &Path, directory: &Path, ctx: &ClassifyContext<'_>) -> InstallOrigin {
    // Unix npm shims are symlinks into `<prefix>/lib/node_modules/@opencoven/cli`.
    if let Some(prefix) = npm_prefix_from_target(target) {
        return InstallOrigin::Npm { prefix };
    }
    // Windows npm shims (`coven.cmd`, `coven.ps1`, `coven`) are scripts, not
    // links, sitting directly in the prefix beside its `node_modules`. The
    // same probe also covers a Unix shim that was copied rather than linked.
    let beside = directory.join("node_modules");
    if (ctx.exists)(&npm_package_marker(&beside)) {
        return InstallOrigin::Npm {
            prefix: directory.to_path_buf(),
        };
    }
    if let Some(parent) = directory.parent() {
        let under_lib = parent.join("lib").join("node_modules");
        if (ctx.exists)(&npm_package_marker(&under_lib)) {
            return InstallOrigin::Npm {
                prefix: parent.to_path_buf(),
            };
        }
    }
    if is_source_build(target) {
        return InstallOrigin::SourceBuild;
    }
    let cargo_bin = ctx.cargo_home.map(|home| home.join("bin"));
    if cargo_bin.is_some_and(|bin| same_dir(directory, &bin, ctx.platform))
        || ends_with_components(directory, &[".cargo", "bin"])
    {
        return InstallOrigin::Cargo;
    }
    if ends_with_components(directory, &[".coven-code", "bin"]) {
        return InstallOrigin::EngineInstaller;
    }
    InstallOrigin::Unknown
}

fn npm_package_marker(node_modules: &Path) -> PathBuf {
    node_modules
        .join("@opencoven")
        .join("cli")
        .join("package.json")
}

/// The npm prefix that owns a path inside `node_modules/@opencoven/...`, or
/// `None` when the path is not in an @opencoven package tree. Unix prefixes
/// hold the tree under `lib/`; Windows prefixes hold it directly.
fn npm_prefix_from_target(target: &Path) -> Option<PathBuf> {
    let components: Vec<Component<'_>> = target.components().collect();
    let index = components.windows(2).position(|pair| {
        pair[0].as_os_str() == "node_modules" && pair[1].as_os_str() == "@opencoven"
    })?;
    let mut end = index;
    if end > 0 && components[end - 1].as_os_str() == "lib" {
        end -= 1;
    }
    let prefix: PathBuf = components[..end].iter().collect();
    if prefix.as_os_str().is_empty() {
        return None;
    }
    Some(prefix)
}

/// The `node_modules/@opencoven` directory that owns `path`, when it is inside
/// one. The wrapper shim and the native binary live in sibling packages under
/// it, so sharing this root is what ties the running process to its shim.
fn opencoven_root(path: &Path) -> Option<PathBuf> {
    let components: Vec<Component<'_>> = path.components().collect();
    let index = components.windows(2).position(|pair| {
        pair[0].as_os_str() == "node_modules" && pair[1].as_os_str() == "@opencoven"
    })?;
    Some(components[..=index + 1].iter().collect())
}

fn is_source_build(target: &Path) -> bool {
    target
        .components()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|pair| {
            pair[0].as_os_str() == "target"
                && (pair[1].as_os_str() == "debug" || pair[1].as_os_str() == "release")
        })
}

fn ends_with_components(path: &Path, suffix: &[&str]) -> bool {
    let components: Vec<Component<'_>> = path.components().collect();
    if components.len() < suffix.len() {
        return false;
    }
    components[components.len() - suffix.len()..]
        .iter()
        .zip(suffix)
        .all(|(component, expected)| component.as_os_str() == *expected)
}

/// Follow a symlink chain lexically. Relative targets resolve against the
/// link's own directory, exactly as the OS does, and the result is normalized
/// so `bin/../lib/x` reads as `lib/x`.
fn follow_links(path: &Path, read_link: &dyn Fn(&Path) -> Option<PathBuf>) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..16 {
        let Some(link_target) = read_link(&current) else {
            break;
        };
        let joined = if link_target.is_absolute() {
            link_target
        } else {
            current
                .parent()
                .map(|parent| parent.join(&link_target))
                .unwrap_or(link_target)
        };
        current = normalize(&joined);
    }
    current
}

/// Lexical normalization: collapse `.` and `..` without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let popped = matches!(out.components().next_back(), Some(Component::Normal(_)));
                if popped {
                    out.pop();
                } else if !matches!(
                    out.components().next_back(),
                    Some(Component::RootDir) | Some(Component::Prefix(_))
                ) {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn same_dir(a: &Path, b: &Path, platform: Platform) -> bool {
    let a = normalize(a);
    let b = normalize(b);
    match platform {
        Platform::Windows => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
        Platform::Unix => a == b,
    }
}

/// Index of the install that is this very process, if any. Direct hits compare
/// canonical paths; an npm install matches when the running native binary sits
/// under the same `node_modules/@opencoven` tree as the shim -- the shim is a
/// Node script, so `current_exe` never equals it.
pub fn running_index(
    installations: &[Installation],
    current_exe: Option<&Path>,
    canonical: &dyn Fn(&Path) -> Option<PathBuf>,
) -> Option<usize> {
    let exe = canonical(current_exe?)?;
    installations.iter().position(|installation| {
        if canonical(&installation.path).is_some_and(|path| path == exe) {
            return true;
        }
        if !matches!(installation.origin, InstallOrigin::Npm { .. }) {
            return false;
        }
        canonical(&installation.target)
            .and_then(|target| opencoven_root(&target))
            .is_some_and(|root| exe.starts_with(&root))
    })
}

/// Which install `npm install -g` would overwrite, given the prefix npm
/// reports as global. `None` when no install lives in that prefix -- including
/// when the prefix is not on PATH at all, which is its own diagnosis.
pub fn index_for_npm_prefix(
    installations: &[Installation],
    npm_prefix: &Path,
    platform: Platform,
    canonical: &dyn Fn(&Path) -> Option<PathBuf>,
) -> Option<usize> {
    let wanted = canonical(npm_prefix).unwrap_or_else(|| normalize(npm_prefix));
    installations.iter().position(|installation| {
        let InstallOrigin::Npm { prefix } = &installation.origin else {
            return false;
        };
        let have = canonical(prefix).unwrap_or_else(|| normalize(prefix));
        same_dir(&have, &wanted, platform)
    })
}

/// Short, path-free name for an origin. Safe for the JSON report, which
/// redacts every path on purpose.
pub fn origin_kind(origin: &InstallOrigin) -> &'static str {
    match origin {
        InstallOrigin::Npm { .. } => "npm",
        InstallOrigin::Cargo => "cargo install",
        InstallOrigin::SourceBuild => "source build",
        InstallOrigin::EngineInstaller => "legacy coven-code installer",
        InstallOrigin::Unknown => "unknown origin",
    }
}

/// Origin with the detail an operator needs to act on it (the npm prefix).
pub fn origin_label(origin: &InstallOrigin) -> String {
    match origin {
        InstallOrigin::Npm { prefix } => format!("npm, prefix {}", prefix.display()),
        other => origin_kind(other).to_string(),
    }
}

/// The exact command that removes this copy and nothing else. Every arm is
/// pasteable in the platform's shell; the trailing parenthetical is context,
/// not part of the command.
pub fn removal_command(installation: &Installation, platform: Platform) -> String {
    let delete = match platform {
        Platform::Windows => "del",
        Platform::Unix => "rm",
    };
    match &installation.origin {
        InstallOrigin::Npm { prefix } => {
            format!(
                "npm uninstall -g --prefix {} @opencoven/cli",
                shell_arg(prefix)
            )
        }
        InstallOrigin::Cargo => "cargo uninstall coven-cli".to_string(),
        InstallOrigin::SourceBuild => format!(
            "{delete} {} (a build from a source checkout; rebuilding recreates it)",
            shell_arg(&installation.path)
        ),
        InstallOrigin::EngineInstaller => format!(
            "{delete} {} (left by an older coven-code installer)",
            shell_arg(&installation.path)
        ),
        InstallOrigin::Unknown => format!(
            "find out how {} was installed, then remove it with that same tool",
            installation.path.display()
        ),
    }
}

/// The command that upgrades this copy in place. The npm form names the
/// prefix explicitly because a bare `npm install -g` writes to whichever
/// prefix is active, which is how shadowed installs come to exist.
pub fn upgrade_command(origin: &InstallOrigin) -> Option<String> {
    match origin {
        InstallOrigin::Npm { prefix } => Some(format!(
            "npm install -g --prefix {} @opencoven/cli@latest",
            shell_arg(prefix)
        )),
        InstallOrigin::Cargo => Some(
            "cargo install --path crates/coven-cli --force (from an up-to-date OpenCoven/coven checkout)"
                .to_string(),
        ),
        InstallOrigin::SourceBuild => {
            Some("rebuild the checkout it came from: cargo build -p coven-cli".to_string())
        }
        InstallOrigin::EngineInstaller | InstallOrigin::Unknown => None,
    }
}

/// Quote a path for pasting into sh, PowerShell, or cmd when it has spaces
/// (`C:\Users\First Last\AppData\Roaming\npm` is the common case).
fn shell_arg(path: &Path) -> String {
    let text = path.display().to_string();
    if text.chars().any(char::is_whitespace) {
        format!("\"{text}\"")
    } else {
        text
    }
}

/// The "Install"/"Installs" block of the doctor prose report.
///
/// `running` is the index of the install that is this process;
/// `npm_writes_to` is `npm prefix -g` paired with the index of the install in
/// that prefix (or `None` when no install lives there), supplied only when the
/// caller chose to ask npm.
pub fn doctor_lines(
    installations: &[Installation],
    running: Option<usize>,
    current_exe: Option<&Path>,
    npm_writes_to: Option<(&Path, Option<usize>)>,
    platform: Platform,
) -> Vec<String> {
    let mut lines = Vec::new();
    let exe = |lines: &mut Vec<String>, note: &str| {
        if let Some(exe) = current_exe {
            lines.push(format!("{note} {}", exe.display()));
        }
    };
    match installations {
        [] => {
            exe(
                &mut lines,
                "Install: none on PATH; this process was launched by explicit path:",
            );
            if current_exe.is_none() {
                lines.push("Install: none on PATH".to_string());
            }
        }
        [only] => {
            lines.push(format!(
                "Install: {} ({})",
                only.path.display(),
                origin_label(&only.origin)
            ));
            if running.is_none() {
                exe(
                    &mut lines,
                    "  [--] this process is not that install; it was launched by explicit path:",
                );
            }
        }
        many => {
            lines.push("Installs:".to_string());
            for (index, installation) in many.iter().enumerate() {
                let marker = if index == 0 { "OK" } else { "!!" };
                let role = match (index, running) {
                    (0, Some(0)) => "active, this process",
                    (0, _) => "active",
                    (_, Some(run)) if run == index => "shadowed, yet this process",
                    _ => "shadowed",
                };
                lines.push(format!(
                    "  [{marker}] {} ({role}) — {}",
                    installation.path.display(),
                    origin_label(&installation.origin)
                ));
                if index > 0 {
                    lines.push(format!(
                        "       remove: {}",
                        removal_command(installation, platform)
                    ));
                }
            }
            lines.push(
                "  The first entry wins. Keep one install per machine: remove the shadowed copies with the commands above, then re-check with `coven --version`."
                    .to_string(),
            );
            if running.is_none() {
                exe(
                    &mut lines,
                    "  This process is none of the entries above; it was launched by explicit path:",
                );
            }
            if let Some((prefix, index)) = npm_writes_to {
                let active_upgrade = upgrade_command(&many[0].origin);
                match index {
                    Some(0) => lines.push(format!(
                        "  `npm install -g` writes to {}, the active copy.",
                        prefix.display()
                    )),
                    Some(_) => lines.push(format!(
                        "  [!!] `npm install -g` writes to {}, a shadowed copy, so upgrades never reach the active install. {}",
                        prefix.display(),
                        upgrade_hint(active_upgrade.as_deref())
                    )),
                    None => lines.push(format!(
                        "  [!!] `npm install -g` writes to {}, which holds no install on PATH, so upgrades never reach the active install. {}",
                        prefix.display(),
                        upgrade_hint(active_upgrade.as_deref())
                    )),
                }
            }
        }
    }
    lines
}

fn upgrade_hint(active_upgrade: Option<&str>) -> String {
    match active_upgrade {
        Some(command) => format!("Upgrade the active copy with: {command}"),
        None => {
            "The active copy has no package manager; replace it by hand or remove it.".to_string()
        }
    }
}

/// Resolve against the real environment and filesystem.
pub fn current_installations(name: &str) -> Vec<Installation> {
    let path_var = std::env::var("PATH").ok();
    let pathext = std::env::var("PATHEXT").ok();
    let mut found = installations_on_path(
        name,
        path_var.as_deref(),
        pathext.as_deref(),
        Platform::current(),
        &is_runnable,
    );
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs_next::home_dir().map(|home| home.join(".cargo")));
    let read_link = |path: &Path| std::fs::read_link(path).ok();
    let exists = |path: &Path| path.exists();
    classify_all(
        &mut found,
        &ClassifyContext {
            platform: Platform::current(),
            cargo_home: cargo_home.as_deref(),
            read_link: &read_link,
            exists: &exists,
        },
    );
    found
}

/// `std::fs::canonicalize` as an `Option`, for the injected-canonicalizer
/// signatures above.
pub fn canonical(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

/// Whether `path` is a regular file the current process could execute.
///
/// Unix: at least one executable bit must be set. Elsewhere the executable
/// bit has no meaning and existence as a file is the whole test. Shared by
/// every PATH-style probe in the crate (engine, harness, setup, memory
/// dashboard) so they agree on what "runnable" means.
#[cfg(unix)]
pub(crate) fn is_runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// See the Unix variant.
#[cfg(not(unix))]
pub(crate) fn is_runnable(path: &Path) -> bool {
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

    // ---- origin classification ------------------------------------------

    fn no_link(_: &Path) -> Option<PathBuf> {
        None
    }

    fn nothing_exists(_: &Path) -> bool {
        false
    }

    fn ctx<'a>(
        platform: Platform,
        cargo_home: Option<&'a Path>,
        read_link: &'a dyn Fn(&Path) -> Option<PathBuf>,
        exists: &'a dyn Fn(&Path) -> bool,
    ) -> ClassifyContext<'a> {
        ClassifyContext {
            platform,
            cargo_home,
            read_link,
            exists,
        }
    }

    fn install(directory: &str, file: &str) -> Installation {
        Installation {
            path: at(directory, file),
            directory: PathBuf::from(directory),
            target: at(directory, file),
            origin: InstallOrigin::Unknown,
        }
    }

    #[test]
    fn a_unix_npm_shim_is_classified_by_its_relative_link_target() {
        // `npm install -g` links <prefix>/bin/coven -> ../lib/node_modules/@opencoven/cli/bin/coven.js
        let shim = at("/fixture/.local/bin", "coven");
        let read_link = move |path: &Path| {
            (path == shim).then(|| PathBuf::from("../lib/node_modules/@opencoven/cli/bin/coven.js"))
        };
        let mut found = vec![install("/fixture/.local/bin", "coven")];
        classify_all(
            &mut found,
            &ctx(Platform::Unix, None, &read_link, &nothing_exists),
        );
        assert_eq!(
            found[0].origin,
            InstallOrigin::Npm {
                prefix: PathBuf::from("/fixture/.local")
            }
        );
        assert_eq!(
            found[0].target,
            PathBuf::from("/fixture/.local/lib/node_modules/@opencoven/cli/bin/coven.js"),
            "the shim's real location is the link target, normalized"
        );
    }

    #[test]
    fn an_nvm_prefix_is_reported_per_node_version() {
        // The shape behind the nvm trap: every Node version is its own prefix.
        let shim = at("/fixture/.nvm/versions/node/v24.18.1/bin", "coven");
        let read_link = move |path: &Path| {
            (path == shim).then(|| PathBuf::from("../lib/node_modules/@opencoven/cli/bin/coven.js"))
        };
        let origin = classify(
            &follow_links(&shim_path(), &read_link),
            Path::new("/fixture/.nvm/versions/node/v24.18.1/bin"),
            &ctx(Platform::Unix, None, &read_link, &nothing_exists),
        );
        fn shim_path() -> PathBuf {
            at("/fixture/.nvm/versions/node/v24.18.1/bin", "coven")
        }
        assert_eq!(
            origin,
            InstallOrigin::Npm {
                prefix: PathBuf::from("/fixture/.nvm/versions/node/v24.18.1")
            }
        );
    }

    #[test]
    fn a_windows_npm_shim_is_classified_by_its_sibling_node_modules() {
        // %APPDATA%\npm\coven.cmd is a script, not a link; the package tree
        // sits right beside it.
        let marker = Path::new("C:/fixture/AppData/Roaming/npm")
            .join("node_modules")
            .join("@opencoven")
            .join("cli")
            .join("package.json");
        let exists = move |path: &Path| path == marker;
        let origin = classify(
            &at("C:/fixture/AppData/Roaming/npm", "coven.cmd"),
            Path::new("C:/fixture/AppData/Roaming/npm"),
            &ctx(Platform::Windows, None, &no_link, &exists),
        );
        assert_eq!(
            origin,
            InstallOrigin::Npm {
                prefix: PathBuf::from("C:/fixture/AppData/Roaming/npm")
            }
        );
    }

    #[test]
    fn a_copied_unix_shim_is_still_classified_through_the_lib_tree() {
        let marker = Path::new("/opt/node")
            .join("lib")
            .join("node_modules")
            .join("@opencoven")
            .join("cli")
            .join("package.json");
        let exists = move |path: &Path| path == marker;
        let origin = classify(
            &at("/opt/node/bin", "coven"),
            Path::new("/opt/node/bin"),
            &ctx(Platform::Unix, None, &no_link, &exists),
        );
        assert_eq!(
            origin,
            InstallOrigin::Npm {
                prefix: PathBuf::from("/opt/node")
            }
        );
    }

    #[test]
    fn a_cargo_home_bin_is_a_cargo_install() {
        let cargo_home = PathBuf::from("/fixture/.cargo");
        let origin = classify(
            &at("/fixture/.cargo/bin", "coven"),
            Path::new("/fixture/.cargo/bin"),
            &ctx(Platform::Unix, Some(&cargo_home), &no_link, &nothing_exists),
        );
        assert_eq!(origin, InstallOrigin::Cargo);

        // CARGO_HOME unknown: the conventional directory name still counts.
        let origin = classify(
            &at("C:/fixture/.cargo/bin", "coven.exe"),
            Path::new("C:/fixture/.cargo/bin"),
            &ctx(Platform::Windows, None, &no_link, &nothing_exists),
        );
        assert_eq!(origin, InstallOrigin::Cargo);
    }

    #[test]
    fn a_link_into_a_target_directory_is_a_source_build() {
        let link = at("/fixture/bin", "coven");
        let read_link = move |path: &Path| {
            (path == link).then(|| PathBuf::from("/fixture/src/coven/target/release/coven"))
        };
        let mut found = vec![install("/fixture/bin", "coven")];
        classify_all(
            &mut found,
            &ctx(Platform::Unix, None, &read_link, &nothing_exists),
        );
        assert_eq!(found[0].origin, InstallOrigin::SourceBuild);
    }

    #[test]
    fn the_legacy_engine_installer_directory_is_recognized() {
        let origin = classify(
            &at("/fixture/.coven-code/bin", "coven"),
            Path::new("/fixture/.coven-code/bin"),
            &ctx(Platform::Unix, None, &no_link, &nothing_exists),
        );
        assert_eq!(origin, InstallOrigin::EngineInstaller);
    }

    #[test]
    fn anything_else_is_unknown() {
        let origin = classify(
            &at("/usr/local/bin", "coven"),
            Path::new("/usr/local/bin"),
            &ctx(Platform::Unix, None, &no_link, &nothing_exists),
        );
        assert_eq!(origin, InstallOrigin::Unknown);
    }

    #[test]
    fn normalize_collapses_dot_and_dotdot_without_escaping_the_root() {
        assert_eq!(
            normalize(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
        assert_eq!(normalize(Path::new("/../x")), PathBuf::from("/x"));
        assert_eq!(normalize(Path::new("../x")), PathBuf::from("../x"));
    }

    // ---- which entry is this process ------------------------------------

    fn identity(path: &Path) -> Option<PathBuf> {
        Some(normalize(path))
    }

    #[test]
    fn the_running_npm_install_is_matched_through_its_package_tree() {
        // The process is the native binary in a sibling package of the shim.
        let mut shim = install("/fixture/.local/bin", "coven");
        shim.target = PathBuf::from("/fixture/.local/lib/node_modules/@opencoven/cli/bin/coven.js");
        shim.origin = InstallOrigin::Npm {
            prefix: PathBuf::from("/fixture/.local"),
        };
        let mut other = install("/fixture/.nvm/versions/node/v24/bin", "coven");
        other.target = PathBuf::from(
            "/fixture/.nvm/versions/node/v24/lib/node_modules/@opencoven/cli/bin/coven.js",
        );
        other.origin = InstallOrigin::Npm {
            prefix: PathBuf::from("/fixture/.nvm/versions/node/v24"),
        };
        let exe = PathBuf::from("/fixture/.local/lib/node_modules/@opencoven/cli-macos/bin/coven");
        assert_eq!(
            running_index(&[other.clone(), shim.clone()], Some(&exe), &identity),
            Some(1)
        );
        let exe = PathBuf::from(
            "/fixture/.nvm/versions/node/v24/lib/node_modules/@opencoven/cli-linux-x64/bin/coven",
        );
        assert_eq!(
            running_index(&[other, shim], Some(&exe), &identity),
            Some(0)
        );
    }

    #[test]
    fn a_direct_binary_matches_by_canonical_path_and_a_stranger_matches_nothing() {
        let mut cargo = install("/fixture/.cargo/bin", "coven");
        cargo.origin = InstallOrigin::Cargo;
        let exe = PathBuf::from("/fixture/.cargo/bin/coven");
        assert_eq!(
            running_index(&[cargo.clone()], Some(&exe), &identity),
            Some(0)
        );
        let exe = PathBuf::from("/fixture/src/coven/target/debug/coven");
        assert_eq!(running_index(&[cargo], Some(&exe), &identity), None);
        assert_eq!(running_index(&[], Some(&exe), &identity), None);
    }

    #[test]
    fn npm_prefix_lookup_is_case_insensitive_only_on_windows() {
        let mut shim = install("C:/fixture/AppData/Roaming/npm", "coven.cmd");
        shim.origin = InstallOrigin::Npm {
            prefix: PathBuf::from("C:/fixture/AppData/Roaming/npm"),
        };
        let reported = Path::new("c:/fixture/appdata/roaming/npm");
        assert_eq!(
            index_for_npm_prefix(&[shim.clone()], reported, Platform::Windows, &identity),
            Some(0)
        );
        assert_eq!(
            index_for_npm_prefix(&[shim], reported, Platform::Unix, &identity),
            None
        );
    }

    // ---- commands and prose ---------------------------------------------

    #[test]
    fn removal_and_upgrade_commands_target_the_exact_prefix() {
        let mut shim = install("/fixture/.nvm/versions/node/v24/bin", "coven");
        shim.origin = InstallOrigin::Npm {
            prefix: PathBuf::from("/fixture/.nvm/versions/node/v24"),
        };
        assert_eq!(
            removal_command(&shim, Platform::Unix),
            "npm uninstall -g --prefix /fixture/.nvm/versions/node/v24 @opencoven/cli"
        );
        assert_eq!(
            upgrade_command(&shim.origin).as_deref(),
            Some("npm install -g --prefix /fixture/.nvm/versions/node/v24 @opencoven/cli@latest")
        );

        let mut cargo = install("/fixture/.cargo/bin", "coven");
        cargo.origin = InstallOrigin::Cargo;
        assert_eq!(
            removal_command(&cargo, Platform::Unix),
            "cargo uninstall coven-cli"
        );
        assert!(upgrade_command(&cargo.origin)
            .unwrap()
            .starts_with("cargo install --path crates/coven-cli --force"));

        let mut stray = install("/usr/local/bin", "coven");
        stray.origin = InstallOrigin::Unknown;
        assert!(removal_command(&stray, Platform::Unix).contains("/usr/local/bin"));
        assert_eq!(upgrade_command(&stray.origin), None);
    }

    #[test]
    fn file_removals_use_the_platform_delete_command() {
        let mut build = install("/fixture/bin", "coven");
        build.origin = InstallOrigin::SourceBuild;
        assert!(removal_command(&build, Platform::Unix).starts_with("rm "));
        let mut legacy = install("C:/fixture/.coven-code/bin", "coven.exe");
        legacy.origin = InstallOrigin::EngineInstaller;
        assert!(removal_command(&legacy, Platform::Windows).starts_with("del "));
        let mut spaced = install("C:/fixture/My Tools", "coven.exe");
        spaced.origin = InstallOrigin::SourceBuild;
        assert!(
            removal_command(&spaced, Platform::Windows).starts_with("del \"C:/fixture/My Tools")
        );
    }

    #[test]
    fn a_prefix_with_spaces_is_quoted_for_pasting() {
        let mut shim = install("C:/fixture/First Last/AppData/Roaming/npm", "coven.cmd");
        shim.origin = InstallOrigin::Npm {
            prefix: PathBuf::from("C:/fixture/First Last/AppData/Roaming/npm"),
        };
        assert_eq!(
            removal_command(&shim, Platform::Unix),
            "npm uninstall -g --prefix \"C:/fixture/First Last/AppData/Roaming/npm\" @opencoven/cli"
        );
    }

    #[test]
    fn a_single_install_prints_its_origin_on_one_line() {
        let mut shim = install("/fixture/.local/bin", "coven");
        shim.origin = InstallOrigin::Npm {
            prefix: PathBuf::from("/fixture/.local"),
        };
        let lines = doctor_lines(
            &[shim],
            Some(0),
            Some(Path::new("/x")),
            None,
            Platform::Unix,
        );
        // Paths are rendered as joined, so the expectation joins the same way
        // and the assertion holds on both separators.
        assert_eq!(
            lines,
            vec![format!(
                "Install: {} (npm, prefix /fixture/.local)",
                at("/fixture/.local/bin", "coven").display()
            )]
        );
    }

    #[test]
    fn a_single_install_that_is_not_this_process_says_so() {
        let mut shim = install("/fixture/.local/bin", "coven");
        shim.origin = InstallOrigin::Npm {
            prefix: PathBuf::from("/fixture/.local"),
        };
        let exe = Path::new("/fixture/src/coven/target/debug/coven");
        let lines = doctor_lines(&[shim], None, Some(exe), None, Platform::Unix);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].starts_with("  [--] this process is not that install"));
        assert!(lines[1].ends_with("/fixture/src/coven/target/debug/coven"));
    }

    #[test]
    fn no_install_on_path_names_the_process() {
        let lines = doctor_lines(&[], None, Some(Path::new("/x/coven")), None, Platform::Unix);
        assert_eq!(
            lines,
            vec![
                "Install: none on PATH; this process was launched by explicit path: /x/coven"
                    .to_string()
            ]
        );
        assert_eq!(
            doctor_lines(&[], None, None, None, Platform::Unix),
            vec!["Install: none on PATH".to_string()]
        );
    }

    fn the_maintainer_machine() -> Vec<Installation> {
        let mut local = install("/fixture/.local/bin", "coven");
        local.origin = InstallOrigin::Npm {
            prefix: PathBuf::from("/fixture/.local"),
        };
        let mut nvm = install("/fixture/.nvm/versions/node/v24/bin", "coven");
        nvm.origin = InstallOrigin::Npm {
            prefix: PathBuf::from("/fixture/.nvm/versions/node/v24"),
        };
        let mut cargo = install("/fixture/.cargo/bin", "coven");
        cargo.origin = InstallOrigin::Cargo;
        vec![local, nvm, cargo]
    }

    #[test]
    fn a_conflict_lists_each_copy_with_its_removal_command() {
        let found = the_maintainer_machine();
        let lines = doctor_lines(&found, Some(0), Some(Path::new("/x")), None, Platform::Unix);
        assert_eq!(lines[0], "Installs:");
        assert_eq!(
            lines[1],
            format!(
                "  [OK] {} (active, this process) — npm, prefix /fixture/.local",
                at("/fixture/.local/bin", "coven").display()
            )
        );
        assert_eq!(
            lines[2],
            format!(
                "  [!!] {} (shadowed) — npm, prefix /fixture/.nvm/versions/node/v24",
                at("/fixture/.nvm/versions/node/v24/bin", "coven").display()
            )
        );
        assert_eq!(
            lines[3],
            "       remove: npm uninstall -g --prefix /fixture/.nvm/versions/node/v24 @opencoven/cli"
        );
        assert_eq!(
            lines[4],
            format!(
                "  [!!] {} (shadowed) — cargo install",
                at("/fixture/.cargo/bin", "coven").display()
            )
        );
        assert_eq!(lines[5], "       remove: cargo uninstall coven-cli");
        assert!(lines[6].starts_with("  The first entry wins."));
        assert_eq!(lines.len(), 7, "no npm line when npm was not asked");
    }

    #[test]
    fn a_shadowed_npm_prefix_is_called_out_with_the_prefix_explicit_upgrade() {
        let found = the_maintainer_machine();
        let npm = Path::new("/fixture/.nvm/versions/node/v24");
        let lines = doctor_lines(&found, Some(0), None, Some((npm, Some(1))), Platform::Unix);
        let last = lines.last().unwrap();
        assert!(last.starts_with(
            "  [!!] `npm install -g` writes to /fixture/.nvm/versions/node/v24, a shadowed copy"
        ));
        assert!(last.ends_with(
            "Upgrade the active copy with: npm install -g --prefix /fixture/.local @opencoven/cli@latest"
        ));
    }

    #[test]
    fn an_npm_prefix_that_is_the_active_copy_is_reassuring() {
        let found = the_maintainer_machine();
        let npm = Path::new("/fixture/.local");
        let lines = doctor_lines(&found, Some(0), None, Some((npm, Some(0))), Platform::Unix);
        assert_eq!(
            lines.last().unwrap(),
            "  `npm install -g` writes to /fixture/.local, the active copy."
        );
    }

    #[test]
    fn an_npm_prefix_off_path_and_a_non_npm_active_copy_are_both_explained() {
        let mut found = the_maintainer_machine();
        found.rotate_right(1); // cargo first
        let npm = Path::new("/opt/homebrew");
        let lines = doctor_lines(&found, Some(0), None, Some((npm, None)), Platform::Unix);
        let last = lines.last().unwrap();
        assert!(last.starts_with(
            "  [!!] `npm install -g` writes to /opt/homebrew, which holds no install on PATH"
        ));
        assert!(last.contains("cargo install --path crates/coven-cli --force"));
    }

    #[test]
    fn a_process_that_is_a_shadowed_copy_or_no_copy_is_named() {
        let found = the_maintainer_machine();
        let lines = doctor_lines(&found, Some(2), None, None, Platform::Unix);
        assert_eq!(
            lines[4],
            format!(
                "  [!!] {} (shadowed, yet this process) — cargo install",
                at("/fixture/.cargo/bin", "coven").display()
            )
        );
        let lines = doctor_lines(
            &found,
            None,
            Some(Path::new("/x/target/debug/coven")),
            None,
            Platform::Unix,
        );
        assert!(lines
            .iter()
            .any(|line| line == "  This process is none of the entries above; it was launched by explicit path: /x/target/debug/coven"));
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
        assert_eq!(rendered(&found), vec![exe, cmd]);
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
