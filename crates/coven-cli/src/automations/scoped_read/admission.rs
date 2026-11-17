//! Admission for a launch under a scoped read envelope (`r0-read.2`): what the
//! daemon checks and prepares before the harness starts (coven#857, step 2).
//!
//! - The installed harness must report one of the versions the envelope was
//!   proven against.
//! - The owner's API key must be saved ([`super::credential`]).
//! - The harness starts in an empty folder with a home of its own. Both sit
//!   in a new private folder under the system's temporary folder, never under
//!   `COVEN_HOME`: the harness writes into its home, and the whole folder is
//!   removed when the launch ends.
//! - Its environment holds only the variables the envelope names. The launch
//!   clears everything else.
//! - The reader protects `COVEN_HOME`, the launch folder, and the owner's
//!   harness configuration and credential folders.
//! - The attempt's channel is opened, and the harness is pointed at its MCP
//!   configuration.

use std::fmt;
use std::fs::{self, DirBuilder};
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::channel::Channel;
use super::credential;
use super::open::ScopedReader;
use super::scope::RootKind;
use super::service::{Gate, Limits, Operation, Service};
use super::Refusal;
use crate::automations::runtime_envelope::{RuntimeEnvelope, ServiceBinding};

/// How long the harness has to report its version.
const VERSION_TIME: Duration = Duration::from_secs(10);
/// The most version output read.
const VERSION_BYTES: u64 = 4096;
/// The owner's harness configuration and credential folders and files,
/// relative to their home. The reader refuses each, and anything beneath.
const OWNER_PROTECTED: &[&str] = &[
    ".aws",
    ".claude",
    ".claude.json",
    ".codex",
    ".config/claude",
    ".config/gh",
    ".docker",
    ".gemini",
    ".gnupg",
    ".netrc",
    ".ssh",
];

/// Why a scoped read launch was not admitted. Each message says what to do.
#[derive(Debug)]
pub(crate) enum AdmissionError {
    /// The envelope has no scoped read service.
    NotScoped,
    VersionUnreadable(String),
    VersionNotAdmitted {
        found: String,
        admitted: &'static [&'static str],
    },
    KeyMissing,
    /// `COVEN_HOME` is too long for the channel's socket path.
    HomeTooLong {
        bytes: usize,
        limit: usize,
    },
    /// The grant or the project folder cannot be served.
    Grant(Refusal),
    Setup(anyhow::Error),
}

impl fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotScoped => write!(f, "this envelope does not run under the scoped read service"),
            Self::VersionUnreadable(why) => write!(
                f,
                "could not read the installed Claude Code version ({why}); check that `claude --version` works"
            ),
            Self::VersionNotAdmitted { found, admitted } => write!(
                f,
                "Claude Code {found} has not been checked for Runtime Authority runs; this Coven admits {}",
                admitted.join(", ")
            ),
            Self::KeyMissing => write!(
                f,
                "Runtime Authority runs need the owner's Anthropic API key; save it with `coven automations api-key set`"
            ),
            Self::HomeTooLong { bytes, limit } => write!(
                f,
                "the scoped read channel's socket path would be {bytes} bytes, and this platform allows {limit}; use a shorter COVEN_HOME"
            ),
            Self::Grant(refusal) => write!(
                f,
                "the grant cannot be served over this project folder ({})",
                refusal.code()
            ),
            Self::Setup(error) => write!(f, "could not prepare the launch: {error:#}"),
        }
    }
}

impl std::error::Error for AdmissionError {}

impl From<anyhow::Error> for AdmissionError {
    fn from(error: anyhow::Error) -> Self {
        Self::Setup(error)
    }
}

