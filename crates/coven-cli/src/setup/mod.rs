pub mod claude;
pub mod codex;
pub mod copilot;
mod process;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use process::{wait_bounded, BoundedProcessResult};
pub use process::{
    Clock, LaunchRequest, ProcessExit, ProcessLauncher, RunningProcess, SystemClock,
    SystemProcessLauncher,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Codex,
    Claude,
    Copilot,
}

impl ProviderId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Copilot => "copilot",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Copilot => "GitHub Copilot CLI",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum Selector {
    Codex,
    Claude,
    Copilot,
    All,
}

impl Selector {
    fn providers(self) -> &'static [ProviderId] {
        match self {
            Self::Codex => &[ProviderId::Codex],
            Self::Claude => &[ProviderId::Claude],
            Self::Copilot => &[ProviderId::Copilot],
            Self::All => &[ProviderId::Codex, ProviderId::Claude, ProviderId::Copilot],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupMode {
    Login,
    LoginAndVerify,
    VerifyOnly,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    args: Vec<OsString>,
}

impl CommandSpec {
    pub fn new<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    pub executable: String,
    pub official_install_guidance: String,
    login: Option<CommandSpec>,
    verification: Option<CommandSpec>,
}

impl ProviderDescriptor {
    pub fn new(
        id: ProviderId,
        executable: impl Into<String>,
        official_install_guidance: impl Into<String>,
    ) -> Self {
        Self {
            id,
            executable: executable.into(),
            official_install_guidance: official_install_guidance.into(),
            login: None,
            verification: None,
        }
    }

    pub fn with_login(mut self, command: CommandSpec) -> Self {
        self.login = Some(command);
        self
    }

    pub fn with_verification(mut self, command: CommandSpec) -> Self {
        self.verification = Some(command);
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedExecutable {
    pub path: PathBuf,
    pub version: Option<String>,
}

pub trait ExecutableDiscovery {
    fn discover(&self, provider: &ProviderDescriptor) -> io::Result<Option<ResolvedExecutable>>;
}

#[derive(Clone, Debug)]
pub struct SystemExecutableDiscovery {
    path: Option<OsString>,
    pathext: Option<OsString>,
}

impl SystemExecutableDiscovery {
    pub fn from_environment() -> Self {
        Self::from_path(std::env::var_os("PATH"), std::env::var_os("PATHEXT"))
    }

    pub fn from_path(path: Option<OsString>, pathext: Option<OsString>) -> Self {
        Self { path, pathext }
    }
}

impl ExecutableDiscovery for SystemExecutableDiscovery {
    fn discover(&self, provider: &ProviderDescriptor) -> io::Result<Option<ResolvedExecutable>> {
        let executable = Path::new(&provider.executable);
        if executable.components().count() > 1 {
            return Ok(
                executable_is_runnable(executable).then(|| ResolvedExecutable {
                    path: executable.to_path_buf(),
                    version: None,
                }),
            );
        }
        let Some(path) = self.path.as_deref() else {
            return Ok(None);
        };
        for directory in std::env::split_paths(path) {
            for name in executable_names(&provider.executable, self.pathext.as_deref()) {
                let candidate = directory.join(name);
                if executable_is_runnable(&candidate) {
                    return Ok(Some(ResolvedExecutable {
                        path: candidate,
                        version: None,
                    }));
                }
            }
        }
        Ok(None)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupAction {
    Login,
    Verification,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsentRequest {
    pub provider: ProviderId,
    pub action: SetupAction,
    pub command: String,
    pub notice: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsentDecision {
    Accepted,
    Declined,
    Cancelled,
}

pub trait Confirmer {
    fn confirm(&mut self, request: &ConsentRequest) -> io::Result<ConsentDecision>;
}

pub struct SystemConfirmer;

impl Confirmer for SystemConfirmer {
    fn confirm(&mut self, request: &ConsentRequest) -> io::Result<ConsentDecision> {
        let action = match request.action {
            SetupAction::Login => "login",
            SetupAction::Verification => "verification",
        };
        let mut stderr = io::stderr().lock();
        let stdin = io::stdin();
        loop {
            writeln!(stderr, "{}", request.notice)?;
            write!(
                stderr,
                "{} {action} will launch `{}` with direct terminal access. Continue? [y/N] ",
                request.provider.label(),
                request.command
            )?;
            stderr.flush()?;
            let mut answer = String::new();
            match stdin.read_line(&mut answer) {
                Ok(0) => return Ok(ConsentDecision::Cancelled),
                Ok(_) => match answer.trim().to_ascii_lowercase().as_str() {
                    "y" | "yes" => return Ok(ConsentDecision::Accepted),
                    "n" | "no" | "" => return Ok(ConsentDecision::Declined),
                    _ => continue,
                },
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    return Ok(ConsentDecision::Cancelled);
                }
                Err(error) => return Err(error),
            }
        }
    }
}

pub trait TerminalState {
    fn is_interactive(&self) -> bool;
}

pub struct SystemTerminalState;

impl TerminalState for SystemTerminalState {
    fn is_interactive(&self) -> bool {
        io::stdin().is_terminal() && io::stdout().is_terminal() && io::stderr().is_terminal()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Completed,
    NotInstalled,
    Declined,
    Cancelled,
    ProviderFailed,
    TimedOut,
    NonTty,
    VerificationFailed,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProviderResult {
    pub provider: ProviderId,
    pub outcome: Outcome,
    duration: Duration,
    version: Option<String>,
    install_guidance: Option<String>,
}

impl fmt::Debug for ProviderResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderResult")
            .field("provider", &self.provider)
            .field("outcome", &self.outcome)
            .field("duration", &self.duration)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetupSummary {
    pub results: Vec<ProviderResult>,
}

impl SetupSummary {
    pub fn completed(&self) -> bool {
        !self.results.is_empty()
            && self
                .results
                .iter()
                .all(|result| result.outcome == Outcome::Completed)
    }
}

pub struct SetupOptions {
    pub selector: Selector,
    pub mode: SetupMode,
    pub timeout: Duration,
    pub report_json: Option<PathBuf>,
    pub candidate_commit: String,
}

pub struct SetupRuntime<'a> {
    pub discovery: &'a dyn ExecutableDiscovery,
    pub confirmer: &'a mut dyn Confirmer,
    pub terminal: &'a dyn TerminalState,
    pub clock: &'a dyn Clock,
    pub launcher: &'a mut dyn ProcessLauncher,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupError {
    Exists,
    RequiresVerification,
    Rejected,
    PublicationFailed,
    UnsupportedProvider,
}

impl fmt::Display for SetupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Exists => "setup report destination already exists",
            Self::RequiresVerification => "setup reports are available only for verification modes",
            Self::Rejected => "setup report failed privacy validation",
            Self::PublicationFailed => "setup report could not be published",
            Self::UnsupportedProvider => {
                "the selected setup provider is not available in this build"
            }
        })
    }
}

impl std::error::Error for SetupError {}

pub fn run_setup(
    options: &SetupOptions,
    providers: &[ProviderDescriptor],
    runtime: &mut SetupRuntime<'_>,
) -> Result<SetupSummary, SetupError> {
    if let Some(path) = options.report_json.as_deref() {
        if options.mode == SetupMode::Login {
            return Err(SetupError::RequiresVerification);
        }
        if path.exists() {
            return Err(SetupError::Exists);
        }
    }

    let provider_ids = options.selector.providers();
    if provider_ids
        .iter()
        .any(|provider_id| !providers.iter().any(|provider| provider.id == *provider_id))
    {
        return Err(SetupError::UnsupportedProvider);
    }
    let mut results = Vec::with_capacity(provider_ids.len());
    if !runtime.terminal.is_interactive() {
        results.extend(provider_ids.iter().copied().map(|provider| ProviderResult {
            provider,
            outcome: Outcome::NonTty,
            duration: Duration::ZERO,
            version: None,
            install_guidance: None,
        }));
    } else {
        for provider_id in provider_ids {
            let provider = providers
                .iter()
                .find(|provider| provider.id == *provider_id)
                .expect("provider registration was validated before setup");
            results.push(run_provider(options, provider, runtime));
        }
    }

    let summary = SetupSummary { results };
    if let Some(path) = options.report_json.as_deref() {
        publish_report(path, &summary, &options.candidate_commit)?;
    }
    Ok(summary)
}

pub fn render_human(summary: &SetupSummary, output: &mut dyn Write) -> io::Result<()> {
    for result in &summary.results {
        writeln!(
            output,
            "{}: {}",
            result.provider.label(),
            outcome_name(result.outcome)
        )?;
        if result.outcome == Outcome::NotInstalled {
            if let Some(guidance) = result.install_guidance.as_deref() {
                writeln!(output, "{guidance}")?;
            }
        }
    }
    Ok(())
}

fn run_provider(
    options: &SetupOptions,
    provider: &ProviderDescriptor,
    runtime: &mut SetupRuntime<'_>,
) -> ProviderResult {
    let provider_started = runtime.clock.now();
    let executable = match runtime.discovery.discover(provider) {
        Ok(Some(executable)) => executable,
        Ok(None) => {
            return result(
                provider.id,
                Outcome::NotInstalled,
                provider_started,
                None,
                Some(provider.official_install_guidance.clone()),
                runtime.clock,
            );
        }
        Err(_) => {
            return result(
                provider.id,
                Outcome::ProviderFailed,
                provider_started,
                None,
                None,
                runtime.clock,
            );
        }
    };

    let outcome = match options.mode {
        SetupMode::Login => run_action(
            provider,
            executable.path.as_path(),
            provider.login.as_ref(),
            SetupAction::Login,
            options.timeout,
            runtime,
        ),
        SetupMode::VerifyOnly => run_action(
            provider,
            executable.path.as_path(),
            provider.verification.as_ref(),
            SetupAction::Verification,
            options.timeout,
            runtime,
        ),
        SetupMode::LoginAndVerify => {
            let login = run_action(
                provider,
                executable.path.as_path(),
                provider.login.as_ref(),
                SetupAction::Login,
                options.timeout,
                runtime,
            );
            if login == Outcome::Completed {
                run_action(
                    provider,
                    executable.path.as_path(),
                    provider.verification.as_ref(),
                    SetupAction::Verification,
                    options.timeout,
                    runtime,
                )
            } else {
                login
            }
        }
    };
    result(
        provider.id,
        outcome,
        provider_started,
        executable.version,
        None,
        runtime.clock,
    )
}

fn run_action(
    provider: &ProviderDescriptor,
    executable: &Path,
    command: Option<&CommandSpec>,
    action: SetupAction,
    timeout: Duration,
    runtime: &mut SetupRuntime<'_>,
) -> Outcome {
    let Some(command) = command else {
        return failure_outcome(action);
    };
    let consent = ConsentRequest {
        provider: provider.id,
        action,
        command: display_command(&provider.executable, &command.args),
        notice: match action {
            SetupAction::Login => {
                "Coven will hand the terminal directly to the provider-owned login command."
                    .to_owned()
            }
            SetupAction::Verification => {
                "Verification requires network access and may incur provider usage or cost."
                    .to_owned()
            }
        },
    };
    match runtime.confirmer.confirm(&consent) {
        Ok(ConsentDecision::Accepted) => {}
        Ok(ConsentDecision::Declined) => return Outcome::Declined,
        Ok(ConsentDecision::Cancelled) => return Outcome::Cancelled,
        Err(_) => return Outcome::Cancelled,
    }
    let request = LaunchRequest {
        executable: executable.to_path_buf(),
        args: command.args.clone(),
    };
    let mut process = match runtime.launcher.launch(&request) {
        Ok(process) => process,
        Err(_) => return failure_outcome(action),
    };
    match wait_bounded(process.as_mut(), runtime.clock, timeout) {
        Ok(BoundedProcessResult::TimedOut) => Outcome::TimedOut,
        Ok(BoundedProcessResult::Exited(ProcessExit::Exited(0))) => Outcome::Completed,
        Ok(BoundedProcessResult::Exited(ProcessExit::Exited(_))) => failure_outcome(action),
        Ok(BoundedProcessResult::Exited(ProcessExit::Signalled)) => Outcome::Cancelled,
        Err(_) => failure_outcome(action),
    }
}

fn failure_outcome(action: SetupAction) -> Outcome {
    match action {
        SetupAction::Login => Outcome::ProviderFailed,
        SetupAction::Verification => Outcome::VerificationFailed,
    }
}

fn result(
    provider: ProviderId,
    outcome: Outcome,
    started: Duration,
    version: Option<String>,
    install_guidance: Option<String>,
    clock: &dyn Clock,
) -> ProviderResult {
    ProviderResult {
        provider,
        outcome,
        duration: clock.now().saturating_sub(started),
        version,
        install_guidance,
    }
}

fn display_command(executable: &str, args: &[OsString]) -> String {
    std::iter::once(executable.to_owned())
        .chain(
            args.iter()
                .map(|argument| argument.to_string_lossy().into_owned()),
        )
        .collect::<Vec<_>>()
        .join(" ")
}

fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Completed => "completed",
        Outcome::NotInstalled => "not_installed",
        Outcome::Declined => "declined",
        Outcome::Cancelled => "cancelled",
        Outcome::ProviderFailed => "provider_failed",
        Outcome::TimedOut => "timed_out",
        Outcome::NonTty => "non_tty",
        Outcome::VerificationFailed => "verification_failed",
    }
}

fn executable_names(executable: &str, pathext: Option<&OsStr>) -> Vec<OsString> {
    #[cfg(windows)]
    {
        let path = Path::new(executable);
        if path.extension().is_some() {
            return vec![OsString::from(executable)];
        }
        let extensions = pathext
            .and_then(OsStr::to_str)
            .unwrap_or(".COM;.EXE;.BAT;.CMD");
        let mut names = vec![OsString::from(executable)];
        names.extend(
            extensions
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(|extension| OsString::from(format!("{executable}{extension}"))),
        );
        names
    }

    #[cfg(not(windows))]
    {
        let _ = pathext;
        vec![OsString::from(executable)]
    }
}

#[cfg(unix)]
fn executable_is_runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn executable_is_runnable(path: &Path) -> bool {
    path.is_file()
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SetupReport {
    schema_version: u32,
    results: Vec<ReportResult>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReportResult {
    harness: ProviderId,
    version: String,
    platform: String,
    candidate_commit: String,
    duration_ms: u64,
    exit_class: Outcome,
    completion: bool,
}

fn publish_report(
    destination: &Path,
    summary: &SetupSummary,
    candidate_commit: &str,
) -> Result<(), SetupError> {
    let report = SetupReport {
        schema_version: 1,
        results: summary
            .results
            .iter()
            .map(|result| ReportResult {
                harness: result.provider,
                version: result
                    .version
                    .clone()
                    .unwrap_or_else(|| "unknown".to_owned()),
                platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
                candidate_commit: candidate_commit.to_owned(),
                duration_ms: result.duration.as_millis().try_into().unwrap_or(u64::MAX),
                exit_class: result.outcome,
                completion: result.outcome == Outcome::Completed,
            })
            .collect(),
    };
    validate_report(&report)?;
    let bytes = serde_json::to_vec_pretty(&report).map_err(|_| SetupError::Rejected)?;
    let decoded: SetupReport = serde_json::from_slice(&bytes).map_err(|_| SetupError::Rejected)?;
    validate_report(&decoded)?;

    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .and_then(OsStr::to_str)
        .filter(|name| !name.is_empty())
        .ok_or(SetupError::Rejected)?;
    let staged = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let publish_result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&staged)
            .map_err(|_| SetupError::PublicationFailed)?;
        file.write_all(&bytes)
            .map_err(|_| SetupError::PublicationFailed)?;
        file.sync_all().map_err(|_| SetupError::PublicationFailed)?;
        drop(file);

        let staged_bytes = fs::read(&staged).map_err(|_| SetupError::PublicationFailed)?;
        let staged_report: SetupReport =
            serde_json::from_slice(&staged_bytes).map_err(|_| SetupError::Rejected)?;
        validate_report(&staged_report)?;
        atomic_publish_noclobber(&staged, destination)?;
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if publish_result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    publish_result
}

fn validate_report(report: &SetupReport) -> Result<(), SetupError> {
    if report.schema_version != 1 || report.results.is_empty() {
        return Err(SetupError::Rejected);
    }
    for result in &report.results {
        if !safe_report_value(&result.version)
            || !safe_report_value(&result.platform)
            || !valid_commit(&result.candidate_commit)
            || result.completion != (result.exit_class == Outcome::Completed)
        {
            return Err(SetupError::Rejected);
        }
    }
    Ok(())
}

fn safe_report_value(value: &str) -> bool {
    const FORBIDDEN: &[&str] = &[
        "oauth",
        "account",
        "token",
        "model",
        "cookie",
        "authorization",
        "bearer",
    ];
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
        && !FORBIDDEN
            .iter()
            .any(|forbidden| value.to_ascii_lowercase().contains(forbidden))
}

fn valid_commit(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(unix)]
fn atomic_publish_noclobber(staged: &Path, destination: &Path) -> Result<(), SetupError> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};

    renameat_with(CWD, staged, CWD, destination, RenameFlags::NOREPLACE).map_err(|error| {
        if error == rustix::io::Errno::EXIST {
            SetupError::Exists
        } else {
            SetupError::PublicationFailed
        }
    })
}

#[cfg(not(unix))]
fn atomic_publish_noclobber(staged: &Path, destination: &Path) -> Result<(), SetupError> {
    if destination.exists() {
        return Err(SetupError::Exists);
    }
    fs::rename(staged, destination).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            SetupError::Exists
        } else {
            SetupError::PublicationFailed
        }
    })
}