/// What a scoped read launch needs, from the daemon.
pub(crate) struct Request<'a> {
    pub coven_home: &'a Path,
    pub envelope: &'static RuntimeEnvelope,
    /// The resolved harness executable.
    pub program: &'a Path,
    /// The `coven` executable the harness launches as the relay.
    pub relay: &'a Path,
    /// The grant's validated profile scopes.
    pub scopes: &'a [Value],
    /// The project folder the scopes are relative to.
    pub project: &'a Path,
    /// The owner's home, whose harness folders are protected.
    pub owner_home: Option<&'a Path>,
    /// When the run ends, if it has a deadline.
    pub deadline: Option<Instant>,
}

/// An admitted launch, ready to start. Dropping it, or [`Prepared::finish`],
/// closes the channel and removes the launch folder.
pub(crate) struct Prepared {
    pub program: PathBuf,
    /// The envelope's arguments and `--mcp-config`. The model, the familiar's
    /// preamble and `-- <prompt>` follow.
    pub args: Vec<String>,
    /// The harness's whole environment.
    pub environment: Vec<(String, String)>,
    pub cwd: PathBuf,
    /// The version the harness reported.
    pub version: String,
    channel: Option<Channel>,
    launch: IsolatedLaunch,
}

impl Prepared {
    /// Ends the launch: closes the channel, returns what each tool call did,
    /// and removes the launch folder.
    pub(crate) fn finish(mut self) -> Vec<Operation> {
        self.channel.take().map(Channel::close).unwrap_or_default()
    }

    pub(crate) fn launch_root(&self) -> &Path {
        self.launch.root()
    }
}

/// Admits and prepares one launch. Only the automations module calls it: the
/// daemon's one caller is `trusted_launch::prepare`, which supplies the
/// store gate.
pub(in crate::automations) fn prepare(
    request: Request<'_>,
    gate: Box<dyn Gate>,
) -> Result<Prepared, AdmissionError> {
    let envelope = request.envelope;
    let service = envelope.service.as_ref().ok_or(AdmissionError::NotScoped)?;
    let launch = IsolatedLaunch::create(request.coven_home)?;
    let version = installed_version(request.program, &launch.home(), VERSION_TIME)?;
    if !envelope.harness_versions.contains(&version.as_str()) {
        return Err(AdmissionError::VersionNotAdmitted {
            found: version,
            admitted: envelope.harness_versions,
        });
    }
    let key = credential::load(request.coven_home)?.ok_or(AdmissionError::KeyMissing)?;
    let environment = environment(service, request.program, &launch.home(), &key)?;
    let protected = protected_paths(request.coven_home, request.owner_home, launch.root());
    let protected: Vec<&Path> = protected.iter().map(PathBuf::as_path).collect();
    let reader = ScopedReader::new(
        request.scopes,
        &[(RootKind::Project, request.project)],
        &protected,
        service.policy,
    )
    .map_err(AdmissionError::Grant)?;
    let limits = Limits {
        operation: service.operation_time,
        deadline: request.deadline,
    };
    let channel = Channel::open(
        &super::channel::runtime_dir(request.coven_home),
        request.relay,
        |gate| Service::new(reader, gate, limits),
        gate,
    )?;
    let mut args: Vec<String> = envelope.args.iter().map(|arg| (*arg).to_owned()).collect();
    args.push("--mcp-config".to_owned());
    args.push(text(channel.config_path())?);
    Ok(Prepared {
        program: request.program.to_path_buf(),
        args,
        environment,
        cwd: launch.cwd(),
        version,
        channel: Some(channel),
        launch,
    })
}

/// What admission checks that needs no grant: the envelope runs the scoped
/// read service, `COVEN_HOME` leaves room for the channel's socket path,
/// the installed harness reports an admitted version, and the owner's API
/// key is saved. The trusted adapter runs this before it decides
/// and consumes a Threads decision, so a refusal here burns none. Returns
/// the version the harness reported.
pub(crate) fn preflight(
    coven_home: &Path,
    envelope: &'static RuntimeEnvelope,
    program: &Path,
) -> Result<String, AdmissionError> {
    envelope.service.as_ref().ok_or(AdmissionError::NotScoped)?;
    let (bytes, limit) = super::channel::socket_path_bytes(coven_home);
    if bytes > limit {
        return Err(AdmissionError::HomeTooLong { bytes, limit });
    }
    let launch = IsolatedLaunch::create(coven_home)?;
    let version = installed_version(program, &launch.home(), VERSION_TIME)?;
    if !envelope.harness_versions.contains(&version.as_str()) {
        return Err(AdmissionError::VersionNotAdmitted {
            found: version,
            admitted: envelope.harness_versions,
        });
    }
    credential::load(coven_home)?.ok_or(AdmissionError::KeyMissing)?;
    Ok(version)
}

/// The version `program --version` reports, run with nothing in its
/// environment but its own folder on `PATH`, `home` and no auto-update.
fn installed_version(
    program: &Path,
    home: &Path,
    limit: Duration,
) -> Result<String, AdmissionError> {
    let unreadable = |why: &str| AdmissionError::VersionUnreadable(why.to_owned());
    let mut child = Command::new(program)
        .arg("--version")
        .env_clear()
        .env("HOME", home)
        .env("PATH", search_path(program)?)
        .env("DISABLE_AUTOUPDATER", "1")
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| unreadable(&error.to_string()))?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let reading = thread::spawn(move || {
        let mut output = String::new();
        let _ = (&mut stdout)
            .take(VERSION_BYTES)
            .read_to_string(&mut output);
        output
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| unreadable(&error.to_string()))?
        {
            break status;
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err(unreadable("it did not answer in time"));
        }
        thread::sleep(Duration::from_millis(20));
    };
    let output = reading.join().unwrap_or_default();
    if !status.success() {
        return Err(unreadable("it exited with an error"));
    }
    let version = output.split_whitespace().next().unwrap_or_default();
    let numeric = version.split('.').count() == 3
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    if !numeric {
        return Err(unreadable("it printed no version number"));
    }
    Ok(version.to_owned())
}

/// The harness's environment: exactly the envelope's variables. A name the
/// daemon cannot fill refuses the launch rather than being left out.
fn environment(
    service: &ServiceBinding,
    program: &Path,
    home: &Path,
    key: &str,
) -> Result<Vec<(String, String)>, AdmissionError> {
    service
        .environment
        .iter()
        .map(|name| {
            let value = match *name {
                "ANTHROPIC_API_KEY" => key.to_owned(),
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC" | "DISABLE_AUTOUPDATER" => {
                    "1".to_owned()
                }
                "HOME" => text(home)?,
                "PATH" => search_path(program)?,
                other => {
                    return Err(AdmissionError::Setup(anyhow::anyhow!(
                        "the envelope names an environment variable the daemon cannot fill: {other}"
                    )))
                }
            };
            Ok(((*name).to_owned(), value))
        })
        .collect()
}

/// `PATH` for the harness: its own folder, then the system's.
fn search_path(program: &Path) -> Result<String, AdmissionError> {
    match program
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
    {
        Some(folder) => Ok(format!("{}:/usr/bin:/bin", text(folder)?)),
        None => Ok("/usr/bin:/bin".to_owned()),
    }
}

/// What the reader refuses beyond the grant's own exclusions.
fn protected_paths(coven_home: &Path, owner_home: Option<&Path>, launch: &Path) -> Vec<PathBuf> {
    let mut paths = vec![coven_home.to_path_buf(), launch.to_path_buf()];
    if let Some(home) = owner_home {
        paths.extend(OWNER_PROTECTED.iter().map(|entry| home.join(entry)));
    }
    paths
}

fn text(path: &Path) -> Result<String, AdmissionError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| AdmissionError::Setup(anyhow::anyhow!("{} is not UTF-8", path.display())))
}

/// A launch's own folder: an empty working folder and a home for the
/// harness, removed whole when the launch ends.
///
/// Its name carries a digest of `COVEN_HOME`, and it holds a marker naming the
/// same home, locked for the folder's life. A folder a crashed daemon left
/// behind is therefore positively identifiable, and its free lock shows that
/// no launch owns it: [`super::launch_reaper`] removes it at the next startup.
pub(crate) struct IsolatedLaunch {
    // Declared first, so it is dropped first: the folder is removed while
    // the marker is still locked.
    root: tempfile::TempDir,
    _marker: fs::File,
}

impl IsolatedLaunch {
    pub(crate) fn create(coven_home: &Path) -> anyhow::Result<Self> {
        Self::create_in(&std::env::temp_dir(), coven_home)
    }

    /// Creates the folder beneath `base`. Because it is removed whole, it may
    /// never sit inside `coven_home`, which holds the owner's state.
    pub(super) fn create_in(base: &Path, coven_home: &Path) -> anyhow::Result<Self> {
        let base = fs::canonicalize(base)?;
        let home = fs::canonicalize(coven_home).unwrap_or_else(|_| coven_home.to_path_buf());
        if base.starts_with(&home) {
            anyhow::bail!(
                "refusing to place a launch folder in {}, which is inside COVEN_HOME",
                base.display()
            );
        }
        let digest = super::launch_reaper::home_digest(coven_home);
        let root = tempfile::Builder::new()
            .prefix(&super::launch_reaper::folder_prefix(&digest))
            .tempdir_in(&base)?;
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))?;
        let marker = super::launch_reaper::mark(root.path(), &digest)?;
        for folder in ["cwd", "home"] {
            DirBuilder::new()
                .mode(0o700)
                .create(root.path().join(folder))?;
        }
        Ok(Self {
            root,
            _marker: marker,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        self.root.path()
    }

    pub(crate) fn cwd(&self) -> PathBuf {
        self.root.path().join("cwd")
    }

    pub(crate) fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    /// Simulates a crash: the folder stays, and its marker lock is released
    /// as a dead process's would be.
    #[cfg(test)]
    pub(crate) fn abandon(self) -> PathBuf {
        let Self { root, _marker } = self;
        drop(_marker);
        root.keep()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use serde_json::json;

    use super::*;
    use crate::automations::runtime_envelope::{CLAUDE_R0_READ, CLAUDE_R0_SCOPED_READ};
    use crate::automations::scoped_read::service::Open;

    /// A short scratch base: the channel's socket path has to fit in about
    /// 100 bytes.
    fn scratch() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("csa")
            .tempdir_in("/tmp")
            .expect("scratch directory")
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// A stand-in harness that prints `version` and records its environment
    /// in its home.
    fn fake_claude(base: &Path, version: &str) -> PathBuf {
        let program = base.join("bin/claude");
        fs::create_dir_all(program.parent().unwrap()).unwrap();
        fs::write(
            &program,
            format!("#!/bin/sh\n/usr/bin/env > \"$HOME/version-env\"\necho '{version}'\n"),
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        program
    }

    fn scoped() -> &'static ServiceBinding {
        CLAUDE_R0_SCOPED_READ.service.as_ref().unwrap()
    }

    #[test]
    fn the_version_comes_from_a_scrubbed_run() {
        let scratch = scratch();
        let home = scratch.path().join("home");
        fs::create_dir(&home).unwrap();
        let program = fake_claude(scratch.path(), "2.1.305 (Claude Code)");
        assert_eq!(
            installed_version(&program, &home, VERSION_TIME).unwrap(),
            "2.1.305"
        );
        let seen = fs::read_to_string(home.join("version-env")).unwrap();
        let mut names: Vec<&str> = seen
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name))
            .filter(|name| *name != "PWD" && *name != "SHLVL" && *name != "_")
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["DISABLE_AUTOUPDATER", "HOME", "PATH"], "{seen}");

        for output in ["Claude Code", "", "2.1 (Claude Code)", "v2.1.305", "2.1.x"] {
            let program = fake_claude(scratch.path(), output);
            assert!(
                matches!(
                    installed_version(&program, &home, VERSION_TIME),
                    Err(AdmissionError::VersionUnreadable(_))
                ),
                "{output:?}"
            );
        }
    }

    #[test]
    fn a_harness_that_does_not_answer_is_unreadable() {
        let scratch = scratch();
        let program = scratch.path().join("slow");
        fs::write(&program, "#!/bin/sh\nsleep 30\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let started = Instant::now();
        assert!(matches!(
            installed_version(&program, scratch.path(), Duration::from_millis(200)),
            Err(AdmissionError::VersionUnreadable(_))
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_environment_is_exactly_the_envelopes() {
        let program = Path::new("/opt/claude/bin/claude");
        let home = Path::new("/tmp/launch/home");
        let variables = environment(scoped(), program, home, "sk-ant-test").unwrap();
        let names: Vec<&str> = variables.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, scoped().environment);
        let value = |name: &str| {
            variables
                .iter()
                .find(|(each, _)| each == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(value("ANTHROPIC_API_KEY"), Some("sk-ant-test"));
        assert_eq!(value("HOME"), Some("/tmp/launch/home"));
        assert_eq!(value("PATH"), Some("/opt/claude/bin:/usr/bin:/bin"));
        assert_eq!(value("DISABLE_AUTOUPDATER"), Some("1"));
        assert_eq!(value("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"), Some("1"));

        let widened = ServiceBinding {
            environment: &["HOME", "ANTHROPIC_BASE_URL"],
            ..*scoped()
        };
        assert!(environment(&widened, program, home, "sk-ant-test").is_err());
    }

    #[test]
    fn a_launch_folder_is_private_removed_whole_and_never_in_coven_home() {
        let scratch = scratch();
        let coven_home = scratch.path().join("coven");
        fs::create_dir(&coven_home).unwrap();
        let launch = IsolatedLaunch::create_in(scratch.path(), &coven_home).unwrap();
        let root = launch.root().to_path_buf();
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&launch.cwd()), 0o700);
        assert_eq!(mode(&launch.home()), 0o700);
        assert_eq!(fs::read_dir(launch.cwd()).unwrap().count(), 0);
        fs::create_dir_all(launch.home().join(".claude/projects")).unwrap();
        fs::write(launch.home().join(".claude.json"), "{}").unwrap();
        drop(launch);
        assert!(!root.exists());
        assert!(coven_home.is_dir());

        let inside = coven_home.join("tmp");
        fs::create_dir(&inside).unwrap();
        assert!(IsolatedLaunch::create_in(&inside, &coven_home).is_err());
        assert_eq!(fs::read_dir(&inside).unwrap().count(), 0);
    }

    #[test]
    fn the_owners_harness_folders_and_coven_home_are_protected() {
        let paths = protected_paths(
            Path::new("/srv/owner/.coven"),
            Some(Path::new("/srv/owner")),
            Path::new("/tmp/coven-launch-1"),
        );
        for expected in [
            "/srv/owner/.coven",
            "/tmp/coven-launch-1",
            "/srv/owner/.claude",
            "/srv/owner/.claude.json",
            "/srv/owner/.codex",
            "/srv/owner/.ssh",
        ] {
            assert!(paths.contains(&PathBuf::from(expected)), "{expected}");
        }
    }

    /// A scratch Coven home with a saved key, a project with a granted note,
    /// and a stand-in harness reporting `version`.
    struct Setup {
        scratch: tempfile::TempDir,
        coven_home: PathBuf,
        project: PathBuf,
        program: PathBuf,
    }

    impl Setup {
        fn new(version: &str) -> Self {
            let scratch = scratch();
            let base = scratch.path();
            let (coven_home, project) = (base.join("coven"), base.join("project"));
            fs::create_dir_all(&coven_home).unwrap();
            fs::create_dir_all(project.join("notes")).unwrap();
            fs::write(project.join("notes/today.md"), "granted notes").unwrap();
            credential::store(&coven_home, "sk-ant-test-capture-only").unwrap();
            let program = fake_claude(base, version);
            Self {
                scratch,
                coven_home,
                project,
                program,
            }
        }

        fn request<'a>(
            &'a self,
            envelope: &'static RuntimeEnvelope,
            scopes: &'a [Value],
        ) -> Request<'a> {
            Request {
                coven_home: &self.coven_home,
                envelope,
                program: &self.program,
                relay: Path::new("/usr/local/bin/coven"),
                scopes,
                project: &self.project,
                owner_home: None,
                deadline: None,
            }
        }
    }

    fn note_scope() -> Vec<Value> {
        vec![
            json!({"kind": "filesystem", "root": "project", "path": "notes/today.md", "access": "read", "recursive": false}),
        ]
    }

    #[test]
    fn an_admitted_launch_is_pointed_at_its_channel() {
        let setup = Setup::new("2.1.305 (Claude Code)");
        let scopes = note_scope();
        let prepared = prepare(
            setup.request(&CLAUDE_R0_SCOPED_READ, &scopes),
            Box::new(Open),
        )
        .unwrap();
        assert_eq!(prepared.version, "2.1.305");
        assert_eq!(
            &prepared.args[..CLAUDE_R0_SCOPED_READ.args.len()],
            CLAUDE_R0_SCOPED_READ.args
        );
        let tail = &prepared.args[CLAUDE_R0_SCOPED_READ.args.len()..];
        assert_eq!(tail[0], "--mcp-config");
        let config = PathBuf::from(&tail[1]);
        assert!(config.starts_with(setup.coven_home.join("runtime/scoped-read")));
        assert!(config.is_file());
        assert_eq!(prepared.cwd, prepared.launch_root().join("cwd"));
        assert!(!prepared.launch_root().starts_with(&setup.coven_home));
        let root = prepared.launch_root().to_path_buf();
        assert!(prepared.finish().is_empty());
        assert!(!root.exists(), "the launch folder goes with the launch");
        assert!(!config.exists(), "and so does the channel");
    }

    #[test]
    fn a_launch_is_refused_before_it_starts() {
        let scopes = note_scope();
        let refused = |setup: &Setup, envelope| {
            prepare(setup.request(envelope, &scopes), Box::new(Open))
                .err()
                .expect("refused")
        };
        let setup = Setup::new("2.1.305 (Claude Code)");
        assert!(matches!(
            refused(&setup, &CLAUDE_R0_READ),
            AdmissionError::NotScoped
        ));

        let newer = Setup::new("2.1.306 (Claude Code)");
        let error = refused(&newer, &CLAUDE_R0_SCOPED_READ);
        assert!(matches!(error, AdmissionError::VersionNotAdmitted { .. }));
        assert_eq!(
            error.to_string(),
            "Claude Code 2.1.306 has not been checked for Runtime Authority runs; this Coven admits 2.1.305"
        );

        let keyless = Setup::new("2.1.305 (Claude Code)");
        credential::remove(&keyless.coven_home).unwrap();
        assert!(matches!(
            refused(&keyless, &CLAUDE_R0_SCOPED_READ),
            AdmissionError::KeyMissing
        ));

        // A project inside COVEN_HOME is protected as a whole.
        let inside = Setup::new("2.1.305 (Claude Code)");
        let mut request = inside.request(&CLAUDE_R0_SCOPED_READ, &scopes);
        request.project = request.coven_home;
        assert!(matches!(
            prepare(request, Box::new(Open)),
            Err(AdmissionError::Grant(Refusal::RootProtected))
        ));
        let runtime = inside.coven_home.join("runtime/scoped-read");
        assert!(
            !runtime.exists() || fs::read_dir(&runtime).unwrap().count() == 0,
            "a refused launch leaves no channel"
        );
    }
}
