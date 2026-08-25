use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::sync::atomic::AtomicI32;
#[cfg(unix)]
use std::sync::MutexGuard;

use anyhow::{Context, Result};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size as terminal_cell_size, window_size,
};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize, PtySystem};
use serde::Deserialize;

type HarnessEnvironmentOverrides = Vec<(String, Option<String>)>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessCommand {
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    stdin_prompt: Option<Vec<u8>>,
    env_overrides: HarnessEnvironmentOverrides,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyRunResult {
    pub status: &'static str,
    pub exit_code: Option<i32>,
}

/// Outcome of Coven's one-shot `codex exec --json` bridge.
///
/// `harness_session_id` is the Codex thread id, not Coven's ledger session
/// id. Callers keep the two separate so they can expose a stable Coven id yet
/// resume the actual Codex conversation on a later turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexJsonRunResult {
    pub process: PtyRunResult,
    pub harness_session_id: Option<String>,
    pub error: Option<String>,
    pub emitted_assistant: bool,
}

pub struct DetachedPtySession {
    pub input: Box<dyn Write + Send>,
    pub killer: Box<dyn ChildKiller + Send + Sync>,
}

pub struct DetachedPtyObserver {
    pub on_output: Box<dyn FnMut(Vec<u8>) + Send + 'static>,
    pub on_exit: Box<dyn FnOnce(PtyRunResult) + Send + 'static>,
}

pub type AttachedOutputObserver = Box<dyn FnMut(Vec<u8>) -> Result<()> + Send + 'static>;

#[cfg(windows)]
const DETACHED_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
// Keep the value available to cross-platform source-contract tests while
// taking the production value from the Windows SDK. Centralizing the
// combination prevents a headless spawn path from silently dropping it when
// another flag (notably `CREATE_SUSPENDED`) is also required.
#[cfg(windows)]
const WINDOWS_CREATE_NO_WINDOW: u32 = windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
#[cfg(all(test, not(windows)))]
const WINDOWS_CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PTY_WRITE_QUEUE_CAPACITY: usize = 16;
enum PtyWriteRequest {
    Write {
        bytes: Vec<u8>,
        flush: bool,
        completion: Option<SyncSender<io::Result<()>>>,
    },
    Flush {
        completion: SyncSender<io::Result<()>>,
    },
}

#[derive(Clone)]
struct SharedPtyWriter {
    sender: SyncSender<PtyWriteRequest>,
}

impl Write for SharedPtyWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_and_wait(buf, false)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let (completion, completed) = mpsc::sync_channel(1);
        self.sender
            .send(PtyWriteRequest::Flush { completion })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PTY writer stopped"))?;
        completed
            .recv()
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PTY writer stopped"))?
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.write_and_wait(buf, false)
    }
}

impl SharedPtyWriter {
    fn write_and_wait(&self, bytes: &[u8], flush: bool) -> io::Result<()> {
        let (completion, completed) = mpsc::sync_channel(1);
        self.sender
            .send(PtyWriteRequest::Write {
                bytes: bytes.to_vec(),
                flush,
                completion: Some(completion),
            })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PTY writer stopped"))?;
        completed
            .recv()
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PTY writer stopped"))?
    }

    fn queue_terminal_reply(&self, reply: &'static [u8]) {
        // The output drain must never wait for the PTY input side. All replies
        // use this single FIFO path, preserving query order without blocking.
        let _ = self.sender.try_send(PtyWriteRequest::Write {
            bytes: reply.to_vec(),
            flush: true,
            completion: None,
        });
    }
}

fn spawn_shared_pty_writer(writer: Box<dyn Write + Send>) -> SharedPtyWriter {
    let (sender, receiver) = mpsc::sync_channel(PTY_WRITE_QUEUE_CAPACITY);
    thread::spawn(move || run_pty_writer(writer, receiver));
    SharedPtyWriter { sender }
}

fn run_pty_writer(mut writer: Box<dyn Write + Send>, receiver: mpsc::Receiver<PtyWriteRequest>) {
    while let Ok(request) = receiver.recv() {
        let (result, completion) = match request {
            PtyWriteRequest::Write {
                bytes,
                flush,
                completion,
            } => {
                let result = if completion.is_none() {
                    writer.write(&bytes).and_then(|written| {
                        if written == bytes.len() {
                            Ok(())
                        } else {
                            Err(io::Error::new(
                                io::ErrorKind::WriteZero,
                                "short terminal reply write",
                            ))
                        }
                    })
                } else {
                    writer.write_all(&bytes)
                }
                .and_then(|_| if flush { writer.flush() } else { Ok(()) });
                (result, completion)
            }
            PtyWriteRequest::Flush { completion } => (writer.flush(), Some(completion)),
        };
        let failed = result.is_err();
        let terminal_reply = completion.is_none();
        if let Some(completion) = completion {
            let _ = completion.send(result);
        }
        if terminal_reply && !failed {
            // ConPTY can acknowledge a tiny pipe write before its console
            // input loop is ready for the next reply. A short writer-thread
            // yield preserves FIFO pacing without ever delaying output drain.
            thread::sleep(Duration::from_millis(1));
        }
        if failed {
            break;
        }
    }
}

#[derive(Debug, Clone)]
struct SharedPtyKiller {
    inner: Arc<Mutex<PtyKillerInner>>,
}

#[derive(Debug)]
struct PtyKillerInner {
    fallback: Box<dyn ChildKiller + Send + Sync>,
    #[cfg(windows)]
    job_handle: Option<windows_sys::Win32::Foundation::HANDLE>,
}

#[cfg(windows)]
unsafe impl Send for PtyKillerInner {}

#[cfg(windows)]
impl Drop for PtyKillerInner {
    fn drop(&mut self) {
        if let Some(handle) = self.job_handle.take() {
            // SAFETY: this struct exclusively owns the job handle.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
        }
    }
}

impl ChildKiller for SharedPtyKiller {
    fn kill(&mut self) -> io::Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("PTY killer lock poisoned"))?;
        #[cfg(windows)]
        if let Some(handle) = inner.job_handle.take() {
            // Terminating the job stops the harness and every process it
            // spawned. This is what prevents startup-timeout orphans.
            let result =
                unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(handle, 1) };
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
            if result != 0 {
                return Ok(());
            }
        }
        inner.fallback.kill()
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}

impl HarnessCommand {
    pub fn program(&self) -> &str {
        &self.program
    }

    #[cfg(test)]
    pub fn args(&self) -> &[String] {
        &self.args
    }

    #[cfg(test)]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    #[cfg(test)]
    pub(crate) fn fixture(program: impl Into<String>, args: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            program: program.into(),
            args,
            cwd,
            stdin_prompt: None,
            env_overrides: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn set_stdin_prompt_for_test(&mut self, prompt: Vec<u8>) {
        self.stdin_prompt = Some(prompt);
    }

    pub(crate) fn set_environment_override(
        &mut self,
        name: impl Into<String>,
        value: Option<impl Into<String>>,
    ) {
        self.env_overrides
            .push((name.into(), value.map(Into::into)));
    }

    #[cfg(test)]
    pub(crate) fn environment_override_for_test(&self, name: &str) -> Option<&str> {
        self.env_overrides
            .iter()
            .rev()
            .find_map(|(candidate, value)| (candidate == name).then(|| value.as_deref()).flatten())
    }

    fn to_command_builder(&self) -> CommandBuilder {
        let mut builder = CommandBuilder::new(&self.program);
        builder.args(&self.args);
        builder.cwd(self.cwd.as_os_str());
        for (name, value) in &self.env_overrides {
            match value {
                Some(value) => builder.env(name, value),
                None => builder.env_remove(name),
            }
        }
        builder
    }

    fn apply_environment(&self, command: &mut std::process::Command) {
        for (name, value) in &self.env_overrides {
            match value {
                Some(value) => {
                    command.env(name, value);
                }
                None => {
                    command.env_remove(name);
                }
            }
        }
    }
}

pub fn build_harness_command(
    harness_id: &str,
    prompt: &str,
    cwd: &Path,
    mode: crate::harness::HarnessLaunchMode,
) -> Result<HarnessCommand> {
    build_harness_command_with_conversation(
        harness_id,
        prompt,
        cwd,
        mode,
        None,
        None,
        crate::harness::HarnessLaunchOptions::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_harness_command_with_conversation(
    harness_id: &str,
    prompt: &str,
    cwd: &Path,
    mode: crate::harness::HarnessLaunchMode,
    conversation: Option<&crate::harness::ConversationHint>,
    familiar: Option<&crate::harness::FamiliarContext>,
    options: crate::harness::HarnessLaunchOptions<'_>,
) -> Result<HarnessCommand> {
    build_harness_command_with_conversation_inner(
        harness_id,
        prompt,
        cwd,
        mode,
        conversation,
        familiar,
        options,
        false,
        false,
    )
}

/// Build a daemon-owned noninteractive command whose Codex prompt is carried
/// on stdin on every platform. Ordinary interactive/attached CLI construction
/// keeps its existing argv/PTY behavior; this contract is for the piped daemon
/// runner that owns stdin and can close it after the complete prompt lands.
#[allow(clippy::too_many_arguments)]
pub fn build_piped_harness_command_with_conversation(
    harness_id: &str,
    prompt: &str,
    cwd: &Path,
    mode: crate::harness::HarnessLaunchMode,
    conversation: Option<&crate::harness::ConversationHint>,
    familiar: Option<&crate::harness::FamiliarContext>,
    options: crate::harness::HarnessLaunchOptions<'_>,
) -> Result<HarnessCommand> {
    build_harness_command_with_conversation_inner(
        harness_id,
        prompt,
        cwd,
        mode,
        conversation,
        familiar,
        options,
        false,
        true,
    )
}

/// Build the dedicated one-shot Codex JSON command used by the stream bridge.
/// Keeping JSON-mode construction here makes the actual Codex `exec` token
/// explicit before user-controlled launch values or the trailing prompt are
/// added to argv.
#[allow(clippy::too_many_arguments)]
pub fn build_codex_json_harness_command_with_conversation(
    harness_id: &str,
    prompt: &str,
    cwd: &Path,
    mode: crate::harness::HarnessLaunchMode,
    conversation: Option<&crate::harness::ConversationHint>,
    familiar: Option<&crate::harness::FamiliarContext>,
    options: crate::harness::HarnessLaunchOptions<'_>,
) -> Result<HarnessCommand> {
    build_harness_command_with_conversation_inner(
        harness_id,
        prompt,
        cwd,
        mode,
        conversation,
        familiar,
        options,
        true,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_harness_command_with_conversation_inner(
    harness_id: &str,
    prompt: &str,
    cwd: &Path,
    mode: crate::harness::HarnessLaunchMode,
    conversation: Option<&crate::harness::ConversationHint>,
    familiar: Option<&crate::harness::FamiliarContext>,
    options: crate::harness::HarnessLaunchOptions<'_>,
    codex_json: bool,
    force_codex_stdin: bool,
) -> Result<HarnessCommand> {
    let (program, mut args) = if codex_json {
        crate::harness::command_parts_for_codex_json_with_conversation(
            harness_id,
            prompt,
            mode,
            conversation,
            familiar,
            options,
        )?
    } else {
        crate::harness::command_parts_for_harness_with_conversation(
            harness_id,
            prompt,
            mode,
            conversation,
            familiar,
            options,
        )?
    };
    let familiar_prompt;
    let stdin_prompt_text = if harness_id == "codex" {
        if let Some(familiar) = familiar {
            familiar_prompt = format!("{}\n\n{prompt}", familiar.identity_preamble());
            familiar_prompt.as_str()
        } else {
            prompt
        }
    } else {
        prompt
    };
    let stdin_prompt = move_codex_prompt_to_stdin(
        harness_id,
        mode,
        stdin_prompt_text,
        &mut args,
        cfg!(windows) || force_codex_stdin,
    );
    let (program, env_overrides) = prepare_harness_program(harness_id, mode, program)?;

    Ok(HarnessCommand {
        program,
        args,
        cwd: cwd.to_path_buf(),
        stdin_prompt,
        env_overrides,
    })
}

#[cfg(not(windows))]
fn prepare_harness_program(
    _harness_id: &str,
    _mode: crate::harness::HarnessLaunchMode,
    program: String,
) -> Result<(String, HarnessEnvironmentOverrides)> {
    Ok((program, Vec::new()))
}

#[cfg(windows)]
fn prepare_harness_program(
    harness_id: &str,
    mode: crate::harness::HarnessLaunchMode,
    program: String,
) -> Result<(String, HarnessEnvironmentOverrides)> {
    if harness_id != "codex"
        || mode != crate::harness::HarnessLaunchMode::NonInteractive
        || !windows_program_is_batch_shim(&program)
    {
        return Ok((program, Vec::new()));
    }
    let launch = resolve_official_codex_npm_shim(Path::new(&program)).with_context(|| {
        format!(
            "Windows Codex resolved to npm shim `{program}`, but Coven could not validate its native @openai/codex executable; reinstall @openai/codex and retry"
        )
    })?;
    Ok((
        launch.program.to_string_lossy().into_owned(),
        launch.env_overrides,
    ))
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
struct WindowsCodexNpmLaunch {
    program: PathBuf,
    env_overrides: HarnessEnvironmentOverrides,
}

#[cfg(any(windows, test))]
fn windows_program_is_batch_shim(program: &str) -> bool {
    Path::new(program)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
}

#[cfg(windows)]
fn resolve_official_codex_npm_shim(shim_path: &Path) -> Result<WindowsCodexNpmLaunch> {
    #[cfg(target_arch = "x86_64")]
    const TARGET_PACKAGE: &str = "@openai/codex-win32-x64";
    #[cfg(target_arch = "x86_64")]
    const TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";
    #[cfg(target_arch = "x86_64")]
    const TARGET_CPU: &str = "x64";
    #[cfg(target_arch = "aarch64")]
    const TARGET_PACKAGE: &str = "@openai/codex-win32-arm64";
    #[cfg(target_arch = "aarch64")]
    const TARGET_TRIPLE: &str = "aarch64-pc-windows-msvc";
    #[cfg(target_arch = "aarch64")]
    const TARGET_CPU: &str = "arm64";
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    compile_error!("the official Codex npm package supports Windows x64 and arm64 only");

    resolve_official_codex_npm_shim_for_target(shim_path, TARGET_PACKAGE, TARGET_TRIPLE, TARGET_CPU)
}

#[cfg(any(windows, test))]
fn resolve_official_codex_npm_shim_for_target(
    shim_path: &Path,
    target_package: &str,
    target_triple: &str,
    target_cpu: &str,
) -> Result<WindowsCodexNpmLaunch> {
    let entry = windows_npm_shim_entry(shim_path)?;
    let entry = std::fs::canonicalize(&entry)
        .with_context(|| format!("failed canonicalizing `{}`", entry.display()))?;
    anyhow::ensure!(
        entry.file_name().and_then(|name| name.to_str()) == Some("codex.js")
            && entry
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                == Some("bin"),
        "npm shim does not target the official @openai/codex bin/codex.js entry"
    );
    let package_root = entry
        .parent()
        .and_then(Path::parent)
        .context("official Codex npm entry has no package root")?;
    let package_json = read_json_file(&package_root.join("package.json"))?;
    anyhow::ensure!(
        package_json.get("name").and_then(serde_json::Value::as_str) == Some("@openai/codex"),
        "npm shim target package is not @openai/codex"
    );
    let declared_entry = package_json
        .get("bin")
        .and_then(|bin| bin.get("codex"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .replace('\\', "/");
    anyhow::ensure!(
        declared_entry == "bin/codex.js",
        "@openai/codex package does not declare the expected codex entry"
    );
    anyhow::ensure!(
        package_json
            .get("optionalDependencies")
            .and_then(|dependencies| dependencies.get(target_package))
            .and_then(serde_json::Value::as_str)
            .is_some(),
        "@openai/codex does not declare native package {target_package}"
    );

    let mut target_roots = Vec::new();
    for ancestor in package_root.ancestors() {
        target_roots.push(
            ancestor
                .join("node_modules")
                .join("@openai")
                .join(target_package.trim_start_matches("@openai/")),
        );
    }
    let mut native = None;
    for target_root in target_roots {
        let target_metadata = target_root.join("package.json");
        if !target_metadata.is_file() {
            continue;
        }
        let target_json = read_json_file(&target_metadata)?;
        let supports_windows = target_json
            .get("os")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|values| values.iter().any(|value| value.as_str() == Some("win32")));
        let supports_cpu = target_json
            .get("cpu")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|values| {
                values
                    .iter()
                    .any(|value| value.as_str() == Some(target_cpu))
            });
        if target_json.get("name").and_then(serde_json::Value::as_str) != Some("@openai/codex")
            || !supports_windows
            || !supports_cpu
        {
            continue;
        }
        let candidate = target_root
            .join("vendor")
            .join(target_triple)
            .join("bin")
            .join("codex.exe");
        if candidate.is_file() {
            let canonical_root = std::fs::canonicalize(&target_root)?;
            let canonical_candidate = std::fs::canonicalize(&candidate)?;
            anyhow::ensure!(
                canonical_candidate.starts_with(&canonical_root),
                "native Codex executable escapes its validated package root"
            );
            native = Some(canonical_candidate);
            break;
        }
    }
    let native = native.with_context(|| {
        format!(
            "native optional dependency {target_package} is missing its {target_triple} executable"
        )
    })?;
    let canonical_package_root = std::fs::canonicalize(package_root)?;
    let manager = codex_package_manager(&canonical_package_root);
    let mut env_overrides = [
        "CODEX_MANAGED_BY_NPM",
        "CODEX_MANAGED_BY_BUN",
        "CODEX_MANAGED_BY_PNPM",
    ]
    .into_iter()
    .map(|name| (name.to_string(), None))
    .collect::<Vec<_>>();
    env_overrides.push((
        "CODEX_MANAGED_PACKAGE_ROOT".to_string(),
        Some(canonical_package_root.to_string_lossy().into_owned()),
    ));
    env_overrides.push((format!("CODEX_MANAGED_BY_{manager}"), Some("1".to_string())));
    Ok(WindowsCodexNpmLaunch {
        program: native,
        env_overrides,
    })
}

#[cfg(any(windows, test))]
fn windows_npm_shim_entry(shim_path: &Path) -> Result<PathBuf> {
    let bin_dir = shim_path
        .parent()
        .context("Windows npm shim has no parent directory")?;
    let shim = std::fs::read_to_string(shim_path)
        .with_context(|| format!("failed reading npm shim `{}`", shim_path.display()))?;
    for line in shim.lines() {
        let trimmed = line.trim_start();
        let lowercase = trimmed.to_ascii_lowercase();
        if lowercase.starts_with("if exist")
            || lowercase.starts_with("@if exist")
            || lowercase.starts_with("set")
            || lowercase.starts_with("@set")
            || lowercase.starts_with("call")
            || lowercase.starts_with("@call")
            || lowercase.starts_with("rem")
            || lowercase.starts_with("@rem")
            || trimmed.starts_with("::")
            || trimmed.starts_with(':')
        {
            continue;
        }
        let quoted = line.split('"').skip(1).step_by(2).collect::<Vec<_>>();
        for target in quoted.into_iter().rev() {
            let lowercase = target.to_ascii_lowercase();
            let relative = lowercase
                .strip_prefix("%dp0%\\")
                .map(|_| &target[6..])
                .or_else(|| lowercase.strip_prefix("%~dp0\\").map(|_| &target[6..]))
                .or_else(|| lowercase.strip_prefix("%dp0%/").map(|_| &target[6..]))
                .or_else(|| lowercase.strip_prefix("%~dp0/").map(|_| &target[6..]));
            let Some(relative) = relative else {
                continue;
            };
            if relative.contains('%') {
                continue;
            }
            let relative = relative.replace(['\\', '/'], std::path::MAIN_SEPARATOR_STR);
            let candidate = bin_dir.join(relative);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    anyhow::bail!(
        "npm shim `{}` does not contain a safe existing package entry",
        shim_path.display()
    )
}

#[cfg(any(windows, test))]
fn read_json_file(path: &Path) -> Result<serde_json::Value> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed reading package metadata `{}`", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid package metadata `{}`", path.display()))
}

#[cfg(any(windows, test))]
fn codex_package_manager(package_root: &Path) -> &'static str {
    let user_agent = std::env::var("npm_config_user_agent")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let exec_path = std::env::var("npm_execpath")
        .unwrap_or_default()
        .to_ascii_lowercase();
    codex_package_manager_with_env(package_root, &user_agent, &exec_path)
}

#[cfg(any(windows, test))]
fn codex_package_manager_with_env(
    package_root: &Path,
    user_agent: &str,
    exec_path: &str,
) -> &'static str {
    let rendered = package_root.to_string_lossy().to_ascii_lowercase();
    if codex_package_is_pnpm_owned(package_root)
        || rendered.contains(".pnpm")
        || user_agent.contains("pnpm/")
        || exec_path.contains("pnpm")
    {
        "PNPM"
    } else if rendered.contains(".bun") || user_agent.contains("bun/") || exec_path.contains("bun")
    {
        "BUN"
    } else {
        "NPM"
    }
}

#[cfg(any(windows, test))]
fn codex_package_is_pnpm_owned(package_root: &Path) -> bool {
    package_root.ancestors().any(|ancestor| {
        let node_modules = ancestor.join("node_modules");
        if !node_modules.join(".modules.yaml").is_file() {
            return false;
        }
        std::fs::canonicalize(node_modules.join("@openai").join("codex"))
            .is_ok_and(|candidate| candidate == package_root)
    })
}

/// Codex supports `-` as the prompt positional, reading the complete prompt
/// from stdin. Windows always needs this because npm shims pass through
/// cmd.exe; daemon-owned piped Codex launches request it on every platform so
/// compiled Research prompts never become one oversized argv value.
fn move_codex_prompt_to_stdin(
    harness_id: &str,
    mode: crate::harness::HarnessLaunchMode,
    prompt: &str,
    args: &mut [String],
    use_stdin: bool,
) -> Option<Vec<u8>> {
    if !use_stdin
        || harness_id != "codex"
        || mode != crate::harness::HarnessLaunchMode::NonInteractive
    {
        return None;
    }

    let prompt_arg = args.last_mut()?;
    *prompt_arg = "-".to_string();
    Some(prompt.as_bytes().to_vec())
}

#[cfg(windows)]
fn write_stdin_prompt(child: &mut std::process::Child, prompt: Option<&[u8]>) -> Result<()> {
    let result = write_stdin_prompt_bytes(child, prompt);
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

#[cfg(any(windows, test))]
fn write_stdin_prompt_bytes(child: &mut std::process::Child, prompt: Option<&[u8]>) -> Result<()> {
    let Some(prompt) = prompt else {
        return Ok(());
    };
    let mut stdin = child
        .stdin
        .take()
        .context("piped harness did not expose stdin for its prompt")?;
    stdin
        .write_all(prompt)
        .context("failed writing harness prompt to stdin")?;
    stdin.flush().context("failed flushing harness prompt")
}

const CODEX_JSON_ACTIVITY_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const CODEX_POST_EXIT_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
const CODEX_CHILD_POLL_INTERVAL: Duration = Duration::from_millis(50);
const CODEX_STDERR_TAIL_BYTES: usize = 8 * 1024;
const NATIVE_POST_EXIT_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

// Supervised streams run in separate Unix sessions so Coven can clean up an
// entire harness tree in one operation. That also means a TERM sent to Coven
// itself would otherwise leave the child group behind. The scoped guard below
// records the signal atomically; the owning runner observes it on its bounded
// polling interval, terminates through its stable process-tree handle, and
// reaps the direct child.
#[cfg(unix)]
static SUPERVISED_STREAM_CANCELLATION_SIGNAL: AtomicI32 = AtomicI32::new(0);
#[cfg(unix)]
static SUPERVISED_STREAM_CANCELLATION_LOCK: Mutex<()> = Mutex::new(());

#[cfg(all(test, unix))]
// The test signaler and guard restoration share this lock so a test TERM can
// never race a restored disposition. This observer is not compiled into the
// production signal path.
struct SupervisedStreamCancellationTestLifecycle {
    observer: Option<thread::ThreadId>,
    phase: SupervisedStreamCancellationTestPhase,
}

#[cfg(all(test, unix))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SupervisedStreamCancellationTestPhase {
    Registered,
    InstalledMasked,
    ActiveUnmasked,
    Restoring,
    Finished,
}

#[cfg(all(test, unix))]
static SUPERVISED_STREAM_CANCELLATION_TEST_LIFECYCLE: Mutex<
    SupervisedStreamCancellationTestLifecycle,
> = Mutex::new(SupervisedStreamCancellationTestLifecycle {
    observer: None,
    phase: SupervisedStreamCancellationTestPhase::Finished,
});

#[cfg(all(test, unix))]
struct SupervisedStreamCancellationTestObserver {
    owner: thread::ThreadId,
}

#[cfg(all(test, unix))]
impl SupervisedStreamCancellationTestObserver {
    fn arm() -> Result<Self> {
        let owner = thread::current().id();
        let mut lifecycle = SUPERVISED_STREAM_CANCELLATION_TEST_LIFECYCLE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.observer.is_some() {
            anyhow::bail!("a supervised stream cancellation test observer is already active");
        }
        lifecycle.observer = Some(owner);
        lifecycle.phase = SupervisedStreamCancellationTestPhase::Registered;
        Ok(Self { owner })
    }

    /// Only an actually unmasked guard accepts a test SIGTERM.
    ///
    /// The lifecycle mutex covers both this dispatch and the guard's physical
    /// signal-mask and handler transitions. A test signal can therefore never
    /// be queued for later delivery under a restored disposition.
    fn send_sigterm_if_guarded(
        &self,
        target: usize,
    ) -> Result<SupervisedStreamCancellationTestPhase> {
        let lifecycle = SUPERVISED_STREAM_CANCELLATION_TEST_LIFECYCLE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.observer.as_ref() != Some(&self.owner) {
            anyhow::bail!("supervised stream cancellation test observer no longer owns lifecycle");
        }
        match lifecycle.phase {
            SupervisedStreamCancellationTestPhase::Registered
            | SupervisedStreamCancellationTestPhase::InstalledMasked
            | SupervisedStreamCancellationTestPhase::Restoring
            | SupervisedStreamCancellationTestPhase::Finished => return Ok(lifecycle.phase),
            SupervisedStreamCancellationTestPhase::ActiveUnmasked => {}
        }

        let result = unsafe { libc::pthread_kill(target as libc::pthread_t, libc::SIGTERM) };
        if result != 0 {
            return Err(std::io::Error::from_raw_os_error(result))
                .context("sending fixture SIGTERM to the stream runner thread");
        }
        Ok(lifecycle.phase)
    }
}

#[cfg(all(test, unix))]
impl Drop for SupervisedStreamCancellationTestObserver {
    fn drop(&mut self) {
        let mut lifecycle = SUPERVISED_STREAM_CANCELLATION_TEST_LIFECYCLE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.observer.as_ref() == Some(&self.owner) {
            lifecycle.phase = SupervisedStreamCancellationTestPhase::Finished;
            lifecycle.observer = None;
        }
    }
}

#[cfg(unix)]
extern "C" fn cancel_supervised_stream(signal: libc::c_int) {
    SUPERVISED_STREAM_CANCELLATION_SIGNAL.store(signal, Ordering::Relaxed);
}

/// Temporarily converts TERM/INT/HUP into a supervised bridge cancellation.
///
/// Signal dispositions are process-global, so runs in one process are
/// serialized while the guard is installed. The old dispositions are restored
/// before releasing that lock, preserving normal signal behavior for other
/// Coven commands and unit tests.
#[cfg(unix)]
struct SupervisedStreamCancellationGuard {
    _lock: MutexGuard<'static, ()>,
    previous_handlers: Vec<(libc::c_int, libc::sigaction)>,
    signal_mask: SupervisedSignalMask,
    active: bool,
    #[cfg(test)]
    test_lifecycle_registered: bool,
}

#[cfg(unix)]
struct SupervisedSignalMask {
    previous: libc::sigset_t,
    supervisor_unblocked: bool,
}

#[cfg(unix)]
impl SupervisedSignalMask {
    fn block() -> io::Result<Self> {
        let signals = supervised_signal_set();
        let mut previous = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &signals, &mut previous) };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result));
        }
        Ok(Self {
            previous,
            supervisor_unblocked: false,
        })
    }

    fn unblock_supervisor(&mut self) -> io::Result<()> {
        let result = unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &self.previous, std::ptr::null_mut())
        };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result));
        }
        self.supervisor_unblocked = true;
        Ok(())
    }

    fn reblock(&mut self) -> io::Result<()> {
        if !self.supervisor_unblocked {
            return Ok(());
        }
        let signals = supervised_signal_set();
        let result =
            unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &signals, std::ptr::null_mut()) };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result));
        }
        self.supervisor_unblocked = false;
        Ok(())
    }

    fn restore(&mut self) -> io::Result<()> {
        let result = unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &self.previous, std::ptr::null_mut())
        };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result));
        }
        self.supervisor_unblocked = true;
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for SupervisedSignalMask {
    fn drop(&mut self) {
        if !self.supervisor_unblocked {
            let _ = self.restore();
        }
    }
}

#[cfg(unix)]
fn supervised_signal_set() -> libc::sigset_t {
    let mut signals = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut signals);
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut signals, signal);
        }
    }
    signals
}

#[cfg(unix)]
impl SupervisedStreamCancellationGuard {
    fn install(context: &str) -> Result<Self> {
        let lock = SUPERVISED_STREAM_CANCELLATION_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut signal_mask = SupervisedSignalMask::block()
            .context("failed to block supervised stream cancellation signals")?;
        SUPERVISED_STREAM_CANCELLATION_SIGNAL.store(0, Ordering::Relaxed);

        let mut previous_handlers = Vec::with_capacity(3);
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            // SAFETY: sigaction is the POSIX interface for installing a signal
            // handler. The handler only records into an atomic, and each
            // successful installation retains the prior disposition for Drop.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = cancel_supervised_stream as *const () as usize;
                libc::sigemptyset(&mut action.sa_mask);
                action.sa_flags = 0;
                let mut previous: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(signal, &action, &mut previous) != 0 {
                    let error = std::io::Error::last_os_error();
                    for (installed_signal, installed_previous) in previous_handlers.iter().rev() {
                        let _ = libc::sigaction(
                            *installed_signal,
                            installed_previous,
                            std::ptr::null_mut(),
                        );
                    }
                    SUPERVISED_STREAM_CANCELLATION_SIGNAL.store(0, Ordering::Relaxed);
                    let _ = signal_mask.restore();
                    return Err(error).with_context(|| {
                        format!(
                            "failed to install {context} cancellation handler for signal {signal}"
                        )
                    });
                }
                previous_handlers.push((signal, previous));
            }
        }

        #[cfg(test)]
        let test_lifecycle_registered = {
            let mut lifecycle = SUPERVISED_STREAM_CANCELLATION_TEST_LIFECYCLE
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if lifecycle.observer.as_ref() == Some(&thread::current().id()) {
                debug_assert_eq!(
                    lifecycle.phase,
                    SupervisedStreamCancellationTestPhase::Registered
                );
                lifecycle.phase = SupervisedStreamCancellationTestPhase::InstalledMasked;
                true
            } else {
                false
            }
        };

        Ok(Self {
            _lock: lock,
            previous_handlers,
            signal_mask,
            active: true,
            #[cfg(test)]
            test_lifecycle_registered,
        })
    }

    fn cancelled_signal(&self) -> Option<libc::c_int> {
        let signal = SUPERVISED_STREAM_CANCELLATION_SIGNAL.load(Ordering::Relaxed);
        (signal != 0).then_some(signal)
    }

    fn activate(&mut self) -> Result<Option<libc::c_int>> {
        #[cfg(test)]
        let mut test_lifecycle = self.test_lifecycle_registered.then(|| {
            SUPERVISED_STREAM_CANCELLATION_TEST_LIFECYCLE
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        });
        #[cfg(test)]
        if let Some(lifecycle) = &test_lifecycle {
            if lifecycle.observer.as_ref() == Some(&thread::current().id())
                && lifecycle.phase != SupervisedStreamCancellationTestPhase::InstalledMasked
            {
                anyhow::bail!(
                    "supervised stream cancellation test lifecycle was not installed and masked before activation"
                );
            }
        }
        self.signal_mask
            .unblock_supervisor()
            .context("failed to unblock supervised stream cancellation signals")?;
        #[cfg(test)]
        if let Some(lifecycle) = &mut test_lifecycle {
            if lifecycle.observer.as_ref() == Some(&thread::current().id()) {
                lifecycle.phase = SupervisedStreamCancellationTestPhase::ActiveUnmasked;
            }
        }
        Ok(self.cancelled_signal())
    }

    fn finish(mut self) -> Result<Option<libc::c_int>> {
        #[cfg(test)]
        let mut test_lifecycle = self.test_lifecycle_registered.then(|| {
            SUPERVISED_STREAM_CANCELLATION_TEST_LIFECYCLE
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        });
        #[cfg(test)]
        if let Some(lifecycle) = &mut test_lifecycle {
            if lifecycle.observer.as_ref() == Some(&thread::current().id()) {
                lifecycle.phase = SupervisedStreamCancellationTestPhase::Restoring;
            }
        }
        self.signal_mask
            .reblock()
            .context("failed to re-block supervised stream cancellation signals")?;
        let cancelled_signal = self.cancelled_signal();
        let mut restore_error = None;
        unsafe {
            for (signal, previous) in self.previous_handlers.iter().rev() {
                if libc::sigaction(*signal, previous, std::ptr::null_mut()) != 0
                    && restore_error.is_none()
                {
                    restore_error = Some(std::io::Error::last_os_error());
                }
            }
        }
        SUPERVISED_STREAM_CANCELLATION_SIGNAL.store(0, Ordering::Relaxed);
        self.active = false;
        self.signal_mask
            .restore()
            .context("failed to restore supervised stream signal mask")?;
        #[cfg(test)]
        if let Some(lifecycle) = &mut test_lifecycle {
            if lifecycle.observer.as_ref() == Some(&thread::current().id()) {
                lifecycle.phase = SupervisedStreamCancellationTestPhase::Finished;
            }
        }
        if let Some(error) = restore_error {
            return Err(error).context("failed to restore supervised stream cancellation handlers");
        }
        Ok(cancelled_signal)
    }
}

#[cfg(unix)]
impl Drop for SupervisedStreamCancellationGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        #[cfg(test)]
        let mut test_lifecycle = self.test_lifecycle_registered.then(|| {
            SUPERVISED_STREAM_CANCELLATION_TEST_LIFECYCLE
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        });
        #[cfg(test)]
        if let Some(lifecycle) = &mut test_lifecycle {
            if lifecycle.observer.as_ref() == Some(&thread::current().id()) {
                lifecycle.phase = SupervisedStreamCancellationTestPhase::Restoring;
            }
        }
        let _ = self.signal_mask.reblock();
        // SAFETY: every entry was captured from a successful sigaction call
        // in install. Restoring it here makes the scope transparent once the
        // bridge has reaped its child tree.
        unsafe {
            for (signal, previous) in self.previous_handlers.iter().rev() {
                let _ = libc::sigaction(*signal, previous, std::ptr::null_mut());
            }
        }
        SUPERVISED_STREAM_CANCELLATION_SIGNAL.store(0, Ordering::Relaxed);
        let _ = self.signal_mask.restore();
        #[cfg(test)]
        if let Some(lifecycle) = &mut test_lifecycle {
            if lifecycle.observer.as_ref() == Some(&thread::current().id()) {
                lifecycle.phase = SupervisedStreamCancellationTestPhase::Finished;
            }
        }
        #[cfg(test)]
        drop(test_lifecycle);
    }
}

#[cfg(not(unix))]
struct SupervisedStreamCancellationGuard;

#[cfg(not(unix))]
impl SupervisedStreamCancellationGuard {
    fn install(_context: &str) -> Result<Self> {
        Ok(Self)
    }

    fn cancelled_signal(&self) -> Option<i32> {
        None
    }

    fn activate(&mut self) -> Result<Option<i32>> {
        Ok(None)
    }

    fn finish(self) -> Result<Option<i32>> {
        Ok(None)
    }
}

fn wait_for_supervised_child(
    child: &mut std::process::Child,
    context: &str,
) -> Result<std::process::ExitStatus> {
    child.wait().with_context(|| context.to_string())
}

fn terminate_and_wait_for_supervised_child(
    process_tree: &mut StrictChildProcessTree,
    child: &mut std::process::Child,
    context: &str,
) -> Result<std::process::ExitStatus> {
    process_tree.terminate(child);
    wait_for_supervised_child(child, context)
}

#[cfg(unix)]
fn supervised_stream_cancellation_error(
    guard: &SupervisedStreamCancellationGuard,
    context: &str,
) -> Option<String> {
    guard
        .cancelled_signal()
        .map(|signal| supervised_stream_cancellation_error_for_signal(signal, context))
}

#[cfg(unix)]
fn supervised_stream_cancellation_error_for_signal(signal: libc::c_int, context: &str) -> String {
    let name = match signal {
        libc::SIGTERM => "SIGTERM",
        libc::SIGINT => "SIGINT",
        libc::SIGHUP => "SIGHUP",
        _ => "a termination signal",
    };
    format!("{context} cancelled by {name}; the process tree was terminated")
}

#[cfg(not(unix))]
fn supervised_stream_cancellation_error(
    _guard: &SupervisedStreamCancellationGuard,
    _context: &str,
) -> Option<String> {
    None
}

#[cfg(not(unix))]
fn supervised_stream_cancellation_error_for_signal(_signal: i32, _context: &str) -> String {
    unreachable!("non-Unix cancellation guards never report a signal")
}

fn codex_json_activity_timeout() -> Duration {
    // Integration tests execute the real `coven` binary, not the unit-test
    // crate, so `cfg(test)` cannot inject a short deadline into that child.
    // Keep this hook out of release builds while still making the terminal
    // timeout/result/ledger path testable without waiting five minutes.
    #[cfg(debug_assertions)]
    if let Some(timeout_ms) = std::env::var("COVEN_TEST_CODEX_JSON_IDLE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|timeout_ms| *timeout_ms > 0)
    {
        return Duration::from_millis(timeout_ms);
    }
    CODEX_JSON_ACTIVITY_TIMEOUT
}

enum CodexStdoutMessage {
    Line(String),
    ReadError(String),
}

enum CodexRunnerMessage {
    Stdout(CodexStdoutMessage),
    StdoutClosed,
    StderrClosed(Vec<u8>),
    StdinComplete(std::result::Result<(), String>),
}

#[derive(Default)]
struct CodexJsonState {
    harness_session_id: Option<String>,
    protocol_error: Option<String>,
    emitted_assistant: bool,
}

/// Own a one-shot child process tree. A wrapper can outlive or outspawn the
/// direct launcher, so a plain `Child::kill()` is not enough to guarantee pipe
/// EOF or descendant cleanup.
pub(crate) struct ChildProcessTree {
    #[cfg(unix)]
    pid: u32,
    terminated: bool,
    #[cfg(windows)]
    job_handle: Option<windows_sys::Win32::Foundation::HANDLE>,
}

// The Job Object handle is exclusively owned by `ChildProcessTree` and the
// Win32 job APIs used by its methods are thread-safe. Moving that ownership to
// the daemon's cancellation registry is therefore safe.
#[cfg(windows)]
unsafe impl Send for ChildProcessTree {}

impl ChildProcessTree {
    #[cfg(unix)]
    pub(crate) fn attach(child: &std::process::Child) -> Self {
        let pid = child.id();
        Self {
            pid,
            terminated: false,
        }
    }

    fn terminate_tree(&mut self) -> io::Result<()> {
        if self.terminated {
            return Ok(());
        }
        #[cfg(unix)]
        {
            terminate_unix_process_group(self.pid)?;
            // Leave the handle retryable when signaling fails while live
            // processes may remain. `terminate_unix_process_group` treats
            // platform-specific already-exited results as success.
            self.terminated = true;
        }
        #[cfg(windows)]
        {
            if let Some(job) = self.job_handle.take() {
                let terminated =
                    unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(job, 1) };
                let error = (terminated == 0).then(io::Error::last_os_error);
                unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
                // Closing the KILL_ON_JOB_CLOSE handle is the kernel-enforced
                // backstop even when the explicit termination call reports an
                // error, so no retryable ownership remains after this point.
                self.terminated = true;
                if let Some(error) = error {
                    return Err(error);
                }
            } else {
                self.terminated = true;
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            self.terminated = true;
        }
        Ok(())
    }

    fn terminate_impl(&mut self, child: &mut std::process::Child) {
        let _ = self.terminate_tree();
        let _ = child.kill();
    }
}

/// A process tree whose descendants were contained before its first
/// instruction ran.
pub(crate) struct StrictChildProcessTree(ChildProcessTree);

impl StrictChildProcessTree {
    fn attach(child: &std::process::Child) -> Option<Self> {
        #[cfg(unix)]
        {
            Some(Self(ChildProcessTree::attach(child)))
        }
        #[cfg(windows)]
        {
            let job_handle = child_job_object_for_process(child)?;
            if !resume_suspended_child(child) {
                // KILL_ON_JOB_CLOSE ensures a partially resumed child cannot
                // escape when thread enumeration/resume fails.
                unsafe { windows_sys::Win32::Foundation::CloseHandle(job_handle) };
                return None;
            }
            Some(Self(ChildProcessTree {
                terminated: false,
                job_handle: Some(job_handle),
            }))
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = child;
            None
        }
    }

    pub(crate) fn terminate(&mut self, child: &mut std::process::Child) {
        self.0.terminate_impl(child);
    }

    /// Terminate the contained tree without borrowing the root `Child`.
    ///
    /// Detached piped sessions move the `Child` into their wait thread, while
    /// the daemon retains this containment handle for cancellation. Windows
    /// terminates the pre-attached Job Object; Unix signals the pre-created
    /// process group. Both operations are idempotent when the tree exited.
    pub(crate) fn terminate_tree(&mut self) -> io::Result<()> {
        self.0.terminate_tree()
    }

    #[cfg(all(windows, test))]
    fn close_job_handle_without_explicit_termination_for_test(&mut self) {
        if let Some(job) = self.0.job_handle.take() {
            // Model the handle closure the kernel performs when coven.exe is
            // terminated too abruptly to run Drop. The configured
            // KILL_ON_JOB_CLOSE limit must still tear down every descendant.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
        }
        self.0.terminated = true;
    }
}

/// Cloneable ownership of one strict process-tree containment handle.
///
/// A piped prompt writer and the daemon registry must be able to observe and
/// terminate the same tree without duplicating pid-based containment. The
/// underlying Job Object/process-group handle remains singular and every
/// operation is serialized through this shared owner.
#[derive(Clone)]
pub(crate) struct SharedStrictChildProcessTree {
    process_tree: Arc<Mutex<StrictChildProcessTree>>,
    #[cfg(unix)]
    guardian: Arc<Mutex<ProcessSupervisorGuardian>>,
    child_wait_state: Arc<AtomicU8>,
    exit_callback_complete: Arc<AtomicBool>,
}

impl SharedStrictChildProcessTree {
    #[cfg(unix)]
    fn new(
        process_tree: StrictChildProcessTree,
        guardian: ProcessSupervisorGuardian,
        child_wait_state: Arc<AtomicU8>,
        exit_callback_complete: Arc<AtomicBool>,
    ) -> Self {
        Self {
            process_tree: Arc::new(Mutex::new(process_tree)),
            guardian: Arc::new(Mutex::new(guardian)),
            child_wait_state,
            exit_callback_complete,
        }
    }

    #[cfg(not(unix))]
    fn new(
        process_tree: StrictChildProcessTree,
        child_wait_state: Arc<AtomicU8>,
        exit_callback_complete: Arc<AtomicBool>,
    ) -> Self {
        Self {
            process_tree: Arc::new(Mutex::new(process_tree)),
            child_wait_state,
            exit_callback_complete,
        }
    }

    pub(crate) fn terminate_tree(&self) -> io::Result<()> {
        let termination = match self.process_tree.lock() {
            Ok(mut process_tree) => process_tree.terminate_tree(),
            Err(poisoned) => poisoned.into_inner().terminate_tree(),
        };
        #[cfg(unix)]
        match self.guardian.lock() {
            Ok(mut guardian) => guardian.finish(),
            Err(poisoned) => poisoned.into_inner().finish(),
        }
        termination
    }

    pub(crate) fn terminate_and_wait(&self, timeout: Duration) -> Result<()> {
        let termination = self.terminate_tree().err();
        let wait_state = wait_for_piped_child_reap(&self.child_wait_state, timeout);
        match (termination, wait_state) {
            (None, PIPED_CHILD_REAPED) => Ok(()),
            (Some(error), PIPED_CHILD_REAPED) => Err(anyhow::Error::new(error).context(
                "process-tree termination reported an error after the child became quiescent",
            )),
            (termination, PIPED_CHILD_WAIT_FAILED) => match termination {
                Some(error) => anyhow::bail!(
                    "process-tree termination failed ({error}) and the direct child wait failed"
                ),
                None => anyhow::bail!(
                    "the direct child wait failed after process-tree termination"
                ),
            },
            (termination, _) => match termination {
                Some(error) => anyhow::bail!(
                    "process-tree termination failed ({error}) and the child did not become quiescent within {} ms",
                    timeout.as_millis()
                ),
                None => anyhow::bail!(
                    "the child did not become quiescent within {} ms after process-tree termination",
                    timeout.as_millis()
                ),
            },
        }
    }

    pub(crate) fn wait_for_quiescence(&self, timeout: Duration) -> Result<()> {
        match wait_for_piped_child_reap(&self.child_wait_state, timeout) {
            PIPED_CHILD_REAPED => Ok(()),
            PIPED_CHILD_WAIT_FAILED => {
                anyhow::bail!("the direct child wait or output drain failed")
            }
            _ => anyhow::bail!(
                "the child did not become quiescent within {} ms",
                timeout.as_millis()
            ),
        }
    }

    pub(crate) fn wait_for_shutdown_quiescence(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        self.wait_for_quiescence(timeout)?;
        while !self.exit_callback_complete.load(Ordering::Acquire) {
            anyhow::ensure!(
                Instant::now() < deadline,
                "the piped child exit callback did not complete within {} ms",
                timeout.as_millis()
            );
            thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }

    fn is_terminated(&self) -> bool {
        match self.process_tree.lock() {
            Ok(process_tree) => process_tree.0.terminated,
            Err(poisoned) => poisoned.into_inner().0.terminated,
        }
    }

    #[cfg(all(windows, test))]
    fn close_job_handle_without_explicit_termination_for_test(&self) {
        match self.process_tree.lock() {
            Ok(mut process_tree) => {
                process_tree.close_job_handle_without_explicit_termination_for_test()
            }
            Err(poisoned) => poisoned
                .into_inner()
                .close_job_handle_without_explicit_termination_for_test(),
        }
    }
}

#[cfg(unix)]
fn terminate_unix_process_group(pid: u32) -> io::Result<()> {
    // The launch config puts the child at the head of a new session, so the
    // negative pid reaches its wrapper and every descendant.
    let result = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    let already_exited = error.raw_os_error() == Some(libc::ESRCH)
        || (cfg!(target_os = "macos") && error.raw_os_error() == Some(libc::EPERM));
    if already_exited {
        // Darwin reports EPERM when the group contains only its unreaped
        // zombie leader. Live same-owner descendants make kill succeed.
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(unix)]
fn poll_child_exit_without_reaping(child: &std::process::Child) -> io::Result<bool> {
    loop {
        // POSIX leaves siginfo contents unspecified when WNOHANG finds no
        // waitable child, so initialize it for every call and trust si_pid
        // only when waitid explicitly fills it.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id() as _,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == 0 {
            return Ok(unsafe { info.si_pid() } != 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn wait_for_child_exit_without_reaping(child: &std::process::Child) -> io::Result<()> {
    #[cfg(unix)]
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id() as _,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::WaitForSingleObject;

        let result = unsafe { WaitForSingleObject(child.as_raw_handle() as _, u32::MAX) };
        match result {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_FAILED => Err(io::Error::last_os_error()),
            other => Err(io::Error::other(format!(
                "unexpected process wait result {other}"
            ))),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = child;
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for ChildProcessTree {
    fn drop(&mut self) {
        if !self.terminated {
            // A wrapper can exit after detaching a descendant that has already
            // closed stdout/stderr. There is then no pipe timeout to trigger
            // terminate(), but this one-shot runner still owns that group.
            let _ = terminate_unix_process_group(self.pid);
        }
    }
}

#[cfg(windows)]
impl Drop for ChildProcessTree {
    fn drop(&mut self) {
        if let Some(job) = self.job_handle.take() {
            // Use the same non-zero cancellation code as explicit kill when
            // Rust drops a live handle. KILL_ON_JOB_CLOSE remains the kernel
            // backstop when coven.exe itself terminates too abruptly to run
            // this destructor.
            unsafe {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(job, 1);
                windows_sys::Win32::Foundation::CloseHandle(job);
            }
        }
    }
}

#[cfg(windows)]
fn child_job_object_for_process(
    child: &std::process::Child,
) -> Option<windows_sys::Win32::Foundation::HANDLE> {
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
    };

    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job == INVALID_HANDLE_VALUE || job == 0 as _ {
            return None;
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let limits_set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const std::ffi::c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) != 0;
        // Child already owns the CreateProcess handle with the permissions
        // required for assignment, avoiding a pid reuse race through
        // OpenProcess.
        let assigned = AssignProcessToJobObject(job, child.as_raw_handle() as _) != 0;
        if !limits_set || !assigned {
            CloseHandle(job);
            return None;
        }
        Some(job)
    }
}

#[cfg(windows)]
fn resume_suspended_child(child: &std::process::Child) -> bool {
    use std::mem::size_of;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                THREADENTRY32,
            },
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return false;
    }

    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut thread_ids = Vec::new();
    let mut has_entry = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    let mut enumeration_complete = false;
    while has_entry {
        if entry.th32OwnerProcessID == child.id() {
            thread_ids.push(entry.th32ThreadID);
        }
        has_entry = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
        if !has_entry {
            enumeration_complete = unsafe { GetLastError() } == ERROR_NO_MORE_FILES;
        }
    }
    unsafe { CloseHandle(snapshot) };

    if !enumeration_complete {
        return false;
    }
    // CREATE_SUSPENDED creates exactly one primary thread. Anything else is
    // inconsistent with the promised before-first-instruction boundary.
    let [thread_id] = thread_ids.as_slice() else {
        return false;
    };
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, *thread_id) };
    if thread.is_null() {
        return false;
    }
    let previous_suspend_count = unsafe { ResumeThread(thread) };
    unsafe { CloseHandle(thread) };
    previous_suspend_count == 1
}

#[cfg(unix)]
fn configure_child_process_tree_command(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            // Isolate this turn in a fresh process group. A timeout can then
            // kill the npm/Node/native Codex tree in one signal.
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(any(windows, test))]
fn windows_noninteractive_creation_flags(additional_flags: u32) -> u32 {
    additional_flags | WINDOWS_CREATE_NO_WINDOW
}

#[cfg(windows)]
fn configure_windows_noninteractive_command(
    command: &mut std::process::Command,
    additional_flags: u32,
) {
    use std::os::windows::process::CommandExt;

    command.creation_flags(windows_noninteractive_creation_flags(additional_flags));
}

/// Spawn a process tree that cannot execute before Coven owns its descendants.
/// Unix uses a fresh session created in the child before exec. Windows creates
/// the process suspended, assigns it to a kill-on-close Job Object, and only
/// then resumes its initial thread.
pub(crate) fn spawn_strict_child_process_tree(
    command: &mut std::process::Command,
) -> std::io::Result<(std::process::Child, StrictChildProcessTree)> {
    #[cfg(unix)]
    configure_child_process_tree_command(command);
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

        configure_windows_noninteractive_command(command, CREATE_SUSPENDED);
    }

    spawn_configured_strict_child_process_tree(command)
}

fn spawn_configured_strict_child_process_tree(
    command: &mut std::process::Command,
) -> std::io::Result<(std::process::Child, StrictChildProcessTree)> {
    let mut child = command.spawn()?;
    #[cfg(all(windows, debug_assertions))]
    if let Err(error) = wait_at_windows_strict_preattach_test_barrier(child.id()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    match StrictChildProcessTree::attach(&child) {
        Some(tree) => Ok((child, tree)),
        None => {
            let _ = child.kill();
            let _ = child.wait();
            Err(std::io::Error::other(
                "failed to establish strict child-process containment",
            ))
        }
    }
}

#[cfg(all(windows, debug_assertions))]
fn wait_at_windows_strict_preattach_test_barrier(child_pid: u32) -> io::Result<()> {
    let Some(barrier_dir) = std::env::var_os("COVEN_TEST_WINDOWS_STRICT_PREATTACH_BARRIER_DIR")
    else {
        return Ok(());
    };
    let barrier_dir = PathBuf::from(barrier_dir);
    std::fs::create_dir_all(&barrier_dir)?;
    std::fs::write(barrier_dir.join("pid"), child_pid.to_string())?;
    std::fs::write(barrier_dir.join("ready"), b"ready\n")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !barrier_dir.join("release").exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out at Windows strict pre-attachment test barrier",
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

pub(crate) const PROCESS_SUPERVISOR_PROTOCOL: &str = "coven.process-supervisor.v1";
const PROCESS_SUPERVISOR_CONTROL_PREFIX: &str = "COVEN_PROCESS_SUPERVISOR_V1 ";
const PROCESS_SUPERVISOR_MAX_REQUEST_BYTES: usize = 256 * 1024;
const PROCESS_SUPERVISOR_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessSupervisorRequest {
    version: u8,
    program: String,
    args: Vec<String>,
    cwd: String,
}

fn read_process_supervisor_request(reader: &mut dyn BufRead) -> Result<ProcessSupervisorRequest> {
    let mut line = Vec::new();
    reader
        .take((PROCESS_SUPERVISOR_MAX_REQUEST_BYTES + 1) as u64)
        .read_until(b'\n', &mut line)
        .context("failed reading the process-supervisor launch frame")?;
    anyhow::ensure!(
        !line.is_empty(),
        "process-supervisor launch frame is missing"
    );
    anyhow::ensure!(
        line.len() <= PROCESS_SUPERVISOR_MAX_REQUEST_BYTES,
        "process-supervisor launch frame exceeds {PROCESS_SUPERVISOR_MAX_REQUEST_BYTES} bytes"
    );
    anyhow::ensure!(
        line.last() == Some(&b'\n'),
        "process-supervisor launch frame must end with LF"
    );
    line.pop();
    let request: ProcessSupervisorRequest =
        serde_json::from_slice(&line).context("invalid process-supervisor launch JSON")?;
    anyhow::ensure!(
        request.version == 1,
        "unsupported process-supervisor version"
    );
    anyhow::ensure!(
        !request.program.contains('\0')
            && !request.cwd.contains('\0')
            && request.args.iter().all(|arg| !arg.contains('\0')),
        "process-supervisor launch values must not contain NUL"
    );
    let program = Path::new(&request.program);
    let cwd = Path::new(&request.cwd);
    anyhow::ensure!(
        program.is_absolute(),
        "process-supervisor program must be absolute"
    );
    anyhow::ensure!(cwd.is_absolute(), "process-supervisor cwd must be absolute");
    anyhow::ensure!(
        cwd.is_dir(),
        "process-supervisor cwd must be an existing directory"
    );
    Ok(request)
}

fn write_process_supervisor_control(event: serde_json::Value) -> Result<()> {
    let mut stderr = io::stderr().lock();
    write!(stderr, "{PROCESS_SUPERVISOR_CONTROL_PREFIX}")?;
    serde_json::to_writer(&mut stderr, &event)?;
    writeln!(stderr)?;
    stderr.flush()?;
    Ok(())
}

#[cfg(unix)]
struct ProcessSupervisorGuardian {
    owner_write: libc::c_int,
    setup_pid_write: libc::c_int,
    setup_ack_read: libc::c_int,
    pid: libc::pid_t,
    finished: bool,
}

#[cfg(unix)]
fn cloexec_pipe() -> io::Result<[libc::c_int; 2]> {
    let mut fds = [-1, -1];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    for fd in fds {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == -1
        {
            let error = io::Error::last_os_error();
            unsafe {
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
            return Err(error);
        }
    }
    Ok(fds)
}

#[cfg(unix)]
unsafe fn guardian_read_exact(fd: libc::c_int, bytes: &mut [u8]) -> bool {
    let mut offset = 0;
    while offset != bytes.len() {
        let count = unsafe {
            libc::read(
                fd,
                bytes[offset..].as_mut_ptr().cast(),
                bytes.len() - offset,
            )
        };
        if count > 0 {
            offset += count as usize;
        } else if count == -1 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        } else {
            return false;
        }
    }
    true
}

#[cfg(unix)]
unsafe fn guardian_write_all(fd: libc::c_int, bytes: &[u8]) -> bool {
    let mut offset = 0;
    while offset != bytes.len() {
        let count =
            unsafe { libc::write(fd, bytes[offset..].as_ptr().cast(), bytes.len() - offset) };
        if count > 0 {
            offset += count as usize;
        } else if count == -1 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        } else {
            return false;
        }
    }
    true
}

#[cfg(unix)]
unsafe fn guardian_remap_and_close_unrelated_fds(
    owner_read: libc::c_int,
    pid_read: libc::c_int,
    ack_write: libc::c_int,
    max_fd: libc::c_int,
) -> Option<(libc::c_int, libc::c_int, libc::c_int)> {
    // A guardian is forked from a multithreaded daemon and intentionally does
    // not exec. Without this close sweep it would retain accepted API sockets,
    // store locks, and listeners after the request handler dropped them. First
    // duplicate the three protocol fds out of the fixed range so dup2 cannot
    // overwrite a still-needed source, then keep only 3/4/5.
    let owner_temp = unsafe { libc::fcntl(owner_read, libc::F_DUPFD_CLOEXEC, 64) };
    let pid_temp = unsafe { libc::fcntl(pid_read, libc::F_DUPFD_CLOEXEC, 64) };
    let ack_temp = unsafe { libc::fcntl(ack_write, libc::F_DUPFD_CLOEXEC, 64) };
    if owner_temp < 0 || pid_temp < 0 || ack_temp < 0 {
        return None;
    }
    if unsafe { libc::dup2(owner_temp, 3) } < 0
        || unsafe { libc::dup2(pid_temp, 4) } < 0
        || unsafe { libc::dup2(ack_temp, 5) } < 0
    {
        return None;
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let closed = unsafe { libc::syscall(libc::SYS_close_range, 6_u32, u32::MAX, 0_u32) } == 0;
        if !closed {
            for fd in 6..max_fd {
                unsafe { libc::close(fd) };
            }
        }
    }
    #[cfg(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    unsafe {
        libc::closefrom(6);
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )))]
    for fd in 6..max_fd {
        unsafe { libc::close(fd) };
    }
    for fd in 0..3 {
        unsafe { libc::close(fd) };
    }
    Some((3, 4, 5))
}

#[cfg(unix)]
impl ProcessSupervisorGuardian {
    fn install(command: &mut std::process::Command) -> Result<Self> {
        use std::os::unix::process::CommandExt;

        let owner = cloexec_pipe().context("failed creating supervisor owner pipe")?;
        let pid_channel = match cloexec_pipe() {
            Ok(pipe) => pipe,
            Err(error) => {
                unsafe {
                    libc::close(owner[0]);
                    libc::close(owner[1]);
                }
                return Err(error).context("failed creating supervisor pid channel");
            }
        };
        let ack_channel = match cloexec_pipe() {
            Ok(pipe) => pipe,
            Err(error) => {
                unsafe {
                    for fd in [owner[0], owner[1], pid_channel[0], pid_channel[1]] {
                        libc::close(fd);
                    }
                }
                return Err(error).context("failed creating supervisor acknowledgement channel");
            }
        };

        let max_fd = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) }
            .clamp(64, libc::c_int::MAX as libc::c_long) as libc::c_int;
        let guardian_pid = unsafe { libc::fork() };
        if guardian_pid == -1 {
            let error = io::Error::last_os_error();
            unsafe {
                for fd in [
                    owner[0],
                    owner[1],
                    pid_channel[0],
                    pid_channel[1],
                    ack_channel[0],
                    ack_channel[1],
                ] {
                    libc::close(fd);
                }
            }
            return Err(error).context("failed starting process-supervisor guardian");
        }
        if guardian_pid == 0 {
            unsafe {
                // The owning desktop may use one final group-directed SIGKILL
                // as its crash/shutdown backstop. If the guardian remained in
                // the supervisor's process group, that signal could kill both
                // owners at once while the separately-sessioned target tree
                // survived. A private session keeps the guardian alive long
                // enough to observe owner-pipe EOF and kill the target PGID.
                if libc::setsid() == -1 {
                    libc::_exit(71);
                }
                libc::close(owner[1]);
                libc::close(pid_channel[1]);
                libc::close(ack_channel[0]);
                let Some((owner_read, pid_read, ack_write)) =
                    guardian_remap_and_close_unrelated_fds(
                        owner[0],
                        pid_channel[0],
                        ack_channel[1],
                        max_fd,
                    )
                else {
                    libc::_exit(71);
                };
                let mut pid_bytes = [0_u8; std::mem::size_of::<libc::pid_t>()];
                if guardian_read_exact(pid_read, &mut pid_bytes) {
                    let target_pid = libc::pid_t::from_ne_bytes(pid_bytes);
                    let _ = guardian_write_all(ack_write, &[1]);
                    libc::close(pid_read);
                    libc::close(ack_write);
                    let mut byte = [0_u8; 1];
                    loop {
                        let count = libc::read(owner_read, byte.as_mut_ptr().cast(), 1);
                        if count == 1 && byte == *b"D" {
                            // The parent observed a spawn/exec failure. Rust's
                            // Command implementation may already have reaped
                            // that failed child, so signaling the numeric PGID
                            // here would introduce a stale-id race.
                            break;
                        }
                        if count == 1 && byte == *b"K" {
                            let _ = libc::kill(-target_pid, libc::SIGKILL);
                            break;
                        }
                        if count == 0 {
                            // Owner EOF means the supervisor/daemon died
                            // before it could perform orderly cleanup.
                            let _ = libc::kill(-target_pid, libc::SIGKILL);
                            break;
                        }
                        if count == -1
                            && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted
                        {
                            let _ = libc::kill(-target_pid, libc::SIGKILL);
                            break;
                        }
                        if count == 1 {
                            // Fail closed on an unknown owner command.
                            let _ = libc::kill(-target_pid, libc::SIGKILL);
                            break;
                        }
                    }
                }
                libc::_exit(0);
            }
        }

        unsafe {
            libc::close(owner[0]);
            libc::close(pid_channel[0]);
            libc::close(ack_channel[1]);
        }
        let owner_write = owner[1];
        let setup_pid_write = pid_channel[1];
        let setup_ack_read = ack_channel[0];
        unsafe {
            command.pre_exec(move || {
                libc::close(owner_write);
                let pid = libc::getpid().to_ne_bytes();
                if !guardian_write_all(setup_pid_write, &pid) {
                    return Err(io::Error::last_os_error());
                }
                libc::close(setup_pid_write);
                let mut ack = [0_u8; 1];
                if !guardian_read_exact(setup_ack_read, &mut ack) || ack != [1] {
                    return Err(io::Error::other(
                        "process-supervisor guardian did not acknowledge containment",
                    ));
                }
                libc::close(setup_ack_read);
                Ok(())
            });
        }
        Ok(Self {
            owner_write,
            setup_pid_write,
            setup_ack_read,
            pid: guardian_pid,
            finished: false,
        })
    }

    fn spawn_finished(&mut self) {
        unsafe {
            if self.setup_pid_write >= 0 {
                libc::close(self.setup_pid_write);
                self.setup_pid_write = -1;
            }
            if self.setup_ack_read >= 0 {
                libc::close(self.setup_ack_read);
                self.setup_ack_read = -1;
            }
        }
    }

    fn finish_with_command(&mut self, command: u8) {
        if self.finished {
            return;
        }
        self.spawn_finished();
        unsafe {
            if self.owner_write >= 0 {
                let _ = guardian_write_all(self.owner_write, &[command]);
                libc::close(self.owner_write);
                self.owner_write = -1;
            }
            loop {
                let result = libc::waitpid(self.pid, std::ptr::null_mut(), 0);
                if result == self.pid
                    || (result == -1
                        && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted)
                {
                    break;
                }
            }
        }
        self.finished = true;
    }

    fn finish(&mut self) {
        self.finish_with_command(b'K');
    }

    fn disarm(&mut self) {
        self.finish_with_command(b'D');
    }
}

#[cfg(unix)]
impl Drop for ProcessSupervisorGuardian {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(unix)]
type SpawnedProcessSupervisorTarget = (
    std::process::Child,
    StrictChildProcessTree,
    ProcessSupervisorGuardian,
);
#[cfg(not(unix))]
type SpawnedProcessSupervisorTarget = (std::process::Child, StrictChildProcessTree);

fn spawn_process_supervisor_target(
    request: &ProcessSupervisorRequest,
) -> Result<SpawnedProcessSupervisorTarget> {
    let mut command = std::process::Command::new(&request.program);
    command
        .args(&request.args)
        .current_dir(&request.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        configure_child_process_tree_command(&mut command);
        let mut guardian = ProcessSupervisorGuardian::install(&mut command)?;
        let spawned = spawn_configured_strict_child_process_tree(&mut command);
        guardian.spawn_finished();
        let (child, process_tree) = match spawned {
            Ok(spawned) => spawned,
            Err(error) => {
                guardian.disarm();
                return Err(error.into());
            }
        };
        Ok((child, process_tree, guardian))
    }
    #[cfg(not(unix))]
    {
        let (child, process_tree) = spawn_strict_child_process_tree(&mut command)?;
        Ok((child, process_tree))
    }
}

fn process_supervisor_child_exited(child: &mut std::process::Child) -> io::Result<bool> {
    #[cfg(unix)]
    {
        poll_child_exit_without_reaping(child)
    }
    #[cfg(not(unix))]
    {
        child.try_wait().map(|status| status.is_some())
    }
}

#[cfg(unix)]
fn exit_like_process_supervisor_child(
    status: std::process::ExitStatus,
    cancellation_signal: Option<i32>,
) -> ! {
    use std::os::unix::process::ExitStatusExt;

    if let Some(signal) = cancellation_signal.or_else(|| status.signal()) {
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }
        std::process::exit(128 + signal);
    }
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(not(unix))]
fn exit_like_process_supervisor_child(
    status: std::process::ExitStatus,
    _cancellation_signal: Option<i32>,
) -> ! {
    std::process::exit(status.code().unwrap_or(1));
}

/// Hidden, versioned exact-handle process owner used by desktop clients.
///
/// The first stdin line is a bounded launch request. Keeping stdin open is the
/// ownership lease: EOF cancels the strict target tree. On Windows an abrupt
/// supervisor death closes its kill-on-close Job. On Unix a pre-exec guardian
/// learns the reserved process-group id before the target can exec, then kills
/// that group if even SIGKILL closes the supervisor's owner pipe.
pub(crate) fn run_process_supervisor(protocol: &str) -> Result<()> {
    if protocol != PROCESS_SUPERVISOR_PROTOCOL {
        let _ = write_process_supervisor_control(serde_json::json!({
            "event": "error",
            "code": "unsupported_protocol",
            "message": "unsupported process-supervisor protocol",
        }));
        anyhow::bail!("unsupported process-supervisor protocol");
    }
    let request = {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        match read_process_supervisor_request(&mut stdin) {
            Ok(request) => request,
            Err(error) => {
                let _ = write_process_supervisor_control(serde_json::json!({
                    "event": "error",
                    "code": "invalid_request",
                    "message": "invalid process-supervisor launch request",
                }));
                return Err(error);
            }
        }
    };

    #[cfg(unix)]
    let (mut child, mut process_tree, mut guardian) =
        match spawn_process_supervisor_target(&request) {
            Ok(spawned) => spawned,
            Err(error) => {
                let _ = write_process_supervisor_control(serde_json::json!({
                    "event": "error",
                    "code": "spawn_failed",
                    "message": "the supervised target could not be started",
                }));
                return Err(error);
            }
        };
    #[cfg(not(unix))]
    let (mut child, mut process_tree) = match spawn_process_supervisor_target(&request) {
        Ok(spawned) => spawned,
        Err(error) => {
            let _ = write_process_supervisor_control(serde_json::json!({
                "event": "error",
                "code": "spawn_failed",
                "message": "the supervised target could not be started",
            }));
            return Err(error);
        }
    };

    let mut child_stdout = child
        .stdout
        .take()
        .context("supervised target did not expose stdout")?;
    let mut child_stderr = child
        .stderr
        .take()
        .context("supervised target did not expose stderr")?;
    let mut cancellation = SupervisedStreamCancellationGuard::install("process supervisor")?;
    write_process_supervisor_control(serde_json::json!({
        "event": "ready",
        "protocol": PROCESS_SUPERVISOR_PROTOCOL,
    }))?;

    let stdout_thread = thread::spawn(move || -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        io::copy(&mut child_stdout, &mut stdout)?;
        stdout.flush()
    });
    let stderr_thread = thread::spawn(move || -> io::Result<()> {
        let mut stderr = io::stderr().lock();
        io::copy(&mut child_stderr, &mut stderr)?;
        stderr.flush()
    });
    let (owner_tx, owner_rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut byte = [0_u8; 1];
        let outcome = loop {
            match stdin.read(&mut byte) {
                Ok(0) => break Ok(()),
                Ok(_) => {
                    break Err(anyhow::anyhow!(
                        "unexpected bytes after process-supervisor launch frame"
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => break Err(anyhow::Error::new(error)),
            }
        };
        let _ = owner_tx.send(outcome);
    });

    let pending_signal = cancellation.activate()?;
    let mut cancellation_signal = pending_signal;
    let mut owner_cancelled = pending_signal.is_some();
    while !owner_cancelled && !process_supervisor_child_exited(&mut child)? {
        if let Some(signal) = cancellation.cancelled_signal() {
            cancellation_signal = Some(signal);
            owner_cancelled = true;
            break;
        }
        match owner_rx.try_recv() {
            Ok(Ok(())) => owner_cancelled = true,
            Ok(Err(error)) => {
                eprintln!("coven process supervisor: {error:#}");
                owner_cancelled = true;
            }
            Err(mpsc::TryRecvError::Disconnected) => owner_cancelled = true,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if !owner_cancelled {
            thread::sleep(PROCESS_SUPERVISOR_POLL_INTERVAL);
        }
    }

    let termination_error = process_tree.terminate_tree().err();
    // The Unix guardian intentionally performs one final group kill when its
    // owner pipe closes. Reap it while the target group leader is still an
    // unreaped child, so the numeric PGID cannot be recycled underneath that
    // fail-safe signal.
    #[cfg(unix)]
    guardian.finish();
    let status = child
        .wait()
        .context("failed reaping the supervised target")?;
    let observed_signal = cancellation.finish()?.or(cancellation_signal);
    if let Some(error) = termination_error {
        return Err(anyhow::Error::new(error)
            .context("failed terminating the supervised target process tree"));
    }

    if !owner_cancelled {
        stdout_thread
            .join()
            .map_err(|_| anyhow::anyhow!("supervised stdout pump panicked"))??;
        stderr_thread
            .join()
            .map_err(|_| anyhow::anyhow!("supervised stderr pump panicked"))??;
    }
    exit_like_process_supervisor_child(status, observed_signal)
}

/// Run one non-interactive Codex turn through its supported JSONL protocol.
///
/// This intentionally uses ordinary OS pipes on every platform. In
/// particular, Windows npm installs expose `codex.cmd`; putting that shim
/// behind ConPTY can stall before the real Node/Codex process starts. The
/// existing command builder keeps a Windows prompt on stdin (`codex exec -`),
/// so this runner neither needs a shell nor puts user text in a batch command
/// line.
pub fn stream_codex_json<F>(command: &HarnessCommand, on_assistant: F) -> Result<CodexJsonRunResult>
where
    F: FnMut(&str) -> Result<()>,
{
    stream_codex_json_with_timeouts(
        command,
        codex_json_activity_timeout(),
        CODEX_POST_EXIT_DRAIN_TIMEOUT,
        on_assistant,
    )
}

#[cfg(test)]
fn stream_codex_json_with_timeout<F>(
    command: &HarnessCommand,
    activity_timeout: Duration,
    on_assistant: F,
) -> Result<CodexJsonRunResult>
where
    F: FnMut(&str) -> Result<()>,
{
    stream_codex_json_with_timeouts(
        command,
        activity_timeout,
        CODEX_POST_EXIT_DRAIN_TIMEOUT,
        on_assistant,
    )
}

fn stream_codex_json_with_timeouts<F>(
    command: &HarnessCommand,
    activity_timeout: Duration,
    post_exit_drain_timeout: Duration,
    on_assistant: F,
) -> Result<CodexJsonRunResult>
where
    F: FnMut(&str) -> Result<()>,
{
    stream_codex_json_with_budgets(
        command,
        activity_timeout,
        activity_timeout,
        post_exit_drain_timeout,
        on_assistant,
    )
}

fn stream_codex_json_with_budgets<F>(
    command: &HarnessCommand,
    startup_timeout: Duration,
    activity_timeout: Duration,
    post_exit_drain_timeout: Duration,
    mut on_assistant: F,
) -> Result<CodexJsonRunResult>
where
    F: FnMut(&str) -> Result<()>,
{
    let prompt_separator = command
        .args
        .iter()
        .position(|arg| arg == "--")
        .context("Codex JSON bridge expected a prompt separator")?;
    if !command.args[..prompt_separator]
        .iter()
        .any(|arg| arg == "--json")
    {
        anyhow::bail!("Codex JSON bridge expected `--json` to be constructed before the prompt");
    }

    let mut child_command = std::process::Command::new(&command.program);
    child_command
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(if command.stdin_prompt.is_some() {
            Stdio::piped()
        } else {
            // A one-shot prompt is already an argv positional on non-Windows
            // hosts. Do not inherit Coven's stdin: Codex may otherwise wait
            // for additional input after a completed request.
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.apply_environment(&mut child_command);
    let mut cancellation = SupervisedStreamCancellationGuard::install("Codex")?;
    if let Some(error) = supervised_stream_cancellation_error(&cancellation, "Codex turn") {
        anyhow::bail!(error);
    }
    let (mut child, mut process_tree) = spawn_strict_child_process_tree(&mut child_command)
        .with_context(|| {
            format!(
                "failed to spawn harness `{}` in Codex JSON mode",
                command.program()
            )
        })?;
    if let Some(signal) = cancellation.cancelled_signal() {
        let _ = terminate_and_wait_for_supervised_child(
            &mut process_tree,
            &mut child,
            "failed waiting for cancelled Codex process",
        );
        let signal = cancellation.finish()?.unwrap_or(signal);
        anyhow::bail!(supervised_stream_cancellation_error_for_signal(
            signal,
            "Codex turn"
        ));
    }

    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = terminate_and_wait_for_supervised_child(
                &mut process_tree,
                &mut child,
                "failed waiting for Codex after missing stdout",
            );
            anyhow::bail!("Codex JSON runner did not expose stdout");
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = terminate_and_wait_for_supervised_child(
                &mut process_tree,
                &mut child,
                "failed waiting for Codex after missing stderr",
            );
            anyhow::bail!("Codex JSON runner did not expose stderr");
        }
    };

    let (sender, receiver) = mpsc::channel();
    let stdin_pending = if let Some(prompt) = command.stdin_prompt.clone() {
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                let _ = terminate_and_wait_for_supervised_child(
                    &mut process_tree,
                    &mut child,
                    "failed waiting for Codex after missing stdin",
                );
                anyhow::bail!("Codex JSON runner did not expose stdin for its prompt");
            }
        };
        let sender = sender.clone();
        thread::spawn(move || {
            let result = (|| -> std::io::Result<()> {
                let mut stdin = stdin;
                stdin.write_all(&prompt)?;
                stdin.flush()
            })()
            .map_err(|error| format!("failed writing Codex prompt to stdin: {error}"));
            let _ = sender.send(CodexRunnerMessage::StdinComplete(result));
        });
        true
    } else {
        false
    };

    let stdout_sender = sender.clone();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let message = match line {
                Ok(line) => CodexStdoutMessage::Line(line),
                Err(error) => CodexStdoutMessage::ReadError(error.to_string()),
            };
            if stdout_sender
                .send(CodexRunnerMessage::Stdout(message))
                .is_err()
            {
                return;
            }
        }
        let _ = stdout_sender.send(CodexRunnerMessage::StdoutClosed);
    });
    let stderr_sender = sender.clone();
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut tail = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    append_bounded_tail(&mut tail, &buffer[..count], CODEX_STDERR_TAIL_BYTES)
                }
                Err(_) => break,
            }
        }
        let _ = stderr_sender.send(CodexRunnerMessage::StderrClosed(tail));
    });
    drop(sender);
    let activation_signal = match cancellation.activate() {
        Ok(signal) => signal,
        Err(error) => {
            let _ = terminate_and_wait_for_supervised_child(
                &mut process_tree,
                &mut child,
                "failed waiting for Codex after cancellation activation error",
            );
            return Err(error);
        }
    };

    let mut state = CodexJsonState::default();
    let mut last_activity = Instant::now();
    let mut waiting_for_initial_activity = true;
    let mut status = None;
    let mut direct_child_exited = false;
    let mut post_exit_deadline = None;
    let mut stdout_closed = false;
    let mut stderr_tail = None;
    let mut stdin_complete = !stdin_pending;

    loop {
        if let Some(signal) = activation_signal {
            state.protocol_error.get_or_insert_with(|| {
                supervised_stream_cancellation_error_for_signal(signal, "Codex turn")
            });
            status = Some(terminate_and_wait_for_supervised_child(
                &mut process_tree,
                &mut child,
                "failed waiting for cancelled Codex process",
            )?);
            break;
        }
        if let Some(error) = supervised_stream_cancellation_error(&cancellation, "Codex turn") {
            state.protocol_error.get_or_insert(error);
            status = Some(terminate_and_wait_for_supervised_child(
                &mut process_tree,
                &mut child,
                "failed waiting for cancelled Codex process",
            )?);
            break;
        }
        if !direct_child_exited {
            #[cfg(unix)]
            let observed_exit = match poll_child_exit_without_reaping(&child)
                .context("failed polling Codex JSON process")
            {
                Ok(observed_exit) => observed_exit,
                Err(error) => {
                    let _ = terminate_and_wait_for_supervised_child(
                        &mut process_tree,
                        &mut child,
                        "failed waiting for Codex after poll error",
                    );
                    return Err(error);
                }
            };
            #[cfg(not(unix))]
            let observed_exit = match child
                .try_wait()
                .context("failed polling Codex JSON process")?
            {
                Some(exit_status) => {
                    status = Some(exit_status);
                    true
                }
                None => false,
            };
            if observed_exit {
                direct_child_exited = true;
                // Reserve the Unix pid with WNOWAIT until the whole process
                // group has been terminated. Windows uses its stable Job
                // Object handle after try_wait.
                process_tree.terminate(&mut child);
                post_exit_deadline = Some(Instant::now() + post_exit_drain_timeout);
            }
        }
        if direct_child_exited && stdout_closed && stderr_tail.is_some() && stdin_complete {
            break;
        }

        let remaining = if let Some(deadline) = post_exit_deadline {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_default();
            if remaining.is_zero() {
                state.protocol_error.get_or_insert_with(|| {
                    "Codex exited but its output pipes remained open; terminated remaining process tree"
                        .to_string()
                });
                process_tree.terminate(&mut child);
                break;
            }
            remaining
        } else {
            let timeout = if waiting_for_initial_activity {
                startup_timeout
            } else {
                activity_timeout
            };
            let remaining = timeout
                .checked_sub(last_activity.elapsed())
                .unwrap_or_default();
            if remaining.is_zero() {
                state.protocol_error.get_or_insert_with(|| {
                    format!(
                        "Codex produced no machine-readable activity for {} seconds; the process was terminated",
                        timeout.as_secs()
                    )
                });
                status = Some(terminate_and_wait_for_supervised_child(
                    &mut process_tree,
                    &mut child,
                    "failed waiting for timed-out Codex process",
                )?);
                break;
            }
            remaining
        };

        match receiver.recv_timeout(remaining.min(CODEX_CHILD_POLL_INTERVAL)) {
            Ok(CodexRunnerMessage::Stdout(CodexStdoutMessage::Line(line))) => {
                match handle_codex_json_line(&line, &mut state, &mut on_assistant) {
                    Ok(true) => {
                        last_activity = Instant::now();
                        waiting_for_initial_activity = false;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        let _ = terminate_and_wait_for_supervised_child(
                            &mut process_tree,
                            &mut child,
                            "failed waiting for Codex after assistant callback error",
                        );
                        return Err(error);
                    }
                }
                if state.protocol_error.is_some() {
                    status = Some(terminate_and_wait_for_supervised_child(
                        &mut process_tree,
                        &mut child,
                        "failed waiting for failed Codex turn",
                    )?);
                    break;
                }
            }
            Ok(CodexRunnerMessage::Stdout(CodexStdoutMessage::ReadError(error))) => {
                state
                    .protocol_error
                    .get_or_insert_with(|| format!("failed reading Codex JSON output: {error}"));
                status = Some(terminate_and_wait_for_supervised_child(
                    &mut process_tree,
                    &mut child,
                    "failed waiting for Codex after stdout error",
                )?);
                break;
            }
            Ok(CodexRunnerMessage::StdoutClosed) => stdout_closed = true,
            Ok(CodexRunnerMessage::StderrClosed(tail)) => stderr_tail = Some(tail),
            Ok(CodexRunnerMessage::StdinComplete(Ok(()))) => stdin_complete = true,
            Ok(CodexRunnerMessage::StdinComplete(Err(error))) => {
                state.protocol_error.get_or_insert(error);
                status = Some(terminate_and_wait_for_supervised_child(
                    &mut process_tree,
                    &mut child,
                    "failed waiting for Codex after stdin write error",
                )?);
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                // All sender threads are gone (pipes closed, stdin written),
                // but the child may still be running with its stdio closed.
                // recv_timeout returns immediately on a disconnected channel,
                // so sleep explicitly to keep the child/deadline polling at
                // its normal cadence instead of busy-spinning until the
                // activity timeout fires.
                thread::sleep(remaining.min(CODEX_CHILD_POLL_INTERVAL));
            }
        }
    }

    // A signal can arrive just after the final polling iteration. Honor it
    // before reporting a completed turn so cancellation always reaches the
    // ledger and terminal result when the runner still owns the child tree.
    if let Some(error) = supervised_stream_cancellation_error(&cancellation, "Codex turn") {
        state.protocol_error.get_or_insert(error);
        process_tree.terminate(&mut child);
    }

    let status = match status {
        Some(status) => status,
        None => wait_for_supervised_child(&mut child, "failed waiting for Codex JSON process")?,
    };
    let stderr_tail = stderr_tail.unwrap_or_default();

    if !status.success() && state.protocol_error.is_none() {
        let code = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "an unknown status".to_string());
        let stderr = String::from_utf8_lossy(&stderr_tail).trim().to_string();
        let message = if stderr.is_empty() {
            format!("Codex exited with {code}")
        } else {
            format!("Codex exited with {code}: {stderr}")
        };
        state.protocol_error = Some(message);
    }
    if !state.emitted_assistant && state.protocol_error.is_none() {
        state.protocol_error = Some("Codex completed without an assistant message".to_string());
    }
    if let Some(signal) = cancellation.finish()? {
        state.protocol_error.get_or_insert_with(|| {
            supervised_stream_cancellation_error_for_signal(signal, "Codex turn")
        });
    }
    let failed = !status.success() || state.protocol_error.is_some();
    let exit_code = if failed {
        status.code().filter(|code| *code != 0).or(Some(1))
    } else {
        status.code()
    };
    Ok(CodexJsonRunResult {
        process: PtyRunResult {
            status: if failed { "failed" } else { "completed" },
            exit_code,
        },
        harness_session_id: state.harness_session_id,
        error: state.protocol_error,
        emitted_assistant: state.emitted_assistant,
    })
}

/// Parse one Codex JSONL frame. Returns whether it was a well-formed Codex
/// event, which is the unit that resets the runner's activity deadline.
fn handle_codex_json_line<F>(
    line: &str,
    state: &mut CodexJsonState,
    on_assistant: &mut F,
) -> Result<bool>
where
    F: FnMut(&str) -> Result<()>,
{
    let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
        // `--json` promises JSONL. Ignore an unexpected diagnostic here rather
        // than contaminating Coven's own stdout protocol; if Codex produces no
        // valid activity, the bounded timeout reports it.
        return Ok(false);
    };
    let Some(kind) = event.get("type").and_then(serde_json::Value::as_str) else {
        return Ok(false);
    };
    match kind {
        "thread.started" => {
            if let Some(thread_id) = event.get("thread_id").and_then(serde_json::Value::as_str) {
                state.harness_session_id = Some(thread_id.to_string());
            }
        }
        "item.completed" => {
            let Some(item) = event.get("item") else {
                return Ok(true);
            };
            if item.get("type").and_then(serde_json::Value::as_str) == Some("agent_message") {
                if let Some(text) = item.get("text").and_then(serde_json::Value::as_str) {
                    if !text.is_empty() {
                        on_assistant(text)?;
                        state.emitted_assistant = true;
                    }
                }
            }
        }
        "turn.failed" | "error" => {
            if let Some(message) = codex_event_error_message(&event) {
                state.protocol_error.get_or_insert(message);
            } else {
                state.protocol_error.get_or_insert_with(|| {
                    format!("Codex reported {kind} without an error message")
                });
            }
        }
        _ => {}
    }
    Ok(true)
}

fn append_bounded_tail(tail: &mut Vec<u8>, chunk: &[u8], max_bytes: usize) {
    if chunk.len() >= max_bytes {
        tail.clear();
        tail.extend_from_slice(&chunk[chunk.len() - max_bytes..]);
        return;
    }
    let excess = tail
        .len()
        .saturating_add(chunk.len())
        .saturating_sub(max_bytes);
    if excess > 0 {
        tail.drain(..excess);
    }
    tail.extend_from_slice(chunk);
}

fn codex_event_error_message(event: &serde_json::Value) -> Option<String> {
    if let Some(error) = event.get("error") {
        match error {
            serde_json::Value::String(message) if !message.trim().is_empty() => {
                return Some(message.clone());
            }
            serde_json::Value::Object(_) => {
                if let Some(message) = error
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .filter(|message| !message.trim().is_empty())
                {
                    return Some(message.to_string());
                }
            }
            _ => {}
        }
    }
    // Codex currently emits some `type:"error"` frames as
    // `{ "message": "..." }` rather than nesting the message under `error`.
    // Keep the bridge tolerant of both documented JSONL shapes.
    event
        .get("message")
        .and_then(serde_json::Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .map(ToOwned::to_owned)
}

#[allow(clippy::too_many_arguments)]
pub fn build_stream_harness_command_with_conversation(
    harness_id: &str,
    prompt: &str,
    cwd: &Path,
    forward_stdin: bool,
    conversation: Option<&crate::harness::ConversationHint>,
    familiar: Option<&crate::harness::FamiliarContext>,
    options: crate::harness::HarnessLaunchOptions<'_>,
) -> Result<HarnessCommand> {
    let (program, args) = crate::harness::command_parts_for_harness_with_conversation(
        harness_id,
        "",
        crate::harness::HarnessLaunchMode::Stream,
        conversation,
        familiar,
        options,
    )?;
    let mut args = stream_passthrough_args(args, forward_stdin);
    args.extend(["--".to_string(), prompt.to_string()]);
    let args = crate::harness::sanitize_argv_for_program(
        harness_id,
        crate::harness::HarnessLaunchMode::Stream,
        &program,
        args,
    );
    let (program, env_overrides) = prepare_harness_program(
        harness_id,
        crate::harness::HarnessLaunchMode::Stream,
        program,
    )?;
    Ok(HarnessCommand {
        program,
        args,
        cwd: cwd.to_path_buf(),
        stdin_prompt: None,
        env_overrides,
    })
}

fn stream_passthrough_args(args: Vec<String>, forward_stdin: bool) -> Vec<String> {
    if forward_stdin {
        return args;
    }
    let mut filtered = Vec::with_capacity(args.len());
    let mut iter = args.into_iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--input-format" && iter.peek().is_some_and(|next| next == "stream-json") {
            let _ = iter.next();
            continue;
        }
        filtered.push(arg);
    }
    filtered
}

pub fn run_attached_observed(
    command: &HarnessCommand,
    observer: Option<AttachedOutputObserver>,
) -> Result<PtyRunResult> {
    let pty_system = native_pty_system();
    run_attached_with_pty_system(command, pty_system.as_ref(), observer)
}

/// Run `command` on a PTY like `run_attached`, but capture the PTY output
/// instead of mirroring the raw bytes to stdout. Each captured chunk is
/// handed to `on_output` in order and is guaranteed valid UTF-8 (codepoints
/// split across reads are reassembled by `drain_detached_output`).
///
/// This is the `--stream-json` path for external harnesses without a native
/// machine-readable bridge: stdout must stay
/// JSONL-only, so the raw PTY output (ANSI escapes, prompts, partial lines)
/// is wrapped into `output` events by the caller rather than interleaving
/// with the frames (#307). Stdin is still forwarded to the PTY, matching
/// `run_attached`; raw terminal mode is never enabled because nothing is
/// echoed back to the caller's terminal.
#[cfg(not(windows))]
pub fn run_attached_captured(
    command: &HarnessCommand,
    mut on_output: Box<dyn FnMut(Vec<u8>) + Send + 'static>,
) -> Result<PtyRunResult> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(terminal_size())
        .context("failed to open PTY")?;
    let mut child = pair
        .slave
        .spawn_command(command.to_command_builder())
        .with_context(|| format!("failed to spawn harness `{}`", command.program()))?;

    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;
    let mut writer = pair
        .master
        .take_writer()
        .context("failed to open PTY writer")?;

    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let _ = io::copy(&mut stdin, &mut writer);
    });

    // Drain on this thread until the child closes its end of the PTY; EOF
    // (or EIO on Linux) arrives when the child exits, so the wait below
    // returns promptly.
    drain_detached_output(&mut reader, Some(&mut on_output));

    Ok(wait_for_child(&mut child))
}

/// Run a one-shot harness directly on inherited stdio without allocating a
/// pseudo-terminal. Windows Codex `exec` is reliable in this mode while its
/// ConPTY child can stall before producing output. Inherited handles preserve
/// the caller's stdout/stderr stream exactly (including Coven's JSON framing).
#[cfg(windows)]
pub fn run_piped_attached(
    command: &HarnessCommand,
    merge_stderr_to_stdout: bool,
) -> Result<PtyRunResult> {
    let mut child_command = std::process::Command::new(&command.program);
    child_command
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(if command.stdin_prompt.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        // In stream mode Codex duplicates its final answer on stdout while
        // stderr carries the complete labeled transcript that Cave's filter
        // consumes. Keep only the transcript to avoid rendering it twice.
        .stdout(if merge_stderr_to_stdout {
            Stdio::null()
        } else {
            Stdio::inherit()
        })
        .stderr(if merge_stderr_to_stdout {
            Stdio::piped()
        } else {
            Stdio::inherit()
        });
    command.apply_environment(&mut child_command);
    let mut child = child_command.spawn().with_context(|| {
        format!(
            "failed to spawn harness `{}` in piped mode",
            command.program()
        )
    })?;
    write_stdin_prompt(&mut child, command.stdin_prompt.as_deref())?;

    // Codex on Windows writes its complete `exec` transcript (including the
    // final assistant response) to stderr. `coven run --stream-json` is a
    // stdout protocol consumed by Cave, so forward that transcript to stdout
    // for stream clients while continuing to drain it concurrently.
    let stderr_forwarder = child.stderr.take().map(|mut stderr| {
        thread::spawn(move || -> io::Result<()> {
            let stdout = io::stdout();
            let mut stdout = stdout.lock();
            io::copy(&mut stderr, &mut stdout)?;
            stdout.flush()
        })
    });

    let status = child.wait().context("failed waiting for piped harness")?;
    if let Some(forwarder) = stderr_forwarder {
        forwarder
            .join()
            .map_err(|_| anyhow::anyhow!("stderr forwarding thread panicked"))?
            .context("failed forwarding harness stderr to stdout")?;
    }
    Ok(PtyRunResult {
        status: if status.success() {
            "completed"
        } else {
            "failed"
        },
        exit_code: status.code(),
    })
}

#[cfg(windows)]
pub fn run_piped_attached_observed(
    command: &HarnessCommand,
    observer: AttachedOutputObserver,
) -> Result<PtyRunResult> {
    run_piped_attached_observed_with_writers(
        command,
        observer,
        Box::new(io::stdout()),
        Box::new(io::stderr()),
    )
}

#[cfg(any(windows, test))]
fn run_piped_attached_observed_with_writers(
    command: &HarnessCommand,
    observer: AttachedOutputObserver,
    stdout_writer: Box<dyn Write + Send>,
    stderr_writer: Box<dyn Write + Send>,
) -> Result<PtyRunResult> {
    let mut child_command = std::process::Command::new(&command.program);
    child_command
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(if command.stdin_prompt.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.apply_environment(&mut child_command);
    let (mut child, mut process_tree) = spawn_strict_child_process_tree(&mut child_command)
        .with_context(|| {
            format!(
                "failed to spawn harness `{}` in observed piped mode",
                command.program()
            )
        })?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            process_tree.terminate(&mut child);
            let _ = child.wait();
            anyhow::bail!("observed piped harness did not expose stdout");
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            drop(stdout);
            process_tree.terminate(&mut child);
            let _ = child.wait();
            anyhow::bail!("observed piped harness did not expose stderr");
        }
    };
    let observer = Arc::new(Mutex::new(observer));

    // Drain both output pipes before writing the prompt. Otherwise a child that
    // emits more than one pipe capacity before reading stdin can deadlock with
    // the parent while both sides wait for the other to consume.
    let stdout_observer = Arc::clone(&observer);
    let stdout_forwarder = thread::spawn(move || -> Result<()> {
        let mut reader = stdout;
        let mut writer = stdout_writer;
        let callback: AttachedOutputObserver = Box::new(move |chunk| {
            let mut observer = stdout_observer
                .lock()
                .map_err(|_| anyhow::anyhow!("attached output observer lock poisoned"))?;
            observer(chunk)
        });
        copy_attached_output(&mut reader, &mut writer, Some(callback))
    });
    let stderr_observer = observer;
    let stderr_forwarder = thread::spawn(move || -> Result<()> {
        let mut reader = stderr;
        let mut writer = stderr_writer;
        let callback: AttachedOutputObserver = Box::new(move |chunk| {
            let mut observer = stderr_observer
                .lock()
                .map_err(|_| anyhow::anyhow!("attached output observer lock poisoned"))?;
            observer(chunk)
        });
        copy_attached_output(&mut reader, &mut writer, Some(callback))
    });

    let prompt_result = write_stdin_prompt_bytes(&mut child, command.stdin_prompt.as_deref());
    if prompt_result.is_err() {
        let termination_result = process_tree
            .terminate_tree()
            .context("failed terminating observed piped harness after prompt delivery failure");
        let wait_result = child
            .wait()
            .context("failed waiting for observed piped harness after prompt delivery failure");
        let stdout_result = stdout_forwarder
            .join()
            .map_err(|_| anyhow::anyhow!("stdout forwarding thread panicked"))
            .and_then(|result| result);
        let stderr_result = stderr_forwarder
            .join()
            .map_err(|_| anyhow::anyhow!("stderr forwarding thread panicked"))
            .and_then(|result| result);
        let mut error = prompt_result.expect_err("prompt result was checked as an error");
        for (context, cleanup) in [
            ("process-tree termination", termination_result.map(|_| ())),
            ("direct child wait", wait_result.map(|_| ())),
            ("stdout drain", stdout_result),
            ("stderr drain", stderr_result),
        ] {
            if let Err(cleanup) = cleanup {
                error = anyhow::anyhow!("{error:#}; {context} also failed: {cleanup:#}");
            }
        }
        return Err(error);
    }

    let pre_reap_wait = wait_for_child_exit_without_reaping(&child)
        .context("failed waiting for observed piped harness root exit");
    // A wrapper can exit while a descendant still owns stdout or stderr.
    // Terminate the strictly contained tree before joining either drain so
    // inherited handles reach EOF without changing the root's recorded status.
    let containment_cleanup = process_tree
        .terminate_tree()
        .context("failed terminating observed piped harness descendants after root exit");
    let status = child
        .wait()
        .context("failed reaping observed piped harness root");
    let stdout_result = stdout_forwarder
        .join()
        .map_err(|_| anyhow::anyhow!("stdout forwarding thread panicked"))
        .and_then(|result| result);
    let stderr_result = stderr_forwarder
        .join()
        .map_err(|_| anyhow::anyhow!("stderr forwarding thread panicked"))
        .and_then(|result| result);
    pre_reap_wait?;
    containment_cleanup?;
    let status = status?;
    stdout_result?;
    stderr_result?;
    Ok(PtyRunResult {
        status: if status.success() {
            "completed"
        } else {
            "failed"
        },
        exit_code: status.code(),
    })
}

/// Run a one-shot Windows harness through ordinary pipes while keeping stdout
/// available for Coven's stream-JSON protocol. Codex writes its labeled
/// transcript to stderr, so capture that stream and let the caller wrap it in
/// JSON `output` events; discard Codex's duplicate plain stdout answer.
#[cfg(windows)]
pub fn run_piped_attached_captured(
    command: &HarnessCommand,
    mut on_output: Box<dyn FnMut(Vec<u8>) + Send + 'static>,
) -> Result<PtyRunResult> {
    let mut child_command = std::process::Command::new(&command.program);
    child_command
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(if command.stdin_prompt.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command.apply_environment(&mut child_command);
    let mut child = child_command.spawn().with_context(|| {
        format!(
            "failed to spawn harness `{}` in captured piped mode",
            command.program()
        )
    })?;
    write_stdin_prompt(&mut child, command.stdin_prompt.as_deref())?;
    let mut stderr = child
        .stderr
        .take()
        .context("captured piped harness did not expose stderr")?;
    drain_detached_output(&mut stderr, Some(&mut on_output));
    let status = child.wait().context("failed waiting for piped harness")?;
    Ok(PtyRunResult {
        status: if status.success() {
            "completed"
        } else {
            "failed"
        },
        exit_code: status.code(),
    })
}

/// Run a harness in its native stream-JSON mode, framed by the caller (which
/// emits Coven's own `system.init` / `result` around the call). The command's
/// argv is built from the harness declaration (`stream_args`, continuity,
/// model, sandbox, and identity handling); this runner only spawns it and
/// normalizes each frame's top-level Coven session id before writing to `out`.
pub fn stream_harness<W: Write>(
    command: &HarnessCommand,
    forward_stdin: bool,
    harness_id: &str,
    ledger_session_id: &str,
    out: &mut W,
) -> Result<i32> {
    stream_harness_with_program_and_env(
        &command.program,
        &command.cwd,
        command.args.clone(),
        &command.env_overrides,
        forward_stdin,
        harness_id,
        ledger_session_id,
        out,
    )
}

enum NativeStreamMessage {
    Line(String),
    ReadError(String),
    Closed,
}

fn spawn_native_stdout_reader(
    stdout: std::process::ChildStdout,
    sender: mpsc::Sender<NativeStreamMessage>,
) {
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let message = match line {
                Ok(line) => NativeStreamMessage::Line(line),
                Err(error) => {
                    let _ = sender.send(NativeStreamMessage::ReadError(error.to_string()));
                    let _ = sender.send(NativeStreamMessage::Closed);
                    return;
                }
            };
            if sender.send(message).is_err() {
                return;
            }
        }
        let _ = sender.send(NativeStreamMessage::Closed);
    });
}

fn spawn_native_stdin_forwarder(child_stdin: std::process::ChildStdin, stopped: Arc<AtomicBool>) {
    #[cfg(unix)]
    thread::spawn(move || {
        use std::os::fd::AsRawFd;

        let stdin = io::stdin();
        let fd = stdin.as_raw_fd();
        let mut child_stdin = child_stdin;
        let mut buffer = [0_u8; 4096];
        while !stopped.load(Ordering::Relaxed) {
            let mut poll_fd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut poll_fd, 1, 50) };
            if ready == 0 {
                continue;
            }
            if ready < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            let count = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
            if count <= 0 {
                break;
            }
            if stopped.load(Ordering::Relaxed)
                || child_stdin.write_all(&buffer[..count as usize]).is_err()
                || child_stdin.flush().is_err()
            {
                break;
            }
        }
    });

    #[cfg(not(unix))]
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut handle = stdin.lock();
        let mut child_stdin = child_stdin;
        let mut buffer = String::new();
        while !stopped.load(Ordering::Relaxed) {
            buffer.clear();
            match handle.read_line(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if stopped.load(Ordering::Relaxed)
                        || child_stdin.write_all(buffer.as_bytes()).is_err()
                        || child_stdin.flush().is_err()
                    {
                        break;
                    }
                }
            }
        }
    });
}

fn normalize_native_stream_line<W: Write>(
    line: &str,
    harness_id: &str,
    ledger_session_id: &str,
    out: &mut W,
) -> Result<()> {
    if line.trim().is_empty() {
        return Ok(());
    }
    let mut frame: serde_json::Value = serde_json::from_str(line)
        .with_context(|| format!("invalid JSON from {harness_id} native stream"))?;
    let object = frame
        .as_object_mut()
        .with_context(|| format!("invalid JSON object from {harness_id} native stream"))?;
    if !object.contains_key("harness_session_id") {
        if let Some(native_session_id) = object
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
        {
            object.insert(
                "harness_session_id".to_string(),
                serde_json::Value::String(native_session_id),
            );
        }
    }
    object.insert(
        "session_id".to_string(),
        serde_json::Value::String(ledger_session_id.to_string()),
    );
    serde_json::to_writer(&mut *out, &frame)
        .with_context(|| format!("forwarding {harness_id} stdout"))?;
    writeln!(out).with_context(|| format!("forwarding {harness_id} stdout"))?;
    out.flush()
        .with_context(|| format!("flushing {harness_id} stdout"))
}

#[cfg(test)]
fn stream_harness_with_program<W: Write>(
    program: &str,
    cwd: &Path,
    args: Vec<String>,
    forward_stdin: bool,
    harness_id: &str,
    ledger_session_id: &str,
    out: &mut W,
) -> Result<i32> {
    stream_harness_with_program_and_env(
        program,
        cwd,
        args,
        &[],
        forward_stdin,
        harness_id,
        ledger_session_id,
        out,
    )
}

#[allow(clippy::too_many_arguments)]
fn stream_harness_with_program_and_env<W: Write>(
    program: &str,
    cwd: &Path,
    args: Vec<String>,
    env_overrides: &[(String, Option<String>)],
    forward_stdin: bool,
    harness_id: &str,
    ledger_session_id: &str,
    out: &mut W,
) -> Result<i32> {
    let mut command = std::process::Command::new(program);
    command
        .args(&args)
        .current_dir(cwd)
        .stdin(if forward_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    for (name, value) in env_overrides {
        match value {
            Some(value) => {
                command.env(name, value);
            }
            None => {
                command.env_remove(name);
            }
        }
    }
    let cancellation_context = format!("{harness_id} native stream");
    let mut cancellation = SupervisedStreamCancellationGuard::install(&cancellation_context)?;
    if let Some(signal) = cancellation.cancelled_signal() {
        let signal = cancellation.finish()?.unwrap_or(signal);
        anyhow::bail!(supervised_stream_cancellation_error_for_signal(
            signal,
            &cancellation_context
        ));
    }
    let (mut child, mut process_tree) = match spawn_strict_child_process_tree(&mut command)
        .with_context(|| format!("failed to spawn {harness_id} in stream-json mode"))
    {
        Ok(spawned) => spawned,
        Err(error) => {
            let cancellation_signal = cancellation.finish()?;
            if let Some(signal) = cancellation_signal {
                anyhow::bail!(supervised_stream_cancellation_error_for_signal(
                    signal,
                    &cancellation_context
                ));
            }
            return Err(error);
        }
    };
    if let Some(signal) = cancellation.cancelled_signal() {
        process_tree.terminate(&mut child);
        let _ = child.wait();
        let signal = cancellation.finish()?.unwrap_or(signal);
        anyhow::bail!(supervised_stream_cancellation_error_for_signal(
            signal,
            &cancellation_context
        ));
    }

    let child_stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            process_tree.terminate(&mut child);
            let _ = child.wait();
            anyhow::bail!("stdout requested but {harness_id} has no piped stdout");
        }
    };
    let stdin_stopped = Arc::new(AtomicBool::new(false));
    let child_stdin = if forward_stdin {
        match child.stdin.take() {
            Some(stdin) => Some(stdin),
            None => {
                process_tree.terminate(&mut child);
                let _ = child.wait();
                anyhow::bail!("stdin requested but {harness_id} has no piped stdin");
            }
        }
    } else {
        None
    };

    let (sender, receiver) = mpsc::channel();
    spawn_native_stdout_reader(child_stdout, sender);
    if let Some(child_stdin) = child_stdin {
        spawn_native_stdin_forwarder(child_stdin, Arc::clone(&stdin_stopped));
    }

    let activation_signal = match cancellation.activate() {
        Ok(signal) => signal,
        Err(error) => {
            stdin_stopped.store(true, Ordering::Relaxed);
            process_tree.terminate(&mut child);
            let _ = child.wait();
            return Err(error);
        }
    };

    let mut direct_child_exited = false;
    #[cfg(unix)]
    let status = None;
    #[cfg(not(unix))]
    let mut status = None;
    let mut stdout_closed = false;
    let mut post_exit_deadline = None;
    let result = (|| -> Result<()> {
        if let Some(signal) = activation_signal {
            anyhow::bail!(supervised_stream_cancellation_error_for_signal(
                signal,
                &cancellation_context
            ));
        }
        loop {
            if let Some(error) =
                supervised_stream_cancellation_error(&cancellation, &cancellation_context)
            {
                anyhow::bail!(error);
            }

            if !direct_child_exited {
                #[cfg(unix)]
                let observed_exit = poll_child_exit_without_reaping(&child)
                    .with_context(|| format!("polling {harness_id}"))?;
                #[cfg(not(unix))]
                let observed_exit = match child
                    .try_wait()
                    .with_context(|| format!("polling {harness_id}"))?
                {
                    Some(exit_status) => {
                        status = Some(exit_status);
                        true
                    }
                    None => false,
                };
                if observed_exit {
                    direct_child_exited = true;
                    // Unix's WNOWAIT keeps the pid reserved until the group is
                    // gone. On Windows the Job Object is a stable tree handle,
                    // so terminating it after try_wait is safe.
                    process_tree.terminate(&mut child);
                    post_exit_deadline = Some(Instant::now() + NATIVE_POST_EXIT_DRAIN_TIMEOUT);
                }
            }

            if direct_child_exited && stdout_closed {
                break;
            }
            let timeout = post_exit_deadline
                .map(|deadline| {
                    deadline
                        .checked_duration_since(Instant::now())
                        .unwrap_or_default()
                })
                .unwrap_or(CODEX_CHILD_POLL_INTERVAL);
            if direct_child_exited && timeout.is_zero() {
                break;
            }
            if stdout_closed {
                thread::sleep(timeout.min(CODEX_CHILD_POLL_INTERVAL));
                continue;
            }

            match receiver.recv_timeout(timeout.min(CODEX_CHILD_POLL_INTERVAL)) {
                Ok(NativeStreamMessage::Line(line)) => {
                    normalize_native_stream_line(&line, harness_id, ledger_session_id, out)?;
                }
                Ok(NativeStreamMessage::ReadError(error)) => {
                    anyhow::bail!("reading {harness_id} stdout: {error}");
                }
                Ok(NativeStreamMessage::Closed) => stdout_closed = true,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    stdout_closed = true;
                }
            }
        }
        Ok(())
    })();

    stdin_stopped.store(true, Ordering::Relaxed);
    if result.is_err() {
        process_tree.terminate(&mut child);
    }
    let waited_status = child
        .wait()
        .with_context(|| format!("waiting on {harness_id}"));
    let cancellation_signal = cancellation.finish()?;
    if let Some(signal) = cancellation_signal {
        anyhow::bail!(supervised_stream_cancellation_error_for_signal(
            signal,
            &cancellation_context
        ));
    }
    result?;
    let status = match status {
        Some(status) => status,
        None => waited_status?,
    };
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn stream_harness_with_claude_args<W: Write>(
    program: &str,
    cwd: &Path,
    session_id: &str,
    is_resume: bool,
    prompt: &str,
    forward_stdin: bool,
    system_prompt: Option<&str>,
    options: crate::harness::HarnessLaunchOptions<'_>,
    out: &mut W,
) -> Result<i32> {
    stream_harness_with_claude_args_and_permission_bypass(
        program,
        cwd,
        session_id,
        is_resume,
        prompt,
        forward_stdin,
        system_prompt,
        options,
        false,
        out,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn stream_harness_with_claude_args_and_permission_bypass<W: Write>(
    program: &str,
    cwd: &Path,
    session_id: &str,
    is_resume: bool,
    prompt: &str,
    forward_stdin: bool,
    system_prompt: Option<&str>,
    options: crate::harness::HarnessLaunchOptions<'_>,
    permission_bypass_enabled: bool,
    out: &mut W,
) -> Result<i32> {
    let normalized_model = options
        .model
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(crate::harness::normalize_model_id);

    let mut args = vec!["-p".to_string()];
    if permission_bypass_enabled {
        args.extend([
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
        ]);
    }
    if forward_stdin {
        args.extend(["--input-format".to_string(), "stream-json".to_string()]);
    }
    if let Some(sp) = system_prompt {
        args.extend(["--system-prompt".to_string(), sp.to_string()]);
    }
    if let Some(model) = normalized_model {
        args.extend(["--model".to_string(), model.to_string()]);
    }
    if let Some(effort) = options.claude_effort() {
        args.extend(["--effort".to_string(), effort.to_string()]);
    }
    args.extend([
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
    ]);
    if is_resume {
        args.extend(["--resume".to_string(), session_id.to_string()]);
    } else {
        args.extend(["--session-id".to_string(), session_id.to_string()]);
    }
    args.extend(["--".to_string(), prompt.to_string()]);

    stream_harness_with_program(program, cwd, args, forward_stdin, "claude", session_id, out)
}

#[allow(dead_code)]
pub fn spawn_detached(command: &HarnessCommand) -> Result<DetachedPtySession> {
    spawn_detached_with_observer(command, None)
}

/// Handle returned by `spawn_piped_with_observer`. The child handle itself is
/// owned by the internal wait thread (so `wait()` can block without blocking
/// cancellation); the caller gets writable stdin plus the strict process-tree
/// handle that was established before the Windows child started executing.
pub struct PipedSession {
    input: Box<dyn Write + Send>,
    process_tree: SharedStrictChildProcessTree,
    prompt_delivery: Option<PipedPromptDelivery>,
}

const PIPED_PROMPT_DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);
const PIPED_PROMPT_DELIVERY_POLL_INTERVAL: Duration = Duration::from_millis(10);
const PIPED_PROMPT_REAP_TIMEOUT: Duration = Duration::from_secs(1);
const PIPED_CHILD_WAIT_PENDING: u8 = 0;
const PIPED_CHILD_REAPED: u8 = 1;
const PIPED_CHILD_WAIT_FAILED: u8 = 2;

/// Privacy-safe disposition for a piped launch whose cleanup did not prove
/// child-process quiescence. Callers may preserve killable runtime ownership
/// without exposing either the prompt-delivery or cleanup failure.
#[derive(Debug)]
pub(crate) struct PipedLaunchCleanupRetainedError;

impl std::fmt::Display for PipedLaunchCleanupRetainedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("piped runtime ownership may remain after launch cleanup")
    }
}

impl std::error::Error for PipedLaunchCleanupRetainedError {}

struct PipedPromptDelivery {
    stdin: Option<std::process::ChildStdin>,
    prompt: Option<Vec<u8>>,
    outcome: Arc<PipedPromptOutcome>,
}

struct PipedPromptOutcome {
    delivered: Mutex<Option<bool>>,
    ready: Condvar,
}

impl PipedPromptOutcome {
    fn new(delivered: Option<bool>) -> Self {
        Self {
            delivered: Mutex::new(delivered),
            ready: Condvar::new(),
        }
    }

    fn finish(&self, delivered: bool) {
        let mut state = self
            .delivered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.is_none() {
            *state = Some(delivered);
            self.ready.notify_all();
        }
    }

    fn wait(&self) -> bool {
        let mut state = self
            .delivered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while state.is_none() {
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        state.unwrap_or(false)
    }
}

impl PipedSession {
    pub(crate) fn cancellation_handle(&self) -> SharedStrictChildProcessTree {
        self.process_tree.clone()
    }

    /// Transfer cancellation ownership before delivering a one-shot prompt.
    ///
    /// `register` must retain the supplied process-tree handle whenever it
    /// returns success. This ordering is deliberate: a large prompt may block
    /// in an OS pipe, so daemon cancellation and shutdown must own the child
    /// before prompt delivery starts.
    pub(crate) fn activate<R>(
        self,
        register: impl FnOnce(Box<dyn Write + Send>, SharedStrictChildProcessTree) -> Result<R>,
    ) -> Result<R> {
        self.activate_with_prompt_timeout(PIPED_PROMPT_DELIVERY_TIMEOUT, register)
    }

    fn activate_with_prompt_timeout<R>(
        self,
        prompt_timeout: Duration,
        register: impl FnOnce(Box<dyn Write + Send>, SharedStrictChildProcessTree) -> Result<R>,
    ) -> Result<R> {
        let Self {
            input,
            process_tree,
            prompt_delivery,
        } = self;
        let cleanup_tree = process_tree.clone();
        let registered = match register(input, process_tree) {
            Ok(registered) => registered,
            Err(error) => {
                drop(prompt_delivery);
                return Err(cleanup_piped_launch_failure(error, &cleanup_tree));
            }
        };

        if let Some(delivery) = prompt_delivery {
            delivery.deliver(prompt_timeout, &cleanup_tree)?;
        }
        Ok(registered)
    }
}

impl PipedPromptDelivery {
    fn deliver(
        mut self,
        timeout: Duration,
        process_tree: &SharedStrictChildProcessTree,
    ) -> Result<()> {
        let outcome = Arc::clone(&self.outcome);
        let stdin = self
            .stdin
            .take()
            .context("piped prompt stdin was already consumed")?;
        let prompt = self
            .prompt
            .take()
            .context("piped prompt bytes were already consumed")?;
        let result = deliver_piped_prompt(stdin, prompt, timeout, process_tree);
        outcome.finish(result.is_ok());
        result
    }
}

impl Drop for PipedPromptDelivery {
    fn drop(&mut self) {
        // Registration rejection or caller abandonment means the launch-time
        // prompt did not land. Wake the exit thread so it can persist a failed
        // terminal result instead of trusting an unrelated root exit code.
        self.outcome.finish(false);
    }
}

fn deliver_piped_prompt(
    stdin: std::process::ChildStdin,
    prompt: Vec<u8>,
    timeout: Duration,
    process_tree: &SharedStrictChildProcessTree,
) -> Result<()> {
    if process_tree.is_terminated() {
        return Err(cleanup_piped_launch_failure(
            anyhow::anyhow!(
                "harness process tree was terminated before its stdin prompt could be delivered"
            ),
            process_tree,
        ));
    }

    let (completed_tx, completed_rx) = mpsc::sync_channel(1);
    let writer = thread::Builder::new()
        .name("coven-piped-prompt".into())
        .spawn(move || {
            let mut stdin = stdin;
            let result = stdin
                .write_all(&prompt)
                .and_then(|_| stdin.flush())
                .map_err(|error| {
                    anyhow::Error::new(error).context("failed writing harness prompt to stdin")
                });
            // Dropping stdin after the exact write publishes EOF so a
            // one-shot harness can begin processing the complete prompt.
            drop(stdin);
            let _ = completed_tx.send(result);
        });
    let writer = match writer {
        Ok(writer) => writer,
        Err(error) => {
            return Err(cleanup_piped_launch_failure(
                anyhow::Error::new(error)
                    .context("failed to start bounded harness prompt delivery"),
                process_tree,
            ));
        }
    };

    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            drop(writer);
            return Err(cleanup_piped_launch_failure(
                anyhow::anyhow!(
                    "timed out after {} ms while writing harness prompt to stdin",
                    timeout.as_millis()
                ),
                process_tree,
            ));
        }
        let wait = remaining.min(PIPED_PROMPT_DELIVERY_POLL_INTERVAL);
        match completed_rx.recv_timeout(wait) {
            Ok(Ok(())) => {
                if writer.join().is_err() {
                    return Err(cleanup_piped_launch_failure(
                        anyhow::anyhow!("harness prompt writer panicked after delivery"),
                        process_tree,
                    ));
                }
                return Ok(());
            }
            Ok(Err(error)) => {
                let _ = writer.join();
                return Err(cleanup_piped_launch_failure(error, process_tree));
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = writer.join();
                return Err(cleanup_piped_launch_failure(
                    anyhow::anyhow!("harness prompt writer stopped without reporting delivery"),
                    process_tree,
                ));
            }
            Err(RecvTimeoutError::Timeout) if process_tree.is_terminated() => {
                drop(writer);
                return Err(cleanup_piped_launch_failure(
                    anyhow::anyhow!(
                        "harness process tree was terminated while writing its stdin prompt"
                    ),
                    process_tree,
                ));
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn cleanup_piped_launch_failure(
    primary: anyhow::Error,
    process_tree: &SharedStrictChildProcessTree,
) -> anyhow::Error {
    piped_launch_cleanup_error(
        primary,
        process_tree.terminate_and_wait(PIPED_PROMPT_REAP_TIMEOUT),
    )
}

fn piped_launch_cleanup_error(primary: anyhow::Error, cleanup: Result<()>) -> anyhow::Error {
    match cleanup {
        Ok(()) => primary,
        Err(_) => anyhow::Error::new(PipedLaunchCleanupRetainedError),
    }
}

fn wait_for_piped_child_reap(wait_state: &AtomicU8, timeout: Duration) -> u8 {
    let deadline = Instant::now() + timeout;
    loop {
        let state = wait_state.load(Ordering::Acquire);
        if state != PIPED_CHILD_WAIT_PENDING || Instant::now() >= deadline {
            return state;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(debug_assertions)]
fn wait_at_piped_prepublication_test_barrier() -> Result<()> {
    let Some(barrier_dir) = std::env::var_os("COVEN_TEST_PIPED_PREPUBLICATION_BARRIER_DIR") else {
        return Ok(());
    };
    let barrier_dir = PathBuf::from(barrier_dir);
    std::fs::create_dir_all(&barrier_dir)
        .context("failed creating piped pre-publication test barrier")?;
    std::fs::write(barrier_dir.join("ready"), b"ready\n")
        .context("failed publishing piped pre-publication test barrier")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !barrier_dir.join("release").exists() {
        anyhow::ensure!(
            Instant::now() < deadline,
            "timed out at piped pre-publication test barrier"
        );
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

#[cfg(not(debug_assertions))]
fn wait_at_piped_prepublication_test_barrier() -> Result<()> {
    Ok(())
}

/// Spawn `command` as a plain piped child process (no PTY) and stream its
/// stdout to `observer`. Used by stream-mode harness launches where the
/// child reads newline-delimited JSON from stdin and writes
/// newline-delimited JSON to stdout — wrapping in a PTY would add ANSI
/// escapes the child wouldn't otherwise emit. Lifecycle mirrors
/// `spawn_detached_with_observer`: a background thread drains stdout and
/// fires `on_exit` when the child finishes. Stderr is line-buffered and
/// forwarded to `observer.on_output` wrapped in a stream-json
/// `{"type":"system","subtype":"stderr","text":"…"}` envelope so chat
/// surfaces auth/setup errors instead of swallowing them.
pub fn spawn_piped_with_observer(
    command: &HarnessCommand,
    observer: Option<DetachedPtyObserver>,
    wrap_stderr_as_stream_json: bool,
) -> Result<PipedSession> {
    use std::process::Command as StdCommand;
    use std::sync::{Arc, Mutex as StdMutex};

    let mut std_command = StdCommand::new(&command.program);
    std_command.args(&command.args);
    std_command.current_dir(&command.cwd);
    std_command.stdin(Stdio::piped());
    std_command.stdout(Stdio::piped());
    std_command.stderr(Stdio::piped());
    command.apply_environment(&mut std_command);
    // Reuse strict containment. On Windows the console-subsystem child starts
    // suspended and hidden, enters a KILL_ON_JOB_CLOSE job before its first
    // instruction, and only then resumes. Unix additionally installs an
    // independent guardian before fork/exec; daemon-process death closes its
    // owner pipe and kills the new process group even if shutdown raced before
    // the request handler could publish its in-process cancellation handle.
    #[cfg(unix)]
    let (mut child, mut process_tree, mut guardian) = {
        configure_child_process_tree_command(&mut std_command);
        let mut guardian = ProcessSupervisorGuardian::install(&mut std_command)?;
        let spawned = spawn_configured_strict_child_process_tree(&mut std_command);
        guardian.spawn_finished();
        match spawned {
            Ok((child, process_tree)) => (child, process_tree, guardian),
            Err(error) => {
                guardian.disarm();
                return Err(error).with_context(|| {
                    format!(
                        "failed to spawn harness `{}` in piped mode",
                        command.program
                    )
                });
            }
        }
    };
    #[cfg(not(unix))]
    let (mut child, mut process_tree) = spawn_strict_child_process_tree(&mut std_command)
        .with_context(|| {
            format!(
                "failed to spawn harness `{}` in piped mode",
                command.program
            )
        })?;

    let stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            process_tree.terminate(&mut child);
            #[cfg(unix)]
            guardian.finish();
            let _ = child.wait();
            anyhow::bail!("failed to take child stdin in piped mode");
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            drop(stdin);
            process_tree.terminate(&mut child);
            #[cfg(unix)]
            guardian.finish();
            let _ = child.wait();
            anyhow::bail!("failed to take child stdout in piped mode");
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            drop(stdin);
            drop(stdout);
            process_tree.terminate(&mut child);
            #[cfg(unix)]
            guardian.finish();
            let _ = child.wait();
            anyhow::bail!("failed to take child stderr in piped mode");
        }
    };
    let prompt_outcome = Arc::new(PipedPromptOutcome::new(
        command.stdin_prompt.is_none().then_some(true),
    ));
    let (stdin, prompt_delivery): (Box<dyn Write + Send>, Option<PipedPromptDelivery>) =
        if let Some(prompt) = command.stdin_prompt.clone() {
            (
                Box::new(io::sink()),
                Some(PipedPromptDelivery {
                    stdin: Some(stdin),
                    prompt: Some(prompt),
                    outcome: Arc::clone(&prompt_outcome),
                }),
            )
        } else {
            (Box::new(stdin), None)
        };

    // Share the on_output callback between the stdout and stderr drain
    // threads — both want to feed the same observer pipeline. `on_exit` is
    // moved into the stdout thread (it fires exactly once when the child
    // exits). If no observer was supplied, both callbacks are no-ops.
    let DetachedPtyObserver { on_output, on_exit } = observer.unwrap_or(DetachedPtyObserver {
        on_output: Box::new(|_| {}),
        on_exit: Box::new(|_| {}),
    });
    let on_output_shared = Arc::new(StdMutex::new(on_output));

    // Stderr drain: line-buffered, wrapped in a stream-json system
    // envelope so chat can render auth/setup messages as system lines
    // rather than dropping them silently. Reads raw bytes with
    // `read_until(b'\n')` + `from_utf8_lossy` so non-UTF-8 stderr
    // (rare but seen in some sandboxed environments) doesn't truncate
    // the stream at the first decode error — which `BufRead::lines()`
    // would do.
    let stderr_callback = Arc::clone(&on_output_shared);
    let stderr_thread = thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut buf: Vec<u8> = Vec::with_capacity(256);
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    // Strip the trailing newline (if any) for cleaner
                    // display; the JSON envelope adds its own.
                    let trimmed = match buf.last() {
                        Some(b'\n') => &buf[..buf.len() - 1],
                        _ => &buf[..],
                    };
                    let line = String::from_utf8_lossy(trimmed);
                    let mut payload = if wrap_stderr_as_stream_json {
                        serde_json::json!({
                            "type": "system",
                            "subtype": "stderr",
                            "text": line,
                        })
                        .to_string()
                    } else {
                        line.into_owned()
                    };
                    payload.push('\n');
                    if let Ok(mut cb) = stderr_callback.lock() {
                        cb(payload.into_bytes());
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Start both output drains before this function returns and therefore
    // before `PipedSession::activate` can begin prompt delivery. This avoids
    // the classic duplex pipe deadlock where the parent fills stdin while the
    // child fills stdout before either side reads.
    //
    // The wait thread owns `child`; cancellation uses the independently owned
    // process-group/Job Object handle, so it never needs to lock the `Child`
    // around a blocking wait.
    let child_wait_state = Arc::new(AtomicU8::new(PIPED_CHILD_WAIT_PENDING));
    let wait_state = Arc::clone(&child_wait_state);
    let exit_callback_complete = Arc::new(AtomicBool::new(false));
    let callback_complete = Arc::clone(&exit_callback_complete);
    #[cfg(unix)]
    let process_tree = SharedStrictChildProcessTree::new(
        process_tree,
        guardian,
        Arc::clone(&child_wait_state),
        Arc::clone(&exit_callback_complete),
    );
    #[cfg(not(unix))]
    let process_tree = SharedStrictChildProcessTree::new(
        process_tree,
        Arc::clone(&child_wait_state),
        Arc::clone(&exit_callback_complete),
    );
    let wait_process_tree = Arc::downgrade(&process_tree.process_tree);
    #[cfg(unix)]
    let wait_guardian = Arc::downgrade(&process_tree.guardian);
    let stdout_callback = Arc::clone(&on_output_shared);
    let stdout_thread = thread::spawn(move || {
        let mut reader = stdout;
        let mut bridge: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            if let Ok(mut cb) = stdout_callback.lock() {
                cb(chunk);
            }
        });
        drain_detached_output(&mut reader, Some(&mut bridge));
    });
    thread::spawn(move || {
        // Observe root exit concurrently with both pipe drains, without
        // reaping it. A wrapper can exit while a descendant keeps stdout or
        // stderr open forever; waiting for EOF first would never reach tree
        // cleanup. WNOWAIT (or the stable Windows process handle) reserves the
        // root identity until its entire containment unit is terminated.
        let pre_reap_wait_failed = wait_for_child_exit_without_reaping(&child).is_err();
        let containment_cleanup_failed =
            wait_process_tree
                .upgrade()
                .is_some_and(|process_tree| match process_tree.lock() {
                    Ok(mut process_tree) => process_tree.terminate_tree().is_err(),
                    Err(poisoned) => poisoned.into_inner().terminate_tree().is_err(),
                });
        // Close and reap the independent Unix guardian while the direct child
        // remains WNOWAIT-reserved. Its final group signal can therefore never
        // target a recycled numeric PGID.
        #[cfg(unix)]
        if let Some(guardian) = wait_guardian.upgrade() {
            match guardian.lock() {
                Ok(mut guardian) => guardian.finish(),
                Err(poisoned) => poisoned.into_inner().finish(),
            }
        }
        // Tree termination closes every inherited output pipe. Join both
        // drains before publishing quiescence so no observer callback can run
        // after `/kill` or shutdown reports completion.
        let stdout_drain_failed = stdout_thread.join().is_err();
        let stderr_drain_failed = stderr_thread.join().is_err();
        let (mut result, mut state) = match child.wait() {
            Ok(status) => (
                PtyRunResult {
                    status: if status.success() {
                        "completed"
                    } else {
                        "failed"
                    },
                    exit_code: status.code(),
                },
                PIPED_CHILD_REAPED,
            ),
            Err(_) => (
                PtyRunResult {
                    status: "failed",
                    exit_code: None,
                },
                PIPED_CHILD_WAIT_FAILED,
            ),
        };
        // A successful cancellation response is a quiescence boundary for
        // consumers such as Cave: do not publish completion until both pipe
        // drains have reached EOF. Otherwise a descendant that inherited
        // stderr could still append output after `/kill` returned.
        if stdout_drain_failed
            || stderr_drain_failed
            || pre_reap_wait_failed
            || containment_cleanup_failed
        {
            state = PIPED_CHILD_WAIT_FAILED;
        }
        wait_state.store(state, Ordering::Release);
        // Prompt-delivery cleanup may itself wait for the reaped/quiescent
        // state above, so publish that barrier first. Do not publish on_exit,
        // however, until delivery has a terminal outcome: a root can exit 0
        // after closing stdin while the required prompt write fails with
        // EPIPE. In that race the launch and durable exit event must both be
        // failed, never completed.
        if !prompt_outcome.wait() {
            result.status = "failed";
        }
        on_exit(result);
        callback_complete.store(true, Ordering::Release);
    });

    // Integration-only barrier used to prove that daemon termination remains
    // bounded when a request thread has spawned a strict child but has not yet
    // returned the cancellation handle to LiveSessionRuntime. On Unix, the
    // guardian owner pipe is the process-death backstop for this exact window;
    // on Windows, the already-attached Job Object is.
    if let Err(error) = wait_at_piped_prepublication_test_barrier() {
        return Err(cleanup_piped_launch_failure(error, &process_tree));
    }

    Ok(PipedSession {
        input: stdin,
        process_tree,
        prompt_delivery,
    })
}

pub fn spawn_detached_with_observer(
    command: &HarnessCommand,
    observer: Option<DetachedPtyObserver>,
) -> Result<DetachedPtySession> {
    spawn_detached_with_observer_and_timeout(command, observer, detached_startup_timeout())
}

fn spawn_detached_with_observer_and_timeout(
    command: &HarnessCommand,
    observer: Option<DetachedPtyObserver>,
    startup_timeout: Option<Duration>,
) -> Result<DetachedPtySession> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(terminal_size())
        .context("failed to open PTY")?;
    let portable_pty::PtyPair { master, slave } = pair;
    let mut child = slave
        .spawn_command(command.to_command_builder())
        .with_context(|| format!("failed to spawn harness `{}`", command.program()))?;
    drop(slave);

    let mut reader = master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;
    let writer = master.take_writer().context("failed to open PTY writer")?;
    let shared_writer = spawn_shared_pty_writer(writer);
    let input: Box<dyn Write + Send> = Box::new(shared_writer.clone());
    let killer = shared_pty_killer(child.as_ref());
    let timeout_killer = killer.clone_killer();

    // 0 = waiting for meaningful output, 1 = output or exit observed,
    // 2 = startup timeout won the race. VT queries do not count because the
    // filter consumes them before this state is advanced.
    let startup_state = Arc::new(AtomicU8::new(0));
    let DetachedPtyObserver { on_output, on_exit } = observer.unwrap_or(DetachedPtyObserver {
        on_output: Box::new(|_| {}),
        on_exit: Box::new(|_| {}),
    });
    let on_output = Arc::new(Mutex::new(on_output));

    if let Some(startup_timeout) = startup_timeout {
        let timeout_state = Arc::clone(&startup_state);
        let timeout_output = Arc::clone(&on_output);
        thread::spawn(move || {
            thread::sleep(startup_timeout);
            if timeout_state
                .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if let Ok(mut callback) = timeout_output.lock() {
                    callback(
                        format!(
                            "Coven stopped the detached PTY: no meaningful output was produced before the startup timeout ({} ms).\n",
                            startup_timeout.as_millis()
                        )
                        .into_bytes(),
                    );
                }
                let mut timeout_killer = timeout_killer;
                let _ = timeout_killer.kill();
            }
        });
    }

    let (child_exit_tx, child_exit_rx) = mpsc::channel();
    let child_exit_state = Arc::clone(&startup_state);
    thread::spawn(move || {
        // The cloned read/write pipe handles do not own the Windows HPCON.
        // Keep the MasterPty alive until the child exits; dropping it when
        // this function returned was the source of intermittent 0x7fffffff
        // ConPTY exits with no output (#329).
        let _master = master;
        let result = wait_for_child(&mut child);
        child_exit_state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .ok();
        drop(_master);
        let _ = child_exit_tx.send(result);
    });

    thread::spawn(move || {
        let output_state = Arc::clone(&startup_state);
        let output_callback = Arc::clone(&on_output);
        let mut meaningful_detector = MeaningfulOutputDetector::default();
        let mut bridge: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            if meaningful_detector.push(&chunk) {
                output_state
                    .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                    .ok();
            }
            if let Ok(mut callback) = output_callback.lock() {
                callback(chunk);
            }
        });
        let mut terminal_reply = |reply| shared_writer.queue_terminal_reply(reply);
        drain_detached_pty_output(&mut reader, &mut terminal_reply, Some(&mut bridge));
        let mut result = child_exit_rx.recv().unwrap_or(PtyRunResult {
            status: "failed",
            exit_code: None,
        });
        let previous = startup_state.swap(1, Ordering::AcqRel);
        if previous == 2 {
            result = PtyRunResult {
                status: "failed",
                exit_code: None,
            };
        }
        on_exit(result);
    });

    Ok(DetachedPtySession {
        input,
        killer: Box::new(killer),
    })
}

#[cfg(all(test, windows))]
pub(crate) fn spawn_detached_with_observer_for_test(
    command: &HarnessCommand,
    observer: DetachedPtyObserver,
    startup_timeout: Duration,
) -> Result<DetachedPtySession> {
    spawn_detached_with_observer_and_timeout(command, Some(observer), Some(startup_timeout))
}

#[cfg(all(test, windows))]
pub(crate) fn windows_detached_stub_command(
    build_dir: &Path,
    mode: &str,
    auxiliary_file: Option<&Path>,
) -> Result<HarnessCommand> {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/windows_detached_pty_stub.rs");
    let executable = build_dir.join("windows-detached-pty-stub.exe");
    let compile = std::process::Command::new("rustc.exe")
        .args(["--edition=2021", "-o"])
        .arg(&executable)
        .arg(&source)
        .output()
        .context("failed to compile native Windows detached-PTY stub")?;
    anyhow::ensure!(
        compile.status.success(),
        "native Windows detached-PTY stub failed to compile: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let mut args = vec![mode.to_string()];
    if let Some(auxiliary_file) = auxiliary_file {
        args.push(auxiliary_file.to_string_lossy().into_owned());
    }
    Ok(HarnessCommand {
        program: executable.to_string_lossy().into_owned(),
        args,
        cwd: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        stdin_prompt: None,
        env_overrides: Vec::new(),
    })
}

#[cfg(all(test, windows))]
pub(crate) fn windows_console_probe_command(build_dir: &Path) -> Result<HarnessCommand> {
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/windows_console_probe.rs");
    let executable = build_dir.join("windows-console-probe.exe");
    let compile = std::process::Command::new("rustc.exe")
        .args(["--edition=2021", "-o"])
        .arg(&executable)
        .arg(&source)
        .output()
        .context("failed to compile native Windows console probe")?;
    anyhow::ensure!(
        compile.status.success(),
        "native Windows console probe failed to compile: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    Ok(HarnessCommand {
        program: executable.to_string_lossy().into_owned(),
        args: Vec::new(),
        cwd: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        stdin_prompt: None,
        env_overrides: Vec::new(),
    })
}

#[cfg(all(test, any(unix, windows)))]
pub(crate) fn piped_prompt_probe_command(
    build_dir: &Path,
    mode: &str,
    first: &str,
    second: Option<&Path>,
    prompt: Vec<u8>,
) -> Result<HarnessCommand> {
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/piped_prompt_probe.rs");
    let executable = build_dir.join(if cfg!(windows) {
        "piped-prompt-probe.exe"
    } else {
        "piped-prompt-probe"
    });
    let rustc = if cfg!(windows) { "rustc.exe" } else { "rustc" };
    let compile = std::process::Command::new(rustc)
        .args(["--edition=2021", "-o"])
        .arg(&executable)
        .arg(&source)
        .output()
        .context("failed to compile native piped-prompt probe")?;
    anyhow::ensure!(
        compile.status.success(),
        "native piped-prompt probe failed to compile: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let mut args = vec![mode.to_string(), first.to_string()];
    if let Some(second) = second {
        args.push(second.to_string_lossy().into_owned());
    }
    Ok(HarnessCommand {
        program: executable.to_string_lossy().into_owned(),
        args,
        cwd: build_dir.to_path_buf(),
        stdin_prompt: (!prompt.is_empty()).then_some(prompt),
        env_overrides: Vec::new(),
    })
}

fn detached_startup_timeout() -> Option<Duration> {
    #[cfg(not(windows))]
    {
        None
    }
    #[cfg(windows)]
    {
        if let Some(milliseconds) = std::env::var("COVEN_PTY_STARTUP_TIMEOUT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
        {
            return Some(Duration::from_millis(milliseconds.max(1)));
        }
        Some(DETACHED_STARTUP_TIMEOUT)
    }
}

fn shared_pty_killer(child: &dyn portable_pty::Child) -> SharedPtyKiller {
    #[cfg(windows)]
    let job_handle = child.process_id().and_then(assign_process_to_job);
    SharedPtyKiller {
        inner: Arc::new(Mutex::new(PtyKillerInner {
            fallback: child.clone_killer(),
            #[cfg(windows)]
            job_handle,
        })),
    }
}

#[cfg(windows)]
fn assign_process_to_job(pid: u32) -> Option<windows_sys::Win32::Foundation::HANDLE> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{
            JobObjects::{AssignProcessToJobObject, CreateJobObjectW},
            Threading::{OpenProcess, PROCESS_ALL_ACCESS},
        },
    };
    // SAFETY: all returned handles are checked and either closed here or
    // transferred to PtyKillerInner for exclusive ownership.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job == INVALID_HANDLE_VALUE || job == 0 as _ {
            return None;
        }
        let process = OpenProcess(PROCESS_ALL_ACCESS, 0, pid);
        if process == INVALID_HANDLE_VALUE || process == 0 as _ {
            CloseHandle(job);
            return None;
        }
        let assigned = AssignProcessToJobObject(job, process);
        CloseHandle(process);
        if assigned == 0 {
            CloseHandle(job);
            None
        } else {
            Some(job)
        }
    }
}

fn drain_detached_pty_output(
    reader: &mut dyn Read,
    terminal_reply: &mut dyn FnMut(&'static [u8]),
    on_output: Option<&mut Box<dyn FnMut(Vec<u8>) + Send + 'static>>,
) {
    let mut filter = VtQueryFilter::default();
    drain_detached_output_inner(reader, on_output, Some((&mut filter, terminal_reply)));
}

fn drain_detached_output(
    reader: &mut dyn Read,
    on_output: Option<&mut Box<dyn FnMut(Vec<u8>) + Send + 'static>>,
) {
    drain_detached_output_inner(reader, on_output, None);
}

type VtDrain<'a> = (&'a mut VtQueryFilter, &'a mut dyn FnMut(&'static [u8]));

fn drain_detached_output_inner(
    reader: &mut dyn Read,
    mut on_output: Option<&mut Box<dyn FnMut(Vec<u8>) + Send + 'static>>,
    mut vt: Option<VtDrain<'_>>,
) {
    let mut buffer = [0_u8; 8192];
    // Per-drain UTF-8 reassembly buffer. Each call to this function
    // owns its own buffer, so concurrent stdout+stderr drains in
    // `spawn_piped_with_observer` (which share an `on_output` via
    // Arc<Mutex>) can't corrupt each other's codepoint state. Each
    // chunk we hand to the callback is guaranteed valid UTF-8.
    let mut utf8_buf: Vec<u8> = Vec::with_capacity(8192);
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                if let Some((filter, _)) = vt.as_mut() {
                    filter.finish(&mut utf8_buf);
                }
                // EOF: flush any trailing bytes (lossy if the stream
                // ended mid-codepoint — better to surface garbled
                // glyphs than drop the final message entirely).
                if !utf8_buf.is_empty() {
                    if let Some(callback) = on_output.as_deref_mut() {
                        let text = String::from_utf8_lossy(&utf8_buf).into_owned();
                        callback(text.into_bytes());
                    }
                }
                break;
            }
            Ok(bytes_read) => {
                if let Some((filter, terminal_reply)) = vt.as_mut() {
                    filter.push(&buffer[..bytes_read], *terminal_reply, &mut utf8_buf);
                } else {
                    utf8_buf.extend_from_slice(&buffer[..bytes_read]);
                }
                // Emit the longest valid-UTF-8 prefix; keep the trailing
                // partial codepoint in the buffer for the next read.
                let valid_up_to = match std::str::from_utf8(&utf8_buf) {
                    Ok(_) => utf8_buf.len(),
                    Err(error) => error.valid_up_to(),
                };
                if valid_up_to > 0 {
                    let prefix: Vec<u8> = utf8_buf.drain(..valid_up_to).collect();
                    if let Some(callback) = on_output.as_deref_mut() {
                        callback(prefix);
                    }
                }
                // Pathological tail: if the remaining bytes can't be a
                // partial codepoint (>4 bytes — max UTF-8 codepoint
                // length), the stream is genuinely malformed. Drop one
                // byte at a time via lossy decode so we make progress
                // instead of buffering forever.
                while utf8_buf.len() > 4
                    && std::str::from_utf8(&utf8_buf)
                        .err()
                        .map(|e| e.valid_up_to())
                        == Some(0)
                {
                    let dropped: Vec<u8> = utf8_buf.drain(..1).collect();
                    if let Some(callback) = on_output.as_deref_mut() {
                        let lossy = String::from_utf8_lossy(&dropped).into_owned();
                        callback(lossy.into_bytes());
                    }
                }
            }
            Err(_) => break,
        }
    }
}

#[derive(Default)]
struct VtQueryFilter {
    pending: Vec<u8>,
}

impl VtQueryFilter {
    fn push(
        &mut self,
        chunk: &[u8],
        terminal_reply: &mut dyn FnMut(&'static [u8]),
        output: &mut Vec<u8>,
    ) {
        self.pending.extend_from_slice(chunk);
        let mut offset = 0;
        while offset < self.pending.len() {
            let Some(relative_escape) =
                self.pending[offset..].iter().position(|byte| *byte == 0x1b)
            else {
                output.extend_from_slice(&self.pending[offset..]);
                offset = self.pending.len();
                break;
            };
            let escape = offset + relative_escape;
            output.extend_from_slice(&self.pending[offset..escape]);
            let remaining = &self.pending[escape..];
            if let Some((query_len, reply)) = vt_query_reply(remaining) {
                terminal_reply(reply);
                offset = escape + query_len;
            } else if VT_QUERIES
                .iter()
                .any(|(query, _)| query.starts_with(remaining))
            {
                offset = escape;
                break;
            } else {
                output.push(0x1b);
                offset = escape + 1;
            }
        }
        self.pending.drain(..offset);
    }

    fn finish(&mut self, output: &mut Vec<u8>) {
        output.append(&mut self.pending);
    }
}

const VT_QUERIES: [(&[u8], &[u8]); 4] = [
    (b"\x1b[6n", b"\x1b[1;1R"),
    (b"\x1b[c", b"\x1b[?62;c"),
    (b"\x1b[0c", b"\x1b[?62;c"),
    (b"\x1b[5n", b"\x1b[0n"),
];

fn vt_query_reply(bytes: &[u8]) -> Option<(usize, &'static [u8])> {
    VT_QUERIES
        .iter()
        .find(|(query, _)| bytes.starts_with(query))
        .map(|(query, reply)| (query.len(), *reply))
}

#[derive(Default)]
struct MeaningfulOutputDetector {
    state: EscapeState,
}

#[derive(Default, Clone, Copy)]
enum EscapeState {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    String,
    StringEscape,
}

impl MeaningfulOutputDetector {
    fn push(&mut self, bytes: &[u8]) -> bool {
        let mut meaningful = false;
        for byte in bytes {
            self.state = match self.state {
                EscapeState::Ground if *byte == 0x1b => EscapeState::Escape,
                EscapeState::Ground => {
                    // Whitespace and C0 controls do not prove that a harness
                    // reached a usable prompt. Printable ASCII or UTF-8 does.
                    meaningful |= *byte >= 0x80 || (*byte > 0x20 && *byte != 0x7f);
                    EscapeState::Ground
                }
                EscapeState::Escape if *byte == b'[' => EscapeState::Csi,
                EscapeState::Escape if matches!(*byte, b']' | b'P' | b'^' | b'_') => {
                    EscapeState::String
                }
                EscapeState::Escape if (0x20..=0x2f).contains(byte) => {
                    EscapeState::EscapeIntermediate
                }
                EscapeState::Escape if *byte == 0x1b => EscapeState::Escape,
                EscapeState::Escape => EscapeState::Ground,
                EscapeState::EscapeIntermediate if (0x20..=0x2f).contains(byte) => {
                    EscapeState::EscapeIntermediate
                }
                EscapeState::EscapeIntermediate if *byte == 0x1b => EscapeState::Escape,
                EscapeState::EscapeIntermediate => EscapeState::Ground,
                EscapeState::Csi if *byte == 0x1b => EscapeState::Escape,
                EscapeState::Csi if (0x40..=0x7e).contains(byte) => EscapeState::Ground,
                EscapeState::Csi => EscapeState::Csi,
                EscapeState::String if *byte == 0x07 => EscapeState::Ground,
                EscapeState::String if *byte == 0x1b => EscapeState::StringEscape,
                EscapeState::String => EscapeState::String,
                EscapeState::StringEscape if *byte == b'\\' => EscapeState::Ground,
                EscapeState::StringEscape => EscapeState::String,
            };
        }
        meaningful
    }
}

fn wait_for_child(child: &mut Box<dyn portable_pty::Child + Send + Sync>) -> PtyRunResult {
    match child.wait() {
        Ok(exit_status) => {
            let exit_code = i32::try_from(exit_status.exit_code()).unwrap_or(i32::MAX);
            let status = if exit_status.success() {
                "completed"
            } else {
                "failed"
            };
            PtyRunResult {
                status,
                exit_code: Some(exit_code),
            }
        }
        Err(_) => PtyRunResult {
            status: "failed",
            exit_code: None,
        },
    }
}

trait PtyResizeTarget: Send {
    fn resize_pty(&self, size: PtySize) -> Result<()>;
}

impl PtyResizeTarget for Box<dyn MasterPty + Send> {
    fn resize_pty(&self, size: PtySize) -> Result<()> {
        self.resize(size)
    }
}

fn apply_pty_resize(
    target: &dyn PtyResizeTarget,
    current: &mut PtySize,
    next: Option<PtySize>,
) -> bool {
    let Some(next) = next.and_then(valid_pty_size) else {
        return true;
    };
    if next == *current {
        return true;
    }
    if target.resize_pty(next).is_err() {
        return false;
    }
    *current = next;
    true
}

const PTY_RESIZE_POLL_INTERVAL: Duration = Duration::from_millis(100);

struct PtyResizeWatcher {
    stop: Option<mpsc::Sender<()>>,
    join: Option<thread::JoinHandle<Box<dyn PtyResizeTarget>>>,
}

impl PtyResizeWatcher {
    fn spawn(master: Box<dyn MasterPty + Send>, initial: PtySize) -> Self {
        Self::spawn_with_source(
            master,
            initial,
            detected_terminal_size,
            PTY_RESIZE_POLL_INTERVAL,
        )
    }

    fn spawn_with_source<T, S>(
        target: T,
        initial: PtySize,
        mut size_source: S,
        interval: Duration,
    ) -> Self
    where
        T: PtyResizeTarget + 'static,
        S: FnMut() -> Option<PtySize> + Send + 'static,
    {
        let (stop, stopped) = mpsc::channel();
        let target: Box<dyn PtyResizeTarget> = Box::new(target);
        let join = thread::spawn(move || {
            let mut current = initial;
            loop {
                match stopped.recv_timeout(interval) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                if !apply_pty_resize(target.as_ref(), &mut current, size_source()) {
                    break;
                }
            }
            // Retain the PTY master in the completed join result until `stop`
            // joins and drops it after child teardown. Dropping it in this worker
            // can close an otherwise-active Windows ConPTY on resize failure.
            target
        });
        Self {
            stop: Some(stop),
            join: Some(join),
        }
    }

    fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for PtyResizeWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_attached_with_pty_system(
    command: &HarnessCommand,
    pty_system: &(dyn PtySystem + Send),
    observer: Option<AttachedOutputObserver>,
) -> Result<PtyRunResult> {
    let initial_size = attached_terminal_size();
    let pair = pty_system
        .openpty(initial_size)
        .context("failed to open PTY")?;
    let mut child = pair
        .slave
        .spawn_command(command.to_command_builder())
        .with_context(|| format!("failed to spawn harness `{}`", command.program()))?;

    drop(pair.slave);

    let master = pair.master;
    let mut reader = master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;
    let mut writer = master.take_writer().context("failed to open PTY writer")?;
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let mut master = Some(master);
    let mut resize_watcher = interactive.then(|| {
        PtyResizeWatcher::spawn(
            master.take().expect("interactive PTY master is available"),
            initial_size,
        )
    });
    let _held_master = master;
    let _raw_mode =
        RawModeGuard::enable_if_terminal().context("failed to enable raw terminal mode")?;

    let output_thread = thread::spawn(move || {
        let mut stdout = io::stdout().lock();
        copy_attached_output(&mut reader, &mut stdout, observer)
    });

    // Only forward stdin to the PTY when it is an interactive terminal. A
    // one-shot `coven run` gets its prompt from argv, so a piped or
    // redirected stdin carries nothing the harness needs — and copying it
    // into the PTY makes the line discipline echo the EOF as a visible `^D`
    // in the captured output. Interactive sessions still need the forward so
    // the user can type.
    if io::stdin().is_terminal() {
        thread::spawn(move || {
            let mut stdin = io::stdin().lock();
            let _ = io::copy(&mut stdin, &mut writer);
        });
    }

    let exit_status = child.wait().context("failed to wait for harness process")?;
    if let Some(watcher) = resize_watcher.as_mut() {
        watcher.stop();
    }
    output_thread
        .join()
        .map_err(|_| anyhow::anyhow!("attached output thread panicked"))??;
    let exit_code = i32::try_from(exit_status.exit_code()).unwrap_or(i32::MAX);
    let status = if exit_status.success() {
        "completed"
    } else {
        "failed"
    };

    Ok(PtyRunResult {
        status,
        exit_code: Some(exit_code),
    })
}

fn copy_attached_output(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    mut observer: Option<AttachedOutputObserver>,
) -> Result<()> {
    let mut buffer = [0_u8; 8192];
    let mut observed = Vec::with_capacity(buffer.len());
    let mut observer_error = None;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes_read) => {
                let chunk = &buffer[..bytes_read];
                writer.write_all(chunk)?;
                if let Some(callback) = observer.as_deref_mut() {
                    observed.extend_from_slice(chunk);
                    if let Err(error) = emit_observed_utf8(&mut observed, callback, false) {
                        observer_error = Some(error);
                        observer = None;
                        observed.clear();
                    }
                }
            }
            Err(_) => break,
        }
    }
    if let Some(callback) = observer.as_deref_mut() {
        if let Err(error) = emit_observed_utf8(&mut observed, callback, true) {
            observer_error = Some(error);
        }
    }
    writer
        .flush()
        .context("failed to flush attached harness output")?;
    match observer_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn emit_observed_utf8(
    pending: &mut Vec<u8>,
    observer: &mut (dyn FnMut(Vec<u8>) -> Result<()> + Send + 'static),
    finish: bool,
) -> Result<()> {
    let valid_up_to = match std::str::from_utf8(pending) {
        Ok(_) => pending.len(),
        Err(error) => error.valid_up_to(),
    };
    if valid_up_to > 0 {
        observer(pending.drain(..valid_up_to).collect())?;
    }
    while pending.len() > 4
        && std::str::from_utf8(pending)
            .err()
            .is_some_and(|error| error.valid_up_to() == 0)
    {
        let byte = pending.drain(..1).collect::<Vec<_>>();
        observer(String::from_utf8_lossy(&byte).into_owned().into_bytes())?;
    }
    if finish && !pending.is_empty() {
        observer(String::from_utf8_lossy(pending).into_owned().into_bytes())?;
        pending.clear();
    }
    Ok(())
}

struct RawModeGuard {
    enabled: bool,
}

impl RawModeGuard {
    fn enable_if_terminal() -> Result<Self> {
        let enabled = io::stdin().is_terminal() && io::stdout().is_terminal();
        if enabled {
            enable_raw_mode()?;
        }
        Ok(Self { enabled })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = disable_raw_mode();
        }
    }
}

const DEFAULT_PTY_ROWS: u16 = 24;
const DEFAULT_PTY_COLS: u16 = 80;

fn detected_terminal_size() -> Option<PtySize> {
    if !io::stdout().is_terminal() {
        return None;
    }
    let window = window_size().ok().map(|window| PtySize {
        rows: window.rows,
        cols: window.columns,
        pixel_width: window.width,
        pixel_height: window.height,
    });
    detected_terminal_size_from_sources(window, terminal_cell_size().ok())
}

fn detected_terminal_size_from_sources(
    window: Option<PtySize>,
    cells: Option<(u16, u16)>,
) -> Option<PtySize> {
    window.and_then(valid_pty_size).or_else(|| {
        cells.and_then(|(cols, rows)| {
            valid_pty_size(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
        })
    })
}

fn valid_pty_size(size: PtySize) -> Option<PtySize> {
    (size.rows > 0 && size.cols > 0).then_some(size)
}

fn terminal_size_from_sources(
    terminal: Option<PtySize>,
    env_rows: Option<u16>,
    env_cols: Option<u16>,
) -> PtySize {
    terminal.and_then(valid_pty_size).unwrap_or(PtySize {
        rows: env_rows.unwrap_or(DEFAULT_PTY_ROWS),
        cols: env_cols.unwrap_or(DEFAULT_PTY_COLS),
        pixel_width: 0,
        pixel_height: 0,
    })
}

fn terminal_size() -> PtySize {
    terminal_size_from_sources(None, env_u16("LINES"), env_u16("COLUMNS"))
}

fn attached_terminal_size() -> PtySize {
    terminal_size_from_sources(
        detected_terminal_size(),
        env_u16("LINES"),
        env_u16("COLUMNS"),
    )
}

fn env_u16(name: &str) -> Option<u16> {
    std::env::var(name)
        .ok()?
        .parse()
        .ok()
        .filter(|value| *value > 0)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    fn pty_size(rows: u16, cols: u16, pixel_width: u16, pixel_height: u16) -> PtySize {
        PtySize {
            rows,
            cols,
            pixel_width,
            pixel_height,
        }
    }

    #[test]
    fn windows_noninteractive_flags_preserve_other_creation_flags() {
        const CREATE_SUSPENDED_VALUE: u32 = 0x0000_0004;
        assert_eq!(
            windows_noninteractive_creation_flags(0),
            WINDOWS_CREATE_NO_WINDOW
        );
        assert_eq!(
            windows_noninteractive_creation_flags(CREATE_SUSPENDED_VALUE),
            WINDOWS_CREATE_NO_WINDOW | CREATE_SUSPENDED_VALUE
        );
    }

    #[test]
    fn official_windows_codex_npm_shim_resolves_validated_native_package() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let shim = temp_dir.path().join("codex.cmd");
        let package_root = temp_dir
            .path()
            .join("node_modules")
            .join("@openai")
            .join("codex");
        let entry = package_root.join("bin").join("codex.js");
        let native_root = package_root
            .join("node_modules")
            .join("@openai")
            .join("codex-win32-x64");
        let native = native_root
            .join("vendor")
            .join("x86_64-pc-windows-msvc")
            .join("bin")
            .join("codex.exe");
        std::fs::create_dir_all(entry.parent().unwrap())?;
        std::fs::create_dir_all(native.parent().unwrap())?;
        std::fs::write(
            temp_dir.path().join("node_modules").join(".modules.yaml"),
            b"fixture\n",
        )?;
        std::fs::write(&entry, "// official entry fixture\n")?;
        std::fs::write(&native, b"native fixture")?;
        std::fs::write(
            package_root.join("package.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "@openai/codex",
                "bin": { "codex": "bin/codex.js" },
                "optionalDependencies": {
                    "@openai/codex-win32-x64": "npm:@openai/codex@0.0.0-win32-x64"
                }
            }))?,
        )?;
        std::fs::write(
            native_root.join("package.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "@openai/codex",
                "os": ["win32"],
                "cpu": ["x64"]
            }))?,
        )?;
        std::fs::write(
            &shim,
            "@ECHO off\r\nendLocal & \"%_prog%\" \"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n",
        )?;

        let launch = resolve_official_codex_npm_shim_for_target(
            &shim,
            "@openai/codex-win32-x64",
            "x86_64-pc-windows-msvc",
            "x64",
        )?;
        assert_eq!(launch.program, std::fs::canonicalize(native)?);
        assert!(launch.env_overrides.contains(&(
            "CODEX_MANAGED_PACKAGE_ROOT".to_string(),
            Some(
                std::fs::canonicalize(package_root)?
                    .to_string_lossy()
                    .into_owned()
            )
        )));
        assert_eq!(
            launch
                .env_overrides
                .iter()
                .filter(|(name, value)| name.starts_with("CODEX_MANAGED_BY_")
                    && value.as_deref() == Some("1"))
                .count(),
            1
        );
        assert!(launch
            .env_overrides
            .contains(&("CODEX_MANAGED_BY_PNPM".to_string(), Some("1".to_string()))));
        Ok(())
    }

    #[test]
    fn unofficial_windows_codex_batch_shim_fails_closed() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let shim = temp_dir.path().join("codex.cmd");
        assert!(windows_program_is_batch_shim(&shim.to_string_lossy()));
        std::fs::write(&shim, "@echo off\r\necho not-codex %*\r\n")?;
        let error = resolve_official_codex_npm_shim_for_target(
            &shim,
            "@openai/codex-win32-x64",
            "x86_64-pc-windows-msvc",
            "x64",
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not contain a safe existing package entry"),
            "{error:#}"
        );
        Ok(())
    }

    #[test]
    fn codex_managed_package_markers_match_supported_package_managers() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let npm_root = temp_dir.path().join("npm/node_modules/@openai/codex");
        let bun_root = temp_dir
            .path()
            .join(".bun/install/global/node_modules/@openai/codex");
        std::fs::create_dir_all(&npm_root)?;
        std::fs::create_dir_all(&bun_root)?;
        assert_eq!(
            codex_package_manager_with_env(&npm_root, "npm/10", "npm-cli.js"),
            "NPM"
        );
        assert_eq!(
            codex_package_manager_with_env(&npm_root, "pnpm/10", "pnpm.cjs"),
            "PNPM"
        );
        assert_eq!(codex_package_manager_with_env(&bun_root, "", ""), "BUN");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unattended_launch_policy_piped_fixture_creates_primary_artifact() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path().join("project");
        let allowed_dir = project_root.join("artifacts");
        std::fs::create_dir_all(&allowed_dir)?;
        let allowed_dir_text = allowed_dir.to_string_lossy().into_owned();
        let fake_codex = temp_dir.path().join("fake-codex");
        std::fs::write(
            &fake_codex,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > received-args.txt\ncat > received-prompt.txt\nprintf 'fixture artifact\\n' > artifacts/primary.md\nprintf 'artifact written\\n'\n",
        )?;
        let mut permissions = std::fs::metadata(&fake_codex)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_codex, permissions)?;

        let policy =
            crate::harness::LaunchPolicy::unattended_workspace_write(
                vec![allowed_dir_text.clone()],
            );
        let mut command = build_piped_harness_command_with_conversation(
            "codex",
            "write artifacts/primary.md",
            &project_root,
            crate::harness::HarnessLaunchMode::NonInteractive,
            None,
            None,
            crate::harness::HarnessLaunchOptions {
                launch_policy: Some(&policy),
                ..Default::default()
            },
        )?;
        command.program = fake_codex.to_string_lossy().into_owned();
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };

        let session = spawn_piped_with_observer(&command, Some(observer), false)?;
        let process_tree = session.activate(|_input, process_tree| Ok(process_tree))?;
        let result = exit_rx.recv_timeout(Duration::from_secs(10))?;
        drop(process_tree);

        assert_eq!(result.status, "completed", "{result:?}");
        assert_eq!(
            std::fs::read_to_string(allowed_dir.join("primary.md"))?,
            "fixture artifact\n"
        );
        assert_eq!(
            std::fs::read_to_string(project_root.join("received-args.txt"))?,
            [
                "--ask-for-approval",
                "never",
                "-c",
                r#"approval_policy="never""#,
                "--sandbox",
                "workspace-write",
                "--add-dir",
                allowed_dir_text.as_str(),
                "exec",
                "--skip-git-repo-check",
                "--color",
                "never",
                "--",
                "-",
                "",
            ]
            .join("\n")
        );
        assert_eq!(
            std::fs::read_to_string(project_root.join("received-prompt.txt"))?,
            "write artifacts/primary.md"
        );
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn piped_prompt_delivery_failure_terminates_after_registration() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("closed-stdin.pid");
        let command = piped_prompt_probe_command(
            temp_dir.path(),
            "close-stdin",
            &pid_file.to_string_lossy(),
            None,
            vec![b'x'; 1024 * 1024],
        )?;
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };

        let session = spawn_piped_with_observer(&command, Some(observer), false)?;
        let child_pid = await_piped_descendant_pid(&pid_file)?;
        let registered = Arc::new(AtomicBool::new(false));
        let registered_in_callback = Arc::clone(&registered);
        let error = match session.activate(|_input, process_tree| {
            registered_in_callback.store(true, Ordering::Release);
            Ok(process_tree)
        }) {
            Ok(_) => anyhow::bail!("a closed stdin unexpectedly accepted the prompt"),
            Err(error) => error,
        };
        assert!(registered.load(Ordering::Acquire));
        assert!(
            format!("{error:#}").contains("failed writing harness prompt to stdin"),
            "unexpected error: {error:#}"
        );
        let result = exit_rx.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(result.status, "failed", "{result:?}");
        assert!(
            wait_for_piped_process_exit(child_pid, Duration::from_secs(2)),
            "failed prompt delivery left child {child_pid} running"
        );
        Ok(())
    }

    #[test]
    fn failed_piped_cleanup_has_typed_privacy_safe_disposition() {
        let primary = anyhow::anyhow!("private primary prompt failure");
        let cleanup = Err(anyhow::anyhow!("private cleanup failure"));

        let error = piped_launch_cleanup_error(primary, cleanup);

        assert!(
            error
                .downcast_ref::<PipedLaunchCleanupRetainedError>()
                .is_some(),
            "cleanup ambiguity lost its typed disposition: {error:#}"
        );
        assert_eq!(
            error.to_string(),
            "piped runtime ownership may remain after launch cleanup"
        );
        let diagnostic = format!("{error:#}");
        for private in ["private primary prompt failure", "private cleanup failure"] {
            assert!(
                !diagnostic.contains(private),
                "typed cleanup disposition leaked private failure data"
            );
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn definitive_piped_cleanup_is_ordinary_while_exit_callback_is_delayed() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("delayed-callback.pid");
        let command = piped_prompt_probe_command(
            temp_dir.path(),
            "close-stdin",
            &pid_file.to_string_lossy(),
            None,
            vec![b'x'; 1024 * 1024],
        )?;
        let (callback_tx, callback_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(move |result| {
                let _ = callback_tx.send(result);
                let _ = release_rx.recv();
            }),
        };

        let session = spawn_piped_with_observer(&command, Some(observer), false)?;
        let child_pid = await_piped_descendant_pid(&pid_file)?;
        let error = match session.activate(|_input, process_tree| Ok(process_tree)) {
            Ok(_) => anyhow::bail!("closed stdin unexpectedly accepted the launch prompt"),
            Err(error) => error,
        };
        let result = callback_rx.recv_timeout(Duration::from_secs(2))?;

        assert_eq!(result.status, "failed", "{result:?}");
        assert!(
            error
                .downcast_ref::<PipedLaunchCleanupRetainedError>()
                .is_none(),
            "definitive child cleanup became ambiguous while on_exit was delayed"
        );
        assert!(
            format!("{error:#}").contains("failed writing harness prompt to stdin"),
            "ordinary primary error was not preserved: {error:#}"
        );
        assert!(
            wait_for_piped_process_exit(child_pid, Duration::from_secs(2)),
            "definitive cleanup left child {child_pid} running"
        );
        release_tx.send(())?;
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn piped_prompt_failure_overrides_successful_root_exit() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("exit-zero-closed-stdin.pid");
        let command = piped_prompt_probe_command(
            temp_dir.path(),
            "exit-zero-close-stdin",
            &pid_file.to_string_lossy(),
            None,
            vec![b'x'; 1024 * 1024],
        )?;
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };

        let session = spawn_piped_with_observer(&command, Some(observer), false)?;
        let child_pid = await_piped_descendant_pid(&pid_file)?;
        assert!(
            wait_for_piped_process_exit(child_pid, Duration::from_secs(10)),
            "successful root did not exit before prompt delivery"
        );
        let error = match session.activate(|_input, process_tree| Ok(process_tree)) {
            Ok(_) => anyhow::bail!("a closed stdin unexpectedly accepted the large prompt"),
            Err(error) => error,
        };
        let diagnostic = format!("{error:#}");
        assert!(
            diagnostic.contains("failed writing harness prompt to stdin")
                || diagnostic.contains("terminated before its stdin prompt could be delivered"),
            "{diagnostic}"
        );
        let result = exit_rx.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(result.status, "failed", "{result:?}");
        assert_eq!(
            result.exit_code,
            Some(0),
            "root exit remains diagnostic but cannot override prompt failure"
        );
        assert!(wait_for_piped_process_exit(
            child_pid,
            Duration::from_secs(2)
        ));
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn piped_prompt_and_pre_read_output_exceeding_pipe_capacity_complete_exactly(
    ) -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let receipt = temp_dir.path().join("received-prompt.bin");
        let prompt = (0..1024 * 1024)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let output_bytes = 1024 * 1024;
        let command = piped_prompt_probe_command(
            temp_dir.path(),
            "duplex",
            &output_bytes.to_string(),
            Some(&receipt),
            prompt.clone(),
        )?;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_output = Arc::clone(&captured);
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(move |chunk| {
                captured_for_output.lock().unwrap().extend(chunk);
            }),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };

        let session = spawn_piped_with_observer(&command, Some(observer), false)?;
        let process_tree = session
            .activate_with_prompt_timeout(Duration::from_secs(8), |_input, process_tree| {
                Ok(process_tree)
            })?;
        let result = exit_rx.recv_timeout(Duration::from_secs(10))?;
        drop(process_tree);

        assert_eq!(result.status, "completed", "{result:?}");
        assert_eq!(result.exit_code, Some(0), "{result:?}");
        let output = captured.lock().unwrap();
        assert_eq!(output.len(), output_bytes);
        assert!(output.iter().all(|byte| *byte == b'o'));
        assert_eq!(std::fs::read(receipt)?, prompt);
        Ok(())
    }

    #[test]
    fn observed_piped_drains_output_before_delivering_large_prompt() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let receipt = temp_dir.path().join("observed-prompt.bin");
        let prompt = vec![b'p'; 128 * 1024];
        let output_bytes = 128 * 1024;
        let command = piped_prompt_probe_command(
            temp_dir.path(),
            "duplex-contained",
            &output_bytes.to_string(),
            Some(&receipt),
            prompt.clone(),
        )?;
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_output = Arc::clone(&observed);

        let result = run_piped_attached_observed_with_writers(
            &command,
            Box::new(move |chunk| {
                observed_output.lock().unwrap().extend(chunk);
                Ok(())
            }),
            Box::new(io::sink()),
            Box::new(io::sink()),
        )?;

        assert_eq!(result.status, "completed", "{result:?}");
        assert_eq!(result.exit_code, Some(0), "{result:?}");
        assert_eq!(std::fs::read(receipt)?, prompt);
        let output = observed.lock().unwrap();
        assert_eq!(output.len(), output_bytes);
        assert!(output.iter().all(|byte| *byte == b'o'));
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn piped_root_exit_reaps_closed_pipe_descendant_before_completion() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("closed-pipe-descendant.pid");
        let command = piped_prompt_probe_command(
            temp_dir.path(),
            "root-exit-closed-descendant",
            &pid_file.to_string_lossy(),
            None,
            Vec::new(),
        )?;
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };

        let session = spawn_piped_with_observer(&command, Some(observer), false)?;
        let descendant_pid = await_piped_descendant_pid(&pid_file)?;
        let process_tree = session.activate(|_input, process_tree| Ok(process_tree))?;
        let result = exit_rx.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(result.status, "completed", "{result:?}");
        assert_eq!(result.exit_code, Some(0), "{result:?}");
        assert!(
            wait_for_piped_process_exit(descendant_pid, Duration::from_secs(2)),
            "successful root exit left closed-pipe descendant {descendant_pid} running"
        );
        drop(process_tree);
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn piped_root_exit_terminates_inherited_output_descendant_and_preserves_exit(
    ) -> anyhow::Result<()> {
        use std::sync::atomic::AtomicUsize;

        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("inherited-output-descendant.pid");
        let command = piped_prompt_probe_command(
            temp_dir.path(),
            "root-exit-output-descendant",
            &pid_file.to_string_lossy(),
            None,
            Vec::new(),
        )?;
        let observed = Arc::new(AtomicUsize::new(0));
        let observed_output = Arc::clone(&observed);
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(move |chunk| {
                observed_output.fetch_add(chunk.len(), Ordering::AcqRel);
            }),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };

        let session = spawn_piped_with_observer(&command, Some(observer), false)?;
        let descendant_pid = await_piped_descendant_pid(&pid_file)?;
        let process_tree = session.activate(|_input, process_tree| Ok(process_tree))?;
        let result = exit_rx.recv_timeout(Duration::from_secs(2))?;
        let count_at_exit = observed.load(Ordering::Acquire);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(observed.load(Ordering::Acquire), count_at_exit);
        drop(process_tree);

        assert_eq!(result.status, "completed", "{result:?}");
        assert_eq!(result.exit_code, Some(0), "{result:?}");
        assert!(
            count_at_exit > 0,
            "descendant never exercised inherited output"
        );
        assert!(
            wait_for_piped_process_exit(descendant_pid, Duration::from_secs(2)),
            "natural root exit left inherited-output descendant {descendant_pid} running"
        );
        Ok(())
    }

    #[test]
    fn harness_command_can_set_a_private_runtime_environment_value() {
        let mut command = HarnessCommand::fixture("echo", Vec::new(), PathBuf::from("/tmp"));

        command.set_environment_override("COVEN_TEST_CAPABILITY", Some("opaque-value"));

        assert!(command.env_overrides.contains(&(
            "COVEN_TEST_CAPABILITY".to_owned(),
            Some("opaque-value".to_owned())
        )));
        assert_eq!(
            command.environment_override_for_test("COVEN_TEST_CAPABILITY"),
            Some("opaque-value")
        );
    }

    #[test]
    fn observed_piped_root_exit_terminates_inherited_output_descendant() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir
            .path()
            .join("observed-inherited-output-descendant.pid");
        let ready_marker = temp_dir
            .path()
            .join("observed-inherited-output-descendant.ready");
        let mut command = piped_prompt_probe_command(
            temp_dir.path(),
            "root-exit-short-output-descendant",
            &pid_file.to_string_lossy(),
            Some(&ready_marker),
            Vec::new(),
        )?;
        command.env_overrides.push((
            "COVEN_TEST_PIPED_STARTUP_DELAY_MS".to_owned(),
            Some("1500".to_owned()),
        ));
        let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_output = Arc::clone(&observed);
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        let runner = thread::spawn(move || {
            let result = run_piped_attached_observed_with_writers(
                &command,
                Box::new(move |chunk| {
                    observed_output.fetch_add(chunk.len(), Ordering::AcqRel);
                    Ok(())
                }),
                Box::new(io::sink()),
                Box::new(io::sink()),
            );
            let _ = result_tx.send(result);
        });
        let descendant_pid = match await_piped_descendant_pid(&pid_file) {
            Ok(pid) => pid,
            Err(error) => {
                let early = result_rx.recv_timeout(Duration::from_secs(5));
                runner
                    .join()
                    .map_err(|_| anyhow::anyhow!("observed runner panicked"))?;
                return Err(error.context(format!(
                    "observed runner failed before fixture readiness; result: {early:?}"
                )));
            }
        };
        // The one-second budget below measures post-exit cleanup only. Start it
        // after the descendant has proven both inherited pipes carry output, so
        // fixture startup under load cannot consume the budget.
        let readiness_deadline = Instant::now() + Duration::from_secs(30);
        while !ready_marker.exists() {
            match result_rx.try_recv() {
                Ok(early) => {
                    runner
                        .join()
                        .map_err(|_| anyhow::anyhow!("observed runner panicked"))?;
                    anyhow::bail!(
                        "observed runner finished before descendant readiness; result: {early:?}"
                    );
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    runner
                        .join()
                        .map_err(|_| anyhow::anyhow!("observed runner panicked"))?;
                    anyhow::bail!("observed runner disconnected before descendant readiness");
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() >= readiness_deadline {
                let late = result_rx.recv_timeout(Duration::from_secs(5));
                runner
                    .join()
                    .map_err(|_| anyhow::anyhow!("observed runner panicked"))?;
                anyhow::bail!(
                    "inherited-output descendant never signalled readiness; result: {late:?}"
                );
            }
            thread::sleep(Duration::from_millis(5));
        }
        let result = match result_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let late = result_rx.recv_timeout(Duration::from_secs(5));
                runner
                    .join()
                    .map_err(|_| anyhow::anyhow!("observed runner panicked"))?;
                anyhow::bail!(
                    "observed runner did not finish post-exit cleanup within one second; late result: {late:?}"
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                runner
                    .join()
                    .map_err(|_| anyhow::anyhow!("observed runner panicked"))?;
                anyhow::bail!("observed runner disconnected without a result");
            }
        };
        runner
            .join()
            .map_err(|_| anyhow::anyhow!("observed runner panicked"))?;
        let result = result?;

        assert_eq!(result.status, "completed", "{result:?}");
        assert_eq!(result.exit_code, Some(0), "{result:?}");
        assert!(observed.load(Ordering::Acquire) > 0);
        assert!(
            wait_for_piped_process_exit(descendant_pid, Duration::from_secs(2)),
            "observed runner left inherited-output descendant {descendant_pid} running"
        );
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn piped_prompt_timeout_terminates_and_reaps_a_child_that_never_reads() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("never-read.pid");
        let command = piped_prompt_probe_command(
            temp_dir.path(),
            "never-read",
            &pid_file.to_string_lossy(),
            None,
            vec![b'x'; 1024 * 1024],
        )?;
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };

        let session = spawn_piped_with_observer(&command, Some(observer), false)?;
        let child_pid = await_piped_descendant_pid(&pid_file)?;
        let started = Instant::now();
        let error = match session
            .activate_with_prompt_timeout(Duration::from_millis(100), |_input, process_tree| {
                Ok(process_tree)
            }) {
            Ok(_) => anyhow::bail!("a never-reading child accepted the complete prompt"),
            Err(error) => error,
        };

        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "blocked prompt timeout took {elapsed:?}"
        );
        assert!(format!("{error:#}").contains("timed out"), "{error:#}");
        let result = exit_rx.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(result.status, "failed", "{result:?}");
        assert!(
            wait_for_piped_process_exit(child_pid, Duration::from_secs(2)),
            "prompt timeout left child {child_pid} running"
        );
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_piped_child_runs_without_a_console_window() -> anyhow::Result<()> {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_NEW_CONSOLE;

        let temp_dir = tempfile::tempdir()?;
        let command = windows_console_probe_command(temp_dir.path())?;
        let mut control = std::process::Command::new(command.program());
        control
            .current_dir(command.cwd())
            .creation_flags(CREATE_NEW_CONSOLE);
        let control_output = control.output()?;
        assert!(
            control_output.status.success(),
            "{:?}",
            control_output.status
        );
        assert_eq!(
            String::from_utf8(control_output.stdout)?.trim(),
            "console=present",
            "the console-subsystem fixture must positively observe an explicitly allocated console before it can prove production suppression"
        );

        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_output = Arc::clone(&captured);
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(move |chunk| {
                captured_for_output.lock().unwrap().extend(chunk);
            }),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };

        let session = spawn_piped_with_observer(&command, Some(observer), false)?;
        let process_tree = session.activate(|_input, process_tree| Ok(process_tree))?;
        let result = exit_rx.recv_timeout(Duration::from_secs(10))?;
        drop(process_tree);
        let output = String::from_utf8(captured.lock().unwrap().clone())?;

        assert_eq!(result.status, "completed", "{result:?}; {output:?}");
        assert_eq!(result.exit_code, Some(0), "{result:?}; {output:?}");
        assert_eq!(output.trim(), "console=absent");
        Ok(())
    }

    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    fn windows_official_codex_shim_launches_native_with_exact_argv_and_no_console(
    ) -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let compiled_probe = windows_console_probe_command(temp_dir.path())?;
        let shim = temp_dir.path().join("codex.cmd");
        let package_root = temp_dir
            .path()
            .join("node_modules")
            .join("@openai")
            .join("codex");
        let entry = package_root.join("bin").join("codex.js");
        let native_root = package_root
            .join("node_modules")
            .join("@openai")
            .join("codex-win32-x64");
        let native = native_root
            .join("vendor")
            .join("x86_64-pc-windows-msvc")
            .join("bin")
            .join("codex.exe");
        std::fs::create_dir_all(entry.parent().unwrap())?;
        std::fs::create_dir_all(native.parent().unwrap())?;
        std::fs::write(&entry, "// official entry fixture\n")?;
        std::fs::copy(compiled_probe.program(), &native)?;
        std::fs::write(
            package_root.join("package.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "@openai/codex",
                "bin": { "codex": "bin/codex.js" },
                "optionalDependencies": {
                    "@openai/codex-win32-x64": "npm:@openai/codex@0.0.0-win32-x64"
                }
            }))?,
        )?;
        std::fs::write(
            native_root.join("package.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "@openai/codex",
                "os": ["win32"],
                "cpu": ["x64"]
            }))?,
        )?;
        std::fs::write(
            &shim,
            "@ECHO off\r\nendLocal & \"%_prog%\" \"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n",
        )?;
        let shim_text = shim.to_string_lossy().into_owned();
        let (interactive_program, interactive_env_overrides) = prepare_harness_program(
            "codex",
            crate::harness::HarnessLaunchMode::Interactive,
            shim_text.clone(),
        )?;
        assert_eq!(interactive_program, shim_text);
        assert!(
            interactive_env_overrides.is_empty(),
            "interactive PTY launches must retain the official wrapper and its existing environment"
        );

        let (program, env_overrides) = prepare_harness_program(
            "codex",
            crate::harness::HarnessLaunchMode::NonInteractive,
            shim_text,
        )?;
        assert_eq!(PathBuf::from(&program), std::fs::canonicalize(&native)?);
        assert!(!windows_program_is_batch_shim(&program));

        let argv_file = temp_dir.path().join("argv.bin");
        let env_file = temp_dir.path().join("managed-env.txt");
        let external_dir = r#"C:\Research & Evidence\資料 ^ 100%!"#.to_string();
        let exact_args = vec![
            "--ask-for-approval".to_string(),
            "never".to_string(),
            "-c".to_string(),
            r#"approval_policy="never""#.to_string(),
            "--sandbox".to_string(),
            "workspace-write".to_string(),
            "-c".to_string(),
            r#"windows.sandbox="unelevated""#.to_string(),
            "--add-dir".to_string(),
            external_dir.clone(),
            "exec".to_string(),
            "--skip-git-repo-check".to_string(),
            "--color".to_string(),
            "never".to_string(),
            "--".to_string(),
            "-".to_string(),
        ];
        let mut fixture_args = vec![
            "--record-argv-env".to_string(),
            argv_file.to_string_lossy().into_owned(),
            env_file.to_string_lossy().into_owned(),
        ];
        fixture_args.extend(exact_args.clone());
        let command = HarnessCommand {
            program,
            args: fixture_args,
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: None,
            env_overrides,
        };
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_output = Arc::clone(&captured);
        let (exit_tx, exit_rx) = mpsc::channel();
        let session = spawn_piped_with_observer(
            &command,
            Some(DetachedPtyObserver {
                on_output: Box::new(move |chunk| {
                    captured_for_output.lock().unwrap().extend(chunk);
                }),
                on_exit: Box::new(move |result| {
                    let _ = exit_tx.send(result);
                }),
            }),
            false,
        )?;
        let process_tree = session.activate(|_input, process_tree| Ok(process_tree))?;
        let result = exit_rx.recv_timeout(Duration::from_secs(10))?;
        drop(process_tree);

        assert_eq!(result.status, "completed", "{result:?}");
        assert_eq!(result.exit_code, Some(0), "{result:?}");
        assert_eq!(
            String::from_utf8(captured.lock().unwrap().clone())?.trim(),
            "console=absent"
        );
        assert_eq!(
            std::fs::read_to_string(argv_file)?,
            exact_args.join("\0"),
            "native Codex argv changed after npm-shim resolution"
        );
        let managed_env = std::fs::read_to_string(env_file)?;
        assert!(
            managed_env.contains(&format!(
                "package_root={}",
                std::fs::canonicalize(package_root)?.display()
            )),
            "{managed_env}"
        );
        assert!(
            managed_env.contains("managers=CODEX_MANAGED_BY_"),
            "{managed_env}"
        );
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_strict_child_combines_no_window_with_suspended_containment() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let probe = windows_console_probe_command(temp_dir.path())?;
        let mut command = std::process::Command::new(probe.program());
        command.current_dir(probe.cwd());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let (child, process_tree) = spawn_strict_child_process_tree(&mut command)?;
        let output = child.wait_with_output()?;
        drop(process_tree);

        assert!(output.status.success(), "{:?}", output.status);
        assert_eq!(String::from_utf8(output.stdout)?.trim(), "console=absent");
        Ok(())
    }

    #[cfg(unix)]
    fn piped_descendant_fixture(build_dir: &Path, pid_file: &Path) -> HarnessCommand {
        HarnessCommand::fixture(
            "/bin/sh",
            vec![
                "-c".to_string(),
                "sleep 120 </dev/null >/dev/null 2>&1 & echo $! > \"$1\"; wait".to_string(),
                "piped-descendant".to_string(),
                pid_file.to_string_lossy().into_owned(),
            ],
            build_dir.to_path_buf(),
        )
    }

    #[cfg(windows)]
    fn piped_descendant_fixture(build_dir: &Path, pid_file: &Path) -> HarnessCommand {
        let mut command = windows_console_probe_command(build_dir).expect("compile Windows probe");
        command.args = vec![
            "--spawn-descendant".to_string(),
            pid_file.to_string_lossy().into_owned(),
        ];
        command
    }

    #[cfg(unix)]
    fn piped_nonzero_fixture(build_dir: &Path) -> HarnessCommand {
        HarnessCommand::fixture(
            "/bin/sh",
            vec!["-c".to_string(), "exit 23".to_string()],
            build_dir.to_path_buf(),
        )
    }

    #[cfg(windows)]
    fn piped_nonzero_fixture(build_dir: &Path) -> HarnessCommand {
        let mut command = windows_console_probe_command(build_dir).expect("compile Windows probe");
        command.args = vec!["--exit-code".to_string(), "23".to_string()];
        command
    }

    #[cfg(any(unix, windows))]
    fn await_piped_descendant_pid(pid_file: &Path) -> anyhow::Result<u32> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(raw) = std::fs::read_to_string(pid_file) {
                if let Ok(pid) = raw.trim().parse::<u32>() {
                    return Ok(pid);
                }
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "piped fixture did not publish its descendant pid"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    fn await_fixture_file(path: &Path, timeout: Duration) -> anyhow::Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if path.exists() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(10));
        }
        anyhow::bail!(
            "fixture did not publish readiness marker {}",
            path.display()
        )
    }

    #[cfg(unix)]
    fn wait_for_piped_process_exit(pid: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(windows)]
    fn wait_for_piped_process_exit(pid: u32, timeout: Duration) -> bool {
        wait_for_windows_process_exit(pid, timeout)
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn piped_child_propagates_nonzero_exit_status() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let command = piped_nonzero_fixture(temp_dir.path());
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };

        let session = spawn_piped_with_observer(&command, Some(observer), false)?;
        let process_tree = session.activate(|_input, process_tree| Ok(process_tree))?;
        let result = exit_rx.recv_timeout(Duration::from_secs(10))?;
        drop(process_tree);

        assert_eq!(result.status, "failed", "{result:?}");
        assert_eq!(result.exit_code, Some(23), "{result:?}");
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn piped_process_tree_explicit_cancel_kills_descendant() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("descendant.pid");
        let command = piped_descendant_fixture(temp_dir.path(), &pid_file);
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };
        let session = spawn_piped_with_observer(&command, Some(observer), false)?;
        let process_tree = session.activate(|_input, process_tree| Ok(process_tree))?;
        let descendant_pid = await_piped_descendant_pid(&pid_file)?;

        process_tree.terminate_tree()?;
        let result = exit_rx.recv_timeout(Duration::from_secs(10))?;
        drop(process_tree);

        assert_eq!(result.status, "failed", "{result:?}");
        assert!(
            wait_for_piped_process_exit(descendant_pid, Duration::from_secs(10)),
            "explicit cancellation left descendant {descendant_pid} running"
        );
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn dropping_piped_process_tree_handle_kills_descendant() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("descendant.pid");
        let command = piped_descendant_fixture(temp_dir.path(), &pid_file);
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };
        let session = spawn_piped_with_observer(&command, Some(observer), false)?;
        let process_tree = session.activate(|_input, process_tree| Ok(process_tree))?;
        let descendant_pid = await_piped_descendant_pid(&pid_file)?;

        drop(process_tree);
        let result = exit_rx.recv_timeout(Duration::from_secs(10))?;

        assert_eq!(result.status, "failed", "{result:?}");
        assert!(
            wait_for_piped_process_exit(descendant_pid, Duration::from_secs(10)),
            "dropping containment left descendant {descendant_pid} running"
        );
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_kill_on_job_close_backstop_kills_descendant() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("descendant.pid");
        let command = piped_descendant_fixture(temp_dir.path(), &pid_file);
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };
        let session = spawn_piped_with_observer(&command, Some(observer), false)?;
        let process_tree = session.activate(|_input, process_tree| Ok(process_tree))?;
        let descendant_pid = await_piped_descendant_pid(&pid_file)?;

        process_tree.close_job_handle_without_explicit_termination_for_test();
        let _ = exit_rx.recv_timeout(Duration::from_secs(10))?;
        drop(process_tree);

        assert!(
            wait_for_piped_process_exit(descendant_pid, Duration::from_secs(10)),
            "KILL_ON_JOB_CLOSE backstop left descendant {descendant_pid} running"
        );
        Ok(())
    }

    #[derive(Clone)]
    struct RecordingResizeTarget {
        sizes: Arc<Mutex<Vec<PtySize>>>,
        fail: bool,
    }

    impl PtyResizeTarget for RecordingResizeTarget {
        fn resize_pty(&self, size: PtySize) -> Result<()> {
            if self.fail {
                anyhow::bail!("synthetic resize failure");
            }
            self.sizes.lock().unwrap().push(size);
            Ok(())
        }
    }

    struct DropAwareResizeTarget {
        sizes: mpsc::Sender<PtySize>,
        dropped: Option<mpsc::Sender<()>>,
        fail: bool,
    }

    impl PtyResizeTarget for DropAwareResizeTarget {
        fn resize_pty(&self, size: PtySize) -> Result<()> {
            self.sizes
                .send(size)
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            if self.fail {
                anyhow::bail!("synthetic resize failure");
            }
            Ok(())
        }
    }

    impl Drop for DropAwareResizeTarget {
        fn drop(&mut self) {
            if let Some(dropped) = self.dropped.take() {
                let _ = dropped.send(());
            }
        }
    }

    #[cfg(unix)]
    fn read_pty_line_with_timeout<R: BufRead>(
        reader: &mut R,
        raw_fd: libc::c_int,
        timeout: Duration,
    ) -> io::Result<String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out waiting for PTY output",
                ));
            }
            let timeout_ms = i32::try_from(remaining.as_millis().max(1)).unwrap_or(i32::MAX);
            let mut descriptor = libc::pollfd {
                fd: raw_fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: descriptor points to one valid pollfd for the duration of the call.
            let ready = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
            if ready > 0 {
                let mut line = String::new();
                if reader.read_line(&mut line)? == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "PTY closed before a complete line arrived",
                    ));
                }
                return Ok(line.trim().to_string());
            }
            if ready == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out waiting for PTY output",
                ));
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    #[cfg(unix)]
    fn wait_for_pty_child_until(
        child: &mut Box<dyn portable_pty::Child + Send + Sync>,
        deadline: Instant,
    ) -> io::Result<Option<portable_pty::ExitStatus>> {
        while Instant::now() < deadline {
            if let Some(status) = child.try_wait()? {
                return Ok(Some(status));
            }
            thread::sleep(Duration::from_millis(10));
        }
        child.try_wait()
    }

    #[cfg(unix)]
    fn finalize_pty_child(
        child: &mut Box<dyn portable_pty::Child + Send + Sync>,
    ) -> anyhow::Result<portable_pty::ExitStatus> {
        if let Some(status) =
            wait_for_pty_child_until(child, Instant::now() + Duration::from_secs(5))?
        {
            return Ok(status);
        }

        if let Err(kill_error) = child.kill() {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            return Err(kill_error).context("failed to kill PTY child during test cleanup");
        }

        wait_for_pty_child_until(child, Instant::now() + Duration::from_secs(5))?
            .context("PTY child was not reaped after cleanup kill")
    }

    #[test]
    fn pty_resize_watcher_skips_duplicates_and_survives_missing_samples() {
        let initial = pty_size(24, 80, 0, 0);
        let resized = pty_size(40, 120, 1440, 800);
        let resized_again = pty_size(48, 132, 1584, 960);
        let samples = Arc::new(Mutex::new(VecDeque::from([
            None,
            Some(pty_size(0, 120, 0, 0)),
            Some(initial),
            Some(resized),
            Some(resized),
            None,
            Some(resized_again),
        ])));
        let samples_for_source = Arc::clone(&samples);
        let (sizes_tx, sizes_rx) = mpsc::channel();
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let target = DropAwareResizeTarget {
            sizes: sizes_tx,
            dropped: Some(dropped_tx),
            fail: false,
        };

        let mut watcher = PtyResizeWatcher::spawn_with_source(
            target,
            initial,
            move || samples_for_source.lock().unwrap().pop_front().flatten(),
            Duration::from_millis(1),
        );

        assert_eq!(
            sizes_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            resized,
        );
        assert_eq!(
            sizes_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            resized_again,
        );
        assert!(sizes_rx.recv_timeout(Duration::from_millis(25)).is_err());
        watcher.stop();
        dropped_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn pty_resize_watcher_drop_interrupts_long_poll_and_drops_target() {
        let (sizes_tx, _sizes_rx) = mpsc::channel();
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let target = DropAwareResizeTarget {
            sizes: sizes_tx,
            dropped: Some(dropped_tx),
            fail: false,
        };
        let watcher = PtyResizeWatcher::spawn_with_source(
            target,
            pty_size(24, 80, 0, 0),
            || None,
            Duration::from_secs(60),
        );
        let started = Instant::now();
        drop(watcher);

        dropped_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn pty_resize_watcher_exits_and_drops_target_after_resize_failure() {
        let resized = pty_size(40, 120, 0, 0);
        let (sizes_tx, sizes_rx) = mpsc::channel();
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let target = DropAwareResizeTarget {
            sizes: sizes_tx,
            dropped: Some(dropped_tx),
            fail: true,
        };

        let mut watcher = PtyResizeWatcher::spawn_with_source(
            target,
            pty_size(24, 80, 0, 0),
            move || Some(resized),
            Duration::from_millis(1),
        );

        assert_eq!(
            sizes_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            resized,
        );
        assert!(sizes_rx.recv_timeout(Duration::from_millis(100)).is_err());
        assert!(dropped_rx.recv_timeout(Duration::from_millis(100)).is_err());
        watcher.stop();
        dropped_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn pty_resize_watcher_updates_real_child_geometry() -> anyhow::Result<()> {
        let initial = pty_size(24, 80, 0, 0);
        let resized = pty_size(40, 120, 0, 0);
        let pair = native_pty_system().openpty(initial)?;
        let raw_fd = pair
            .master
            .as_raw_fd()
            .context("real PTY master did not expose a file descriptor")?;
        let reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let mut command = CommandBuilder::new("sh");
        command.args(["-c", "stty -echo; stty size; IFS= read -r _; stty size"]);
        let mut child = pair.slave.spawn_command(command)?;
        drop(pair.slave);
        let mut reader = BufReader::new(reader);
        let first = read_pty_line_with_timeout(&mut reader, raw_fd, Duration::from_secs(5));
        let (sampled_tx, sampled_rx) = mpsc::channel();
        let mut samples = 0;
        let mut watcher = PtyResizeWatcher::spawn_with_source(
            pair.master,
            initial,
            move || {
                samples += 1;
                if samples == 2 {
                    let _ = sampled_tx.send(());
                }
                Some(resized)
            },
            Duration::from_millis(1),
        );
        let sampled_twice = sampled_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| anyhow::anyhow!("resize source did not run twice: {error}"));
        let input_sent = writer
            .write_all(b"\n")
            .and_then(|_| writer.flush())
            .context("failed to release child after resize");
        let second = read_pty_line_with_timeout(&mut reader, raw_fd, Duration::from_secs(5));

        watcher.stop();
        drop(writer);
        let status = finalize_pty_child(&mut child);

        let first = first.context("failed to read initial stty size")?;
        sampled_twice?;
        input_sent?;
        let second = second.context("failed to read resized stty size")?;
        anyhow::ensure!(first == "24 80", "unexpected initial geometry: {first}");
        anyhow::ensure!(second == "40 120", "unexpected resized geometry: {second}");
        anyhow::ensure!(
            status?.success(),
            "PTY child did not exit successfully after resize"
        );
        Ok(())
    }

    #[test]
    fn pty_resize_ignores_missing_invalid_and_unchanged_samples() {
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let target = RecordingResizeTarget {
            sizes: Arc::clone(&sizes),
            fail: false,
        };
        let initial = pty_size(24, 80, 0, 0);
        let mut current = initial;

        assert!(apply_pty_resize(&target, &mut current, None));
        assert!(apply_pty_resize(
            &target,
            &mut current,
            Some(pty_size(0, 120, 0, 0)),
        ));
        assert!(apply_pty_resize(&target, &mut current, Some(initial),));
        assert!(sizes.lock().unwrap().is_empty());
        assert_eq!(current, initial);
    }

    #[test]
    fn pty_resize_applies_each_distinct_complete_geometry_once() {
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let target = RecordingResizeTarget {
            sizes: Arc::clone(&sizes),
            fail: false,
        };
        let mut current = pty_size(24, 80, 0, 0);
        let first = pty_size(40, 120, 1440, 800);
        let second = pty_size(40, 120, 1680, 900);

        assert!(apply_pty_resize(&target, &mut current, Some(first)));
        assert!(apply_pty_resize(&target, &mut current, Some(first)));
        assert!(apply_pty_resize(&target, &mut current, Some(second)));
        assert_eq!(*sizes.lock().unwrap(), vec![first, second]);
        assert_eq!(current, second);
    }

    #[test]
    fn pty_resize_failure_stops_relay_without_advancing_geometry() {
        let target = RecordingResizeTarget {
            sizes: Arc::new(Mutex::new(Vec::new())),
            fail: true,
        };
        let initial = pty_size(24, 80, 0, 0);
        let mut current = initial;

        assert!(!apply_pty_resize(
            &target,
            &mut current,
            Some(pty_size(40, 120, 0, 0)),
        ));
        assert_eq!(current, initial);
    }

    #[test]
    fn pty_geometry_prefers_live_terminal_size_and_pixels() {
        let live = pty_size(52, 151, 1812, 936);

        assert_eq!(
            terminal_size_from_sources(Some(live), Some(24), Some(80)),
            live,
        );
    }

    #[test]
    fn pty_geometry_falls_back_to_cell_size_when_pixels_are_unavailable() {
        let live = pty_size(52, 151, 1812, 936);

        assert_eq!(
            detected_terminal_size_from_sources(None, Some((151, 52))),
            Some(pty_size(52, 151, 0, 0)),
        );
        assert_eq!(
            detected_terminal_size_from_sources(Some(live), Some((80, 24))),
            Some(live),
        );
    }

    #[test]
    fn pty_geometry_rejects_zero_detected_sources() {
        let invalid_window = pty_size(0, 151, 1812, 936);

        assert_eq!(detected_terminal_size_from_sources(None, None), None);
        assert_eq!(
            detected_terminal_size_from_sources(Some(invalid_window), Some((132, 41))),
            Some(pty_size(41, 132, 0, 0)),
        );
        assert_eq!(
            detected_terminal_size_from_sources(None, Some((132, 0))),
            None,
        );
        assert_eq!(
            detected_terminal_size_from_sources(None, Some((0, 41))),
            None,
        );
    }

    #[test]
    fn pty_geometry_rejects_zero_live_dimensions() {
        let invalid = pty_size(0, 151, 1812, 936);

        assert_eq!(
            terminal_size_from_sources(Some(invalid), Some(41), Some(132)),
            pty_size(41, 132, 0, 0),
        );
    }

    #[test]
    fn pty_geometry_uses_each_environment_fallback_independently() {
        assert_eq!(
            terminal_size_from_sources(None, Some(41), None),
            pty_size(41, 80, 0, 0),
        );
        assert_eq!(
            terminal_size_from_sources(None, None, Some(132)),
            pty_size(24, 132, 0, 0),
        );
    }

    #[test]
    fn pty_geometry_defaults_when_every_source_is_unavailable() {
        assert_eq!(
            terminal_size_from_sources(None, None, None),
            pty_size(24, 80, 0, 0),
        );
    }

    #[test]
    fn builds_codex_command_without_shell_interpolation() {
        let cwd = Path::new("/tmp/coven project");
        let command = build_harness_command(
            "codex",
            "hello; rm -rf /",
            cwd,
            crate::harness::HarnessLaunchMode::Interactive,
        )
        .unwrap();

        assert_eq!(command.program(), "codex");
        assert_eq!(command.args(), &["--", "hello; rm -rf /"]);
        assert_eq!(command.cwd(), cwd);
    }

    #[cfg(unix)]
    #[test]
    fn spawn_detached_starts_pty_and_returns_input_and_kill_handles() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let command = HarnessCommand {
            program: "cat".to_string(),
            args: vec![],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: None,
            env_overrides: Vec::new(),
        };

        let mut session = spawn_detached(&command)?;
        session.input.write_all(b"hello detached pty\n")?;
        session.input.flush()?;
        session.killer.kill()?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn detached_startup_timeout_is_disabled() {
        assert_eq!(detached_startup_timeout(), None);
    }

    #[cfg(windows)]
    fn assert_windows_detached_pty_queries(
        command: &HarnessCommand,
        trace_file: &Path,
    ) -> anyhow::Result<()> {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_output = Arc::clone(&captured);
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(move |chunk| {
                captured_for_output.lock().unwrap().extend(chunk);
            }),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };

        let mut session = spawn_detached_with_observer_and_timeout(
            &command,
            Some(observer),
            // Native CI can schedule ConPTY terminal replies several seconds
            // apart under load. This path must complete all four query/reply
            // exchanges before it emits meaningful output, so keep the proof
            // bounded without racing a healthy interactive child.
            Some(Duration::from_secs(15)),
        )?;
        let result = match exit_rx.recv_timeout(Duration::from_secs(20)) {
            Ok(result) => result,
            Err(error) => {
                let _ = session.killer.kill();
                anyhow::bail!(
                    "{error}; trace: {:?}; observed: {:?}",
                    std::fs::read_to_string(&trace_file),
                    String::from_utf8_lossy(&captured.lock().unwrap())
                );
            }
        };

        let observed = String::from_utf8_lossy(&captured.lock().unwrap()).into_owned();
        let trace = std::fs::read_to_string(&trace_file).unwrap_or_default();
        assert_eq!(
            result.status, "completed",
            "result: {result:?}; observed output: {observed:?}; trace: {trace:?}"
        );
        assert_eq!(result.exit_code, Some(0));
        assert!(observed.contains("WINDOWS_PTY_STUB_OK_🎉"), "{observed:?}");
        for query in ["\x1b[6n", "\x1b[c", "\x1b[0c", "\x1b[5n"] {
            assert!(!observed.contains(query), "query leaked: {query:?}");
        }
        assert!(trace.starts_with("started mode="), "{trace:?}");
        for stage in ["cpr", "da", "status", "da0"] {
            assert!(trace.lines().any(|line| line == stage), "{trace:?}");
        }
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_detached_pty_stub_completes_after_terminal_replies() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let trace_file = temp_dir.path().join("query-trace.txt");
        let command = windows_detached_stub_command(temp_dir.path(), "queries", Some(&trace_file))?;

        assert_windows_detached_pty_queries(&command, &trace_file)
    }

    #[cfg(windows)]
    #[test]
    fn windows_interactive_codex_batch_shim_remains_pty_interactive() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let trace_file = temp_dir.path().join("query-trace.txt");
        // Compile the same console/PTY fixture used by the direct executable
        // regression, then put an npm-style batch boundary in front of it.
        let _native = windows_detached_stub_command(temp_dir.path(), "queries", Some(&trace_file))?;
        let shim = temp_dir.path().join("codex.cmd");
        std::fs::write(
            &shim,
            concat!(
                "@echo off\r\n",
                "\"%~dp0windows-detached-pty-stub.exe\" queries ",
                "\"%~dp0query-trace.txt\" %*\r\n"
            ),
        )?;

        let shim = shim.to_string_lossy().into_owned();
        let (program, env_overrides) = prepare_harness_program(
            "codex",
            crate::harness::HarnessLaunchMode::Interactive,
            shim.clone(),
        )?;
        assert_eq!(program, shim, "interactive Codex must retain its npm shim");
        assert!(
            env_overrides.is_empty(),
            "interactive Codex must not receive native-package overrides"
        );
        let args = crate::harness::sanitize_argv_for_program(
            "codex",
            crate::harness::HarnessLaunchMode::Interactive,
            &program,
            vec!["--".to_string(), "interactive PTY smoke".to_string()],
        );
        let command = HarnessCommand {
            program,
            args,
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: None,
            env_overrides,
        };

        assert_windows_detached_pty_queries(&command, &trace_file)
    }

    /// Deliberately short, and shorter is safer here.
    ///
    /// `spawn_detached_with_observer_and_timeout` races three claimants on one
    /// `AtomicU8`: meaningful output, child exit, and the timeout thread's
    /// `compare_exchange(0 -> 2)`. Only the winner acts, so if anything
    /// printable reaches the PTY first the timeout never fires at all — the
    /// stub then blocks forever in `expect_reply`, `on_exit` never runs, and
    /// every watchdog in this test expires.
    ///
    /// The stub's descendant is `cmd.exe ... >nul`, which normally emits
    /// nothing, but any stray printable byte claims the state permanently.
    /// Lengthening this window widens the opportunity for that, which is what
    /// an earlier attempt at `coven-5ua` did: raising it to 15s traded the
    /// pid-file race for a worse failure where the timeout lost the race
    /// outright. Two seconds is enough because the pid is now polled for
    /// concurrently rather than read after the kill.
    #[cfg(windows)]
    const WINDOWS_DETACHED_STARTUP_TIMEOUT: Duration = Duration::from_secs(2);

    /// Containment watchdogs. Reaching one means something is hung, not that
    /// the host was slow, so they are generous on purpose.
    #[cfg(windows)]
    const WINDOWS_DETACHED_TEST_WATCHDOG: Duration = Duration::from_secs(60);

    /// Wait for the fixture to record its descendant's pid.
    ///
    /// Reading the file once races the stub writing it. Polling until the
    /// startup timeout has had time to fire keeps the failure message
    /// meaningful: if this expires the fixture genuinely never registered,
    /// which is a fixture problem rather than a runner one.
    #[cfg(windows)]
    fn await_windows_descendant_pid(pid_file: &Path, deadline: Instant) -> Option<u32> {
        loop {
            if let Ok(raw) = std::fs::read_to_string(pid_file) {
                if let Ok(pid) = raw.trim().parse::<u32>() {
                    return Some(pid);
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_detached_pty_timeout_fails_and_kills_descendant() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("descendant.pid");
        let command = windows_detached_stub_command(temp_dir.path(), "timeout", Some(&pid_file))?;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_output = Arc::clone(&captured);
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(move |chunk| {
                captured_for_output.lock().unwrap().extend(chunk);
            }),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };

        let mut session = spawn_detached_with_observer_and_timeout(
            &command,
            Some(observer),
            Some(WINDOWS_DETACHED_STARTUP_TIMEOUT),
        )?;
        // Capture the pid while the stub is still running. Reading it after
        // the exit callback races the runner's own process-tree kill.
        let pid_deadline = Instant::now() + WINDOWS_DETACHED_STARTUP_TIMEOUT;
        let descendant_pid = await_windows_descendant_pid(&pid_file, pid_deadline);

        let result = match exit_rx.recv_timeout(WINDOWS_DETACHED_TEST_WATCHDOG) {
            Ok(result) => result,
            Err(error) => {
                let _ = session.killer.kill();
                // Say which race was lost instead of only that a deadline
                // passed. The startup timeout emits its own message when it
                // wins; its absence here means something else claimed
                // `startup_state` first and the timeout never fired, which is
                // a different defect from slow reaping under load.
                let observed = String::from_utf8_lossy(&captured.lock().unwrap()).into_owned();
                let diagnosis = if observed.contains("no meaningful output") {
                    "startup timeout fired but the child was never reaped"
                } else {
                    "startup timeout never fired: another claimant won startup_state"
                };
                return Err(anyhow::anyhow!(
                    "{error}: {diagnosis}; observed output: {observed:?}"
                ));
            }
        };
        let descendant_pid = descendant_pid.with_context(|| {
            format!(
                "timeout stub did not create pid file within {:?}; observed output: {:?}",
                WINDOWS_DETACHED_STARTUP_TIMEOUT,
                String::from_utf8_lossy(&captured.lock().unwrap())
            )
        })?;

        assert_eq!(result.status, "failed");
        assert_eq!(result.exit_code, None);
        let output = String::from_utf8(captured.lock().unwrap().clone())?;
        assert!(output.contains("no meaningful output"), "{output:?}");
        assert!(!output.contains("\x1b[6n"), "query leaked: {output:?}");
        assert!(
            wait_for_windows_process_exit(descendant_pid, WINDOWS_DETACHED_TEST_WATCHDOG),
            "startup timeout left descendant process {descendant_pid} running"
        );
        Ok(())
    }

    #[cfg(windows)]
    fn wait_for_windows_process_exit(pid: u32, timeout: Duration) -> bool {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{OpenProcess, WaitForSingleObject},
        };
        // SAFETY: the process handle is checked and closed exactly once.
        unsafe {
            const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
            let process = OpenProcess(SYNCHRONIZE_ACCESS, 0, pid);
            if process == 0 as _ {
                return true;
            }
            let milliseconds = timeout.as_millis().min(u32::MAX as u128) as u32;
            let result = WaitForSingleObject(process, milliseconds);
            CloseHandle(process);
            result == WAIT_OBJECT_0
        }
    }

    /// Serializes the fake-claude tests: each writes an executable script and
    /// immediately spawns it. Run in parallel, one test's `fork` can inherit
    /// another's still-open write fd and the exec fails with ETXTBSY
    /// ("Text file busy") — a real CI flake, not a theoretical one.
    #[cfg(unix)]
    static FAKE_CLAUDE_SPAWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    #[cfg(unix)]
    const CODEX_JSON_TEST_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

    #[cfg(unix)]
    fn fake_claude_spawn_guard() -> std::sync::MutexGuard<'static, ()> {
        FAKE_CLAUDE_SPAWN_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(unix)]
    #[test]
    fn codex_json_runner_normalizes_agent_message_and_captures_thread() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_codex = temp_dir.path().join("fake-codex");
        std::fs::write(
            &fake_codex,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
printf '%s\n' '{"type":"thread.started","thread_id":"thread-123"}'
printf '%s\n' '{"type":"turn.started"}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"Coven reply"}}'
printf '%s\n' '{"type":"turn.completed"}'
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_codex)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_codex, permissions)?;
        let command = HarnessCommand {
            program: fake_codex.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--model".to_string(),
                "gpt-5.5".to_string(),
                "--".to_string(),
                "reply exactly once".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: None,
            env_overrides: Vec::new(),
        };
        let mut assistant = Vec::new();

        let outcome = stream_codex_json_with_budgets(
            &command,
            CODEX_JSON_TEST_STARTUP_TIMEOUT,
            Duration::from_secs(1),
            CODEX_POST_EXIT_DRAIN_TIMEOUT,
            |text| {
                assistant.push(text.to_string());
                Ok(())
            },
        )?;

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "exec\n--json\n--model\ngpt-5.5\n--\nreply exactly once\n"
        );
        assert_eq!(assistant, vec!["Coven reply"]);
        assert_eq!(outcome.harness_session_id.as_deref(), Some("thread-123"));
        assert!(outcome.emitted_assistant);
        assert!(outcome.error.is_none());
        assert_eq!(outcome.process.exit_code, Some(0));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn codex_json_runner_times_out_and_reaps_a_silent_child() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_codex = temp_dir.path().join("fake-codex");
        std::fs::write(
            &fake_codex,
            r#"#!/bin/sh
echo $$ > child.pid
printf '%s\n' '{"type":"thread.started","thread_id":"timeout-ready"}'
exec sleep 10
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_codex)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_codex, permissions)?;
        let command = HarnessCommand {
            program: fake_codex.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--".to_string(),
                "prompt".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: None,
            env_overrides: Vec::new(),
        };

        // The first frame is a readiness barrier; the child is silent after it,
        // so the one-second activity timeout still exercises termination.
        let outcome = stream_codex_json_with_budgets(
            &command,
            CODEX_JSON_TEST_STARTUP_TIMEOUT,
            Duration::from_secs(1),
            CODEX_POST_EXIT_DRAIN_TIMEOUT,
            |_| Ok(()),
        )?;

        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("terminated")));
        let pid = std::fs::read_to_string(temp_dir.path().join("child.pid"))?
            .trim()
            .parse::<u32>()?;
        assert!(
            wait_for_piped_process_exit(pid, Duration::from_secs(2)),
            "timed-out child {pid} should be reaped"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn codex_json_runner_times_out_while_a_large_prompt_is_still_writing() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_codex = temp_dir.path().join("silent-codex");
        std::fs::write(&fake_codex, "#!/bin/sh\nexec sleep 10\n")?;
        let mut permissions = std::fs::metadata(&fake_codex)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_codex, permissions)?;
        let command = HarnessCommand {
            program: fake_codex.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--".to_string(),
                "-".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            // Far larger than an anonymous-pipe buffer. A synchronous write
            // would block indefinitely because the fake harness never reads.
            stdin_prompt: Some(vec![b'x'; 1024 * 1024]),
            env_overrides: Vec::new(),
        };

        let outcome =
            stream_codex_json_with_timeout(&command, Duration::from_millis(25), |_| Ok(()))?;

        // The terminated outcome proves the activity timeout won while stdin
        // was blocked. A wall-clock assertion here measures runner load, not
        // the timeout behavior.
        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("terminated")));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn codex_json_runner_reaps_a_pipe_holding_descendant_after_wrapper_exit() -> anyhow::Result<()>
    {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_codex = temp_dir.path().join("wrapper-codex");
        std::fs::write(
            &fake_codex,
            r#"#!/bin/sh
sleep 10 &
echo $! > descendant.pid
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_codex)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_codex, permissions)?;
        let command = HarnessCommand {
            program: fake_codex.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--".to_string(),
                "prompt".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: None,
            env_overrides: Vec::new(),
        };

        let outcome = stream_codex_json_with_budgets(
            &command,
            CODEX_JSON_TEST_STARTUP_TIMEOUT,
            Duration::from_secs(1),
            Duration::from_millis(25),
            |_| Ok(()),
        )?;

        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("without an assistant message")));
        let pid = std::fs::read_to_string(temp_dir.path().join("descendant.pid"))?
            .trim()
            .parse::<u32>()?;
        assert!(
            wait_for_piped_process_exit(pid, Duration::from_secs(2)),
            "descendant {pid} should be reaped with its process group"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn codex_json_runner_reaps_a_closed_pipe_descendant_after_wrapper_exit() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_codex = temp_dir.path().join("wrapper-codex");
        std::fs::write(
            &fake_codex,
            r#"#!/bin/sh
sleep 10 </dev/null >/dev/null 2>&1 &
echo $! > descendant.pid
printf '%s\n' '{"type":"thread.started","thread_id":"thread-closed-pipe"}'
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"reply before wrapper failure"}}'
exit 23
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_codex)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_codex, permissions)?;
        let command = HarnessCommand {
            program: fake_codex.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--".to_string(),
                "prompt".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: None,
            env_overrides: Vec::new(),
        };
        let mut assistant = Vec::new();

        let outcome = stream_codex_json_with_budgets(
            &command,
            CODEX_JSON_TEST_STARTUP_TIMEOUT,
            Duration::from_secs(1),
            CODEX_POST_EXIT_DRAIN_TIMEOUT,
            |text| {
                assistant.push(text.to_string());
                Ok(())
            },
        )?;

        assert_eq!(assistant, vec!["reply before wrapper failure"]);
        assert_eq!(outcome.process.status, "failed");
        assert_eq!(outcome.process.exit_code, Some(23));
        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Codex exited with 23")));
        let pid = std::fs::read_to_string(temp_dir.path().join("descendant.pid"))?
            .trim()
            .parse::<u32>()?;
        assert!(
            wait_for_piped_process_exit(pid, Duration::from_secs(2)),
            "closed-pipe descendant {pid} should be reaped with its process group"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn codex_json_runner_synthesizes_nonzero_exit_for_protocol_error() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_codex = temp_dir.path().join("failed-codex");
        std::fs::write(
            &fake_codex,
            r#"#!/bin/sh
printf '%s\n' '{"type":"turn.failed","error":{"message":"fake turn failure"}}'
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_codex)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_codex, permissions)?;
        let command = HarnessCommand {
            program: fake_codex.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--".to_string(),
                "prompt".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: None,
            env_overrides: Vec::new(),
        };

        let outcome = stream_codex_json_with_budgets(
            &command,
            CODEX_JSON_TEST_STARTUP_TIMEOUT,
            Duration::from_secs(1),
            CODEX_POST_EXIT_DRAIN_TIMEOUT,
            |_| Ok(()),
        )?;

        assert_eq!(outcome.process.status, "failed");
        assert_eq!(outcome.process.exit_code, Some(1));
        assert_eq!(outcome.error.as_deref(), Some("fake turn failure"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stream_harness_claude_forwards_jsonl_and_returns_exit_code() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_claude = temp_dir.path().join("fake-claude");
        std::fs::write(
            &fake_claude,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
printf '\n'
printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]},"session_id":"session-123","stop_reason":"end_turn"}'
exit 7
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_claude)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_claude, permissions)?;

        let mut out = Vec::new();
        let code = stream_harness_with_claude_args(
            fake_claude.to_str().unwrap(),
            temp_dir.path(),
            "session-123",
            false,
            "hello prompt",
            false,
            None,
            crate::harness::HarnessLaunchOptions::default(),
            &mut out,
        )?;

        assert_eq!(code, 7);
        // One-shot mode (forward_stdin=false): `--input-format stream-json`
        // is omitted so the positional prompt is honored. Including it
        // makes claude wait for JSONL on stdin and ignore the positional —
        // which is the bug this commit fixes.
        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "-p\n--output-format\nstream-json\n--verbose\n--session-id\nsession-123\n--\nhello prompt\n"
        );
        let frame: serde_json::Value = serde_json::from_slice(&out)?;
        assert_eq!(frame["session_id"], "session-123");
        assert_eq!(frame["harness_session_id"], "session-123");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stream_harness_forwards_declared_args_without_claude_rebuild() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_harness = temp_dir.path().join("fake-streamy");
        std::fs::write(
            &fake_harness,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"streamy","session_id":"nested-unchanged"}]},"session_id":"native-old","harness_session_id":"native-old","stop_reason":"end_turn"}'
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_harness)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_harness, permissions)?;

        let mut out = Vec::new();
        let code = stream_harness_with_program(
            fake_harness.to_str().unwrap(),
            temp_dir.path(),
            vec![
                "--jsonl".to_string(),
                "--resume".to_string(),
                "session-123".to_string(),
                "--".to_string(),
                "hello prompt".to_string(),
            ],
            false,
            "streamy",
            "ledger-current",
            &mut out,
        )?;

        assert_eq!(code, 0);
        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "--jsonl\n--resume\nsession-123\n--\nhello prompt\n"
        );
        let frame: serde_json::Value = serde_json::from_slice(&out)?;
        assert_eq!(frame["session_id"], "ledger-current");
        assert_eq!(frame["harness_session_id"], "native-old");
        assert_eq!(
            frame["message"]["content"][0]["session_id"],
            "nested-unchanged"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_recorded_before_spawn_is_returned_by_checks_and_finish() -> Result<()> {
        let cancellation = SupervisedStreamCancellationGuard::install("test stream")?;
        cancel_supervised_stream(libc::SIGTERM);

        assert_eq!(cancellation.cancelled_signal(), Some(libc::SIGTERM));
        assert_eq!(cancellation.finish()?, Some(libc::SIGTERM));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_handler_has_no_process_group_side_effect_by_construction() {
        let source = include_str!("pty_runner.rs");
        assert!(
            !source.contains(concat!("SUPERVISED_STREAM_", "PROCESS_GROUP")),
            "the signal handler design must not retain a numeric process-group target"
        );
        let handler = source
            .split_once("extern \"C\" fn cancel_supervised_stream")
            .expect("cancellation handler exists")
            .1
            .split_once("/// Temporarily converts")
            .expect("cancellation handler is followed by its guard documentation")
            .0;
        assert!(
            !handler.contains("libc::kill"),
            "the async signal handler must only record cancellation"
        );
    }

    #[cfg(unix)]
    #[test]
    fn wait_without_reaping_keeps_child_pid_reserved() -> Result<()> {
        let mut command = std::process::Command::new("/bin/sh");
        command.arg("-c").arg("exit 0");
        configure_child_process_tree_command(&mut command);
        let mut child = command.spawn()?;
        let pid = child.id() as libc::pid_t;

        wait_for_child_exit_without_reaping(&child)?;

        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            0,
            "wait helper reaped the child before process-tree cleanup"
        );
        assert!(child.wait()?.success());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn nonblocking_wait_without_reaping_does_not_report_live_child_exited() -> Result<()> {
        let mut command = std::process::Command::new("/bin/sh");
        command.arg("-c").arg("sleep 1");
        configure_child_process_tree_command(&mut command);
        let mut child = command.spawn()?;

        assert!(
            !poll_child_exit_without_reaping(&child)?,
            "WNOHANG reported a live direct child as exited"
        );

        let _ = child.kill();
        let _ = child.wait();
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_guard_blocks_supervised_signals_for_spawned_helpers() -> Result<()> {
        // Snapshot the CALLING thread's supervised-signal membership BEFORE the
        // guard is installed. `finish()` restores the calling thread's mask via
        // SIG_SETMASK to whatever it saved at install time, so the post-finish
        // membership must equal this pre-install snapshot exactly. Gap (3) of
        // the Psyche O1.1 conformance plan (#641) is this deterministic
        // round-trip assertion — previously only the mid-install BLOCKED state
        // (on a freshly spawned helper thread) was checked.
        let probe_calling_mask = || {
            let mut mask: libc::sigset_t = unsafe { std::mem::zeroed() };
            let result =
                unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut mask) };
            assert_eq!(result, 0, "failed to read calling thread signal mask");
            [libc::SIGTERM, libc::SIGINT, libc::SIGHUP]
                .map(|signal| unsafe { libc::sigismember(&mask, signal) })
        };
        let pre_install_mask = probe_calling_mask();

        let cancellation = SupervisedStreamCancellationGuard::install("test stream")?;
        let inherited_mask = thread::spawn(|| {
            let mut mask: libc::sigset_t = unsafe { std::mem::zeroed() };
            let result =
                unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut mask) };
            assert_eq!(result, 0);
            [libc::SIGINT, libc::SIGTERM, libc::SIGHUP]
                .map(|signal| unsafe { libc::sigismember(&mask, signal) })
        })
        .join()
        .expect("signal-mask probe thread panicked");

        assert_eq!(inherited_mask, [1, 1, 1]);
        assert_eq!(cancellation.finish()?, None);

        // Post-finish: the calling thread's supervised-signal membership must be
        // exactly what it was before install — SIGTERM/SIGINT/SIGHUP unchanged.
        assert_eq!(
            probe_calling_mask(),
            pre_install_mask,
            "finish() did not restore the calling thread's signal mask to its pre-install value"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn native_stream_does_not_wait_for_stdout_inheriting_descendant() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir()?;
        let fake_harness = temp_dir.path().join("stdout-descendant-stream");
        std::fs::write(
            &fake_harness,
            r#"#!/bin/sh
sleep 2
printf 'ready\n' > stream.ready
printf '%s\n' '{"type":"assistant","session_id":"native","message":{"role":"assistant","content":[]}}'
sleep 5 &
exit 17
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_harness)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_harness, permissions)?;

        let program = fake_harness.to_string_lossy().into_owned();
        let cwd = temp_dir.path().to_path_buf();
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        let runner = thread::spawn(move || {
            let result = stream_harness_with_program(
                &program,
                &cwd,
                Vec::new(),
                false,
                "streamy",
                "ledger-current",
                &mut Vec::new(),
            );
            let _ = result_tx.send(result);
        });
        if let Err(error) = await_fixture_file(
            &temp_dir.path().join("stream.ready"),
            Duration::from_secs(10),
        ) {
            let early = result_rx.recv_timeout(Duration::from_secs(6));
            runner
                .join()
                .map_err(|_| anyhow::anyhow!("native stream runner panicked"))?;
            return Err(error.context(format!(
                "native stream failed before fixture readiness; result: {early:?}"
            )));
        }
        let code = match result_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let late = result_rx.recv_timeout(Duration::from_secs(6));
                runner
                    .join()
                    .map_err(|_| anyhow::anyhow!("native stream runner panicked"))?;
                anyhow::bail!(
                    "native stream did not finish post-exit cleanup within two seconds; late result: {late:?}"
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                runner
                    .join()
                    .map_err(|_| anyhow::anyhow!("native stream runner panicked"))?;
                anyhow::bail!("native stream runner disconnected without a result");
            }
        };
        runner
            .join()
            .map_err(|_| anyhow::anyhow!("native stream runner panicked"))?;
        let code = code?;

        assert_eq!(code, 17);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn successful_native_stream_cleans_closed_output_descendant() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir()?;
        let fake_harness = temp_dir.path().join("successful-stream");
        let descendant_pid_file = temp_dir.path().join("descendant.pid");
        std::fs::write(
            &fake_harness,
            r#"#!/bin/sh
sleep 30 </dev/null >/dev/null 2>&1 &
printf '%s\n' "$!" > descendant.pid
printf '%s\n' '{"type":"assistant","session_id":"native","message":{"role":"assistant","content":[]}}'
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_harness)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_harness, permissions)?;

        let code = stream_harness_with_program(
            fake_harness.to_str().unwrap(),
            temp_dir.path(),
            Vec::new(),
            false,
            "streamy",
            "ledger-current",
            &mut Vec::new(),
        )?;

        assert_eq!(code, 0);
        let descendant_pid: libc::pid_t = std::fs::read_to_string(descendant_pid_file)?
            .trim()
            .parse()?;
        let deadline = Instant::now() + Duration::from_secs(1);
        while unsafe { libc::kill(descendant_pid, 0) } == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            unsafe { libc::kill(descendant_pid, 0) },
            -1,
            "successful native stream left detached descendant {descendant_pid} alive"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn native_stream_malformed_json_terminates_and_reaps_harness() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir()?;
        let fake_harness = temp_dir.path().join("fake-stream");
        let pid_file = temp_dir.path().join("harness.pid");
        let descendant_pid_file = temp_dir.path().join("descendant.pid");
        // The malformed-stream fixture spawns a long-lived DESCENDANT (a
        // detached `sleep`) before emitting invalid JSON. Gap (2) of the
        // Psyche O1.1 conformance plan (#641): a protocol/JSON failure must
        // reap the entire process tree, not merely the direct child. We record
        // both the direct child's pid ($$) and the descendant's pid ($!) so the
        // assertions below can confirm both are gone (kill(pid,0) == ESRCH).
        std::fs::write(
            &fake_harness,
            r#"#!/bin/sh
printf '%s\n' "$$" > harness.pid
sleep 30 </dev/null >/dev/null 2>&1 &
printf '%s\n' "$!" > descendant.pid
printf '%s\n' 'not-json'
while :; do sleep 1; done
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_harness)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_harness, permissions)?;

        let error = stream_harness_with_program(
            fake_harness.to_str().unwrap(),
            temp_dir.path(),
            Vec::new(),
            false,
            "streamy",
            "ledger-current",
            &mut Vec::new(),
        )
        .expect_err("malformed native JSONL must fail");
        assert!(format!("{error:#}").contains("invalid JSON"));

        for (label, path) in [("harness", pid_file), ("descendant", descendant_pid_file)] {
            let pid: libc::pid_t = std::fs::read_to_string(path)?.trim().parse()?;
            let deadline = Instant::now() + Duration::from_secs(1);
            let mut reaped = false;
            while Instant::now() < deadline {
                if unsafe { libc::kill(pid, 0) } == -1
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    reaped = true;
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            if !reaped {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                    libc::waitpid(pid, std::ptr::null_mut(), 0);
                }
            }
            assert!(reaped, "malformed JSON left {label} {pid} alive");
        }
        Ok(())
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    const NATIVE_STREAM_SIGTERM_TEST_WATCHDOG: Duration = Duration::from_secs(30);
    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    const NATIVE_STREAM_SIGTERM_REAPING_GRACE: Duration = Duration::from_secs(1);

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    const NATIVE_STREAM_SIGTERM_FIXTURES: [(&str, &str); 2] =
        [("harness", "harness.pid"), ("descendant", "descendant.pid")];

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct NativeStreamSigtermFixtureIdentity {
        pid: libc::pid_t,
        started_at: NativeStreamSigtermProcessStart,
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum NativeStreamSigtermProcessStart {
        #[cfg(target_os = "linux")]
        LinuxClockTick(u64),
        #[cfg(target_os = "macos")]
        MacOs { seconds: u64, microseconds: u64 },
    }

    #[cfg(target_os = "macos")]
    #[repr(C)]
    struct NativeStreamSigtermMacOsProcBsdInfo {
        flags: u32,
        status: u32,
        exit_status: u32,
        pid: u32,
        parent_pid: u32,
        uid: libc::uid_t,
        gid: libc::gid_t,
        real_uid: libc::uid_t,
        real_gid: libc::gid_t,
        saved_uid: libc::uid_t,
        saved_gid: libc::gid_t,
        reserved: u32,
        command: [libc::c_char; 16],
        name: [libc::c_char; 32],
        open_files: u32,
        process_group: u32,
        job_control_count: u32,
        controlling_terminal: u32,
        terminal_process_group: u32,
        nice: i32,
        start_seconds: u64,
        start_microseconds: u64,
    }

    #[cfg(target_os = "macos")]
    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: libc::pid_t,
            flavor: libc::c_int,
            arg: u64,
            buffer: *mut libc::c_void,
            buffer_size: libc::c_int,
        ) -> libc::c_int;
    }

    #[cfg(target_os = "linux")]
    fn native_stream_sigterm_process_start(
        pid: libc::pid_t,
    ) -> io::Result<Option<NativeStreamSigtermProcessStart>> {
        let stat_path = PathBuf::from(format!("/proc/{pid}/stat"));
        let stat = match std::fs::read_to_string(&stat_path) {
            Ok(stat) => stat,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let (_, fields) = stat.rsplit_once(") ").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("malformed process stat record at {}", stat_path.display()),
            )
        })?;
        let start_tick = fields
            .split_whitespace()
            // Field 3 begins after the process name; starttime is field 22.
            .nth(19)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("missing process start time at {}", stat_path.display()),
                )
            })?
            .parse()
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "parsing process start time at {}: {error}",
                        stat_path.display()
                    ),
                )
            })?;
        Ok(Some(NativeStreamSigtermProcessStart::LinuxClockTick(
            start_tick,
        )))
    }

    #[cfg(target_os = "macos")]
    fn native_stream_sigterm_process_start(
        pid: libc::pid_t,
    ) -> io::Result<Option<NativeStreamSigtermProcessStart>> {
        const PROC_PIDTBSDINFO: libc::c_int = 3;
        let mut info: NativeStreamSigtermMacOsProcBsdInfo = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is initialized and passed with its exact C layout and size.
        let result = unsafe {
            proc_pidinfo(
                pid,
                PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut NativeStreamSigtermMacOsProcBsdInfo).cast(),
                std::mem::size_of::<NativeStreamSigtermMacOsProcBsdInfo>() as libc::c_int,
            )
        };
        if result == std::mem::size_of::<NativeStreamSigtermMacOsProcBsdInfo>() as libc::c_int {
            return Ok(Some(NativeStreamSigtermProcessStart::MacOs {
                seconds: info.start_seconds,
                microseconds: info.start_microseconds,
            }));
        }
        if result == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(None);
            }
            return Err(error);
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("proc_pidinfo returned {result} bytes for process {pid}"),
        ))
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn native_stream_sigterm_fixture_pid(
        fixture_dir: &Path,
        label: &str,
        file_name: &str,
    ) -> Result<Option<libc::pid_t>> {
        let path = fixture_dir.join(file_name);
        match std::fs::read_to_string(&path) {
            Ok(contents) if contents.trim().is_empty() => Ok(None),
            Ok(contents) => {
                let pid = contents
                    .trim()
                    .parse::<libc::pid_t>()
                    .with_context(|| format!("parsing {label} fixture PID"))?;
                if pid <= 0 {
                    anyhow::bail!("{label} fixture recorded invalid PID {pid}");
                }
                Ok(Some(pid))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => {
                Err(error).with_context(|| format!("reading {label} PID file {}", path.display()))
            }
        }
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn native_stream_sigterm_fixture_identities(
        fixture_dir: &Path,
    ) -> Result<Option<[NativeStreamSigtermFixtureIdentity; 2]>> {
        let Some(harness_pid) = native_stream_sigterm_fixture_pid(
            fixture_dir,
            NATIVE_STREAM_SIGTERM_FIXTURES[0].0,
            NATIVE_STREAM_SIGTERM_FIXTURES[0].1,
        )?
        else {
            return Ok(None);
        };
        let Some(descendant_pid) = native_stream_sigterm_fixture_pid(
            fixture_dir,
            NATIVE_STREAM_SIGTERM_FIXTURES[1].0,
            NATIVE_STREAM_SIGTERM_FIXTURES[1].1,
        )?
        else {
            return Ok(None);
        };
        let Some(harness_started_at) = native_stream_sigterm_process_start(harness_pid)? else {
            return Ok(None);
        };
        let Some(descendant_started_at) = native_stream_sigterm_process_start(descendant_pid)?
        else {
            return Ok(None);
        };

        Ok(Some([
            NativeStreamSigtermFixtureIdentity {
                pid: harness_pid,
                started_at: harness_started_at,
            },
            NativeStreamSigtermFixtureIdentity {
                pid: descendant_pid,
                started_at: descendant_started_at,
            },
        ]))
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum NativeStreamSigtermFixtureState {
        Reaped,
        Alive,
        Reused,
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn native_stream_sigterm_fixture_state(
        identity: &NativeStreamSigtermFixtureIdentity,
    ) -> io::Result<NativeStreamSigtermFixtureState> {
        match native_stream_sigterm_process_start(identity.pid)? {
            None => Ok(NativeStreamSigtermFixtureState::Reaped),
            Some(started_at) if started_at == identity.started_at => {
                Ok(NativeStreamSigtermFixtureState::Alive)
            }
            Some(_) => Ok(NativeStreamSigtermFixtureState::Reused),
        }
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn native_stream_sigterm_fixture_states(
        identities: &[NativeStreamSigtermFixtureIdentity; 2],
    ) -> io::Result<[NativeStreamSigtermFixtureState; 2]> {
        Ok([
            native_stream_sigterm_fixture_state(&identities[0])?,
            native_stream_sigterm_fixture_state(&identities[1])?,
        ])
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn await_native_stream_sigterm_fixture_reaping_grace(
        identities: &[NativeStreamSigtermFixtureIdentity; 2],
    ) -> io::Result<[NativeStreamSigtermFixtureState; 2]> {
        let deadline = Instant::now() + NATIVE_STREAM_SIGTERM_REAPING_GRACE;
        loop {
            match native_stream_sigterm_fixture_states(identities) {
                Ok(states) => {
                    if states
                        .iter()
                        .all(|state| *state == NativeStreamSigtermFixtureState::Reaped)
                        || Instant::now() >= deadline
                    {
                        return Ok(states);
                    }
                }
                Err(error) if Instant::now() >= deadline => return Err(error),
                Err(_) => {}
            }
            // This is containment observation after the direct child has been
            // waited, not a normal-path timing assertion.
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[derive(Clone)]
    struct NativeStreamSigtermCleanupHandle {
        command_path: PathBuf,
        acknowledgement_path: PathBuf,
        token: String,
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    struct NativeStreamSigtermFixtureCleanup {
        command: std::fs::File,
        readiness_path: PathBuf,
        token: String,
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    struct NativeStreamSigtermCleanupSentinel {
        shutdown: Arc<AtomicBool>,
        result: mpsc::Receiver<std::result::Result<(), String>>,
        outcome: Option<std::result::Result<(), String>>,
        outcome_observed: bool,
        reader: Option<thread::JoinHandle<()>>,
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    impl Drop for NativeStreamSigtermCleanupSentinel {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::Relaxed);
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn create_native_stream_sigterm_cleanup_handle(
        fixture_dir: &Path,
    ) -> Result<(
        NativeStreamSigtermCleanupHandle,
        NativeStreamSigtermFixtureCleanup,
    )> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::OpenOptionsExt;

        let command_path = fixture_dir.join("cleanup.fifo");
        let command_path_c = CString::new(command_path.as_os_str().as_bytes())
            .context("encoding native stream fixture cleanup FIFO path")?;
        // SAFETY: the temporary fixture directory is unique and the C string
        // is NUL-terminated for mkfifo.
        if unsafe { libc::mkfifo(command_path_c.as_ptr(), 0o600) } != 0 {
            return Err(io::Error::last_os_error())
                .context("creating native stream fixture cleanup FIFO");
        }
        let marker = format!("coven-native-stream-sigterm-{}", uuid::Uuid::new_v4());
        let fixture_command_path = fixture_dir.join("fixture-cleanup.fifo");
        let fixture_command_path_c = CString::new(fixture_command_path.as_os_str().as_bytes())
            .context("encoding native stream fixture cleanup FIFO path")?;
        // SAFETY: the temporary fixture directory is unique and the C string
        // is NUL-terminated for mkfifo.
        if unsafe { libc::mkfifo(fixture_command_path_c.as_ptr(), 0o600) } != 0 {
            return Err(io::Error::last_os_error())
                .context("creating native stream fixture-group cleanup FIFO");
        }
        // Retain both ends so the fixture's read-only open cannot deadlock
        // before the signaler observes its readiness marker. This endpoint
        // never reads, so only the in-fixture sentinel can consume a command.
        let fixture_command = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fixture_command_path)
            .context("opening native stream fixture-group cleanup FIFO")?;
        Ok((
            NativeStreamSigtermCleanupHandle {
                command_path,
                acknowledgement_path: fixture_dir.join("cleanup.ack"),
                token: format!("coven-native-stream-sigterm-{}", uuid::Uuid::new_v4()),
            },
            NativeStreamSigtermFixtureCleanup {
                command: fixture_command,
                readiness_path: fixture_dir.join("fixture-cleanup.ready"),
                token: marker,
            },
        ))
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn native_stream_sigterm_fixture_sentinel_is_ready(
        readiness_path: &Path,
        token: &str,
    ) -> Result<bool> {
        match std::fs::read_to_string(readiness_path) {
            Ok(readiness) if readiness.trim() == token => Ok(true),
            Ok(readiness) if readiness.trim().is_empty() => Ok(false),
            Ok(_) => anyhow::bail!(
                "native stream fixture-group cleanup sentinel readiness did not match its token"
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "reading native stream fixture-group cleanup sentinel readiness {}",
                    readiness_path.display()
                )
            }),
        }
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn request_native_stream_sigterm_fixture_group_cleanup(
        fixture_cleanup: &mut NativeStreamSigtermFixtureCleanup,
    ) -> Result<()> {
        // This command is consumed only by the fixture's own process-group
        // member, which uses `kill -KILL 0`; no numeric PID is ever signalled.
        writeln!(fixture_cleanup.command, "{}", fixture_cleanup.token)
            .context("writing native stream fixture-group cleanup command")?;
        fixture_cleanup
            .command
            .flush()
            .context("flushing native stream fixture-group cleanup command")
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn start_native_stream_sigterm_cleanup_sentinel(
        cleanup: &NativeStreamSigtermCleanupHandle,
    ) -> Result<NativeStreamSigtermCleanupSentinel> {
        use std::os::unix::fs::OpenOptionsExt;

        // Keep this reader in the test process, outside the harness session
        // which `stream_harness_with_program` intentionally kills. Opening a
        // read-only nonblocking endpoint here also guarantees the command
        // writer below never relies on a FIFO self-reader.
        let command = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&cleanup.command_path)
            .with_context(|| {
                format!(
                    "opening native stream cleanup sentinel FIFO {}",
                    cleanup.command_path.display()
                )
            })?;
        let acknowledgement_path = cleanup.acknowledgement_path.clone();
        let expected_marker = cleanup.token.clone();
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        let shutdown = Arc::new(AtomicBool::new(false));
        let sentinel_shutdown = Arc::clone(&shutdown);
        let reader = thread::spawn(move || {
            let result = (|| -> Result<()> {
                let mut command = command;
                let mut buffer = Vec::new();
                let mut chunk = [0_u8; 256];
                loop {
                    if sentinel_shutdown.load(Ordering::Relaxed) {
                        anyhow::bail!(
                            "native stream cleanup sentinel was stopped before acknowledging its command"
                        );
                    }
                    match command.read(&mut chunk) {
                        Ok(0) => thread::sleep(Duration::from_millis(10)),
                        Ok(read) => {
                            buffer.extend_from_slice(&chunk[..read]);
                            if let Some(line_end) = buffer.iter().position(|byte| *byte == b'\n') {
                                let line: Vec<u8> = buffer.drain(..=line_end).collect();
                                if line[..line.len() - 1] != *expected_marker.as_bytes() {
                                    anyhow::bail!(
                                        "native stream cleanup sentinel received an unexpected command"
                                    );
                                }
                                std::fs::write(
                                    &acknowledgement_path,
                                    format!("{expected_marker}\n"),
                                )
                                .with_context(|| {
                                    format!(
                                        "writing native stream fixture cleanup acknowledgement {}",
                                        acknowledgement_path.display()
                                    )
                                })?;
                                return Ok(());
                            }
                            if buffer.len() > expected_marker.len() {
                                anyhow::bail!(
                                    "native stream cleanup sentinel received an unterminated command"
                                );
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => {
                            return Err(error)
                                .context("reading native stream cleanup sentinel FIFO");
                        }
                    }
                }
            })()
            .map_err(|error| format!("{error:#}"));
            let _ = result_sender.send(result);
        });
        Ok(NativeStreamSigtermCleanupSentinel {
            shutdown,
            result: result_receiver,
            outcome: None,
            outcome_observed: false,
            reader: Some(reader),
        })
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn await_native_stream_sigterm_cleanup_sentinel(
        sentinel: &mut NativeStreamSigtermCleanupSentinel,
        deadline: Instant,
    ) -> Result<()> {
        if sentinel.outcome.is_none() {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_default();
            let outcome = match sentinel.result.recv_timeout(remaining) {
                Ok(outcome) => outcome,
                Err(RecvTimeoutError::Timeout) => {
                    anyhow::bail!(
                        "native stream cleanup sentinel did not acknowledge its durable command before the watchdog expired"
                    );
                }
                Err(RecvTimeoutError::Disconnected) => {
                    anyhow::bail!(
                        "native stream cleanup sentinel stopped before acknowledging its command"
                    );
                }
            };
            sentinel.outcome = Some(outcome);
        }

        sentinel.outcome_observed = true;
        match sentinel
            .outcome
            .as_ref()
            .expect("cleanup sentinel outcome is recorded before observation")
        {
            Ok(()) => Ok(()),
            Err(error) => anyhow::bail!("native stream cleanup sentinel failed: {error}"),
        }
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn finish_native_stream_sigterm_cleanup_sentinel(
        sentinel: &mut NativeStreamSigtermCleanupSentinel,
    ) -> Vec<String> {
        sentinel.shutdown.store(true, Ordering::Relaxed);
        let mut failures = Vec::new();
        let Some(reader) = sentinel.reader.take() else {
            failures.push("native stream cleanup sentinel reader was already joined".into());
            return failures;
        };
        if reader.join().is_err() {
            failures.push("native stream cleanup sentinel reader panicked".into());
            return failures;
        }

        if sentinel.outcome.is_none() {
            match sentinel.result.try_recv() {
                Ok(outcome) => sentinel.outcome = Some(outcome),
                Err(mpsc::TryRecvError::Empty) => failures.push(
                    "native stream cleanup sentinel reader exited without reporting a result"
                        .into(),
                ),
                Err(mpsc::TryRecvError::Disconnected) => failures.push(
                    "native stream cleanup sentinel reader disconnected without reporting a result"
                        .into(),
                ),
            }
        }
        if let Some(Err(error)) = &sentinel.outcome {
            if !sentinel.outcome_observed {
                failures.push(format!("native stream cleanup sentinel failed: {error}"));
            }
        }
        failures
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn request_native_stream_sigterm_fixture_cleanup(
        cleanup: &NativeStreamSigtermCleanupHandle,
        sentinel: &mut NativeStreamSigtermCleanupSentinel,
        deadline: Instant,
    ) -> Result<()> {
        use std::os::unix::fs::OpenOptionsExt;

        // The test-owned sentinel opens its read endpoint before the fixture
        // launches. A write-only descriptor therefore makes delivery
        // unambiguous: this caller can never consume its own command.
        let result = (|| -> Result<()> {
            let mut command = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&cleanup.command_path)
                .with_context(|| {
                    format!(
                        "opening native stream fixture cleanup FIFO {}",
                        cleanup.command_path.display()
                    )
                })?;
            writeln!(command, "{}", cleanup.token)
                .context("writing native stream fixture cleanup command")?;
            command
                .flush()
                .context("flushing native stream fixture cleanup command")?;
            // The reader now has the complete token; close the only test-side
            // writer before waiting for its acknowledgement.
            drop(command);
            await_native_stream_sigterm_cleanup_sentinel(sentinel, deadline)?;

            match std::fs::read_to_string(&cleanup.acknowledgement_path) {
                Ok(acknowledgement) if acknowledgement.trim() == cleanup.token => Ok(()),
                Ok(_) => anyhow::bail!(
                    "native stream fixture cleanup acknowledgement did not match its token"
                ),
                Err(error) => Err(error).with_context(|| {
                    format!(
                        "reading native stream fixture cleanup acknowledgement {}",
                        cleanup.acknowledgement_path.display()
                    )
                }),
            }
        })();
        if result.is_err() {
            sentinel.shutdown.store(true, Ordering::Relaxed);
        }
        result
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn cleanup_native_stream_sigterm_fixtures(
        cleanup: &NativeStreamSigtermCleanupHandle,
        sentinel: &mut NativeStreamSigtermCleanupSentinel,
        fixture_cleanup: &mut NativeStreamSigtermFixtureCleanup,
        identities: Option<&[NativeStreamSigtermFixtureIdentity; 2]>,
    ) -> Vec<String> {
        let mut failures = Vec::new();
        if let Err(error) = request_native_stream_sigterm_fixture_cleanup(
            cleanup,
            sentinel,
            Instant::now() + NATIVE_STREAM_SIGTERM_TEST_WATCHDOG,
        ) {
            failures.push(format!(
                "failed to request durable native stream fixture cleanup: {error:#}"
            ));
        }

        let Some(identities) = identities else {
            failures.push(
                "fixture identities were never captured before emergency cleanup; refusing to report PID-file absence as reaping"
                    .into(),
            );
            if let Err(error) = request_native_stream_sigterm_fixture_group_cleanup(fixture_cleanup)
            {
                failures.push(format!(
                    "failed to request fixture-local native stream process-group cleanup: {error:#}"
                ));
            }
            return failures;
        };

        let states_after_reaping_grace = match await_native_stream_sigterm_fixture_reaping_grace(
            identities,
        ) {
            Ok(states)
                if states
                    .iter()
                    .all(|state| *state == NativeStreamSigtermFixtureState::Reaped) =>
            {
                return failures;
            }
            Ok(states) => {
                // This is the production assertion. The emergency FIFO can
                // only contain a leak; it must never turn one into a pass.
                for (index, state) in states.iter().enumerate() {
                    match state {
                        NativeStreamSigtermFixtureState::Reaped => {}
                        NativeStreamSigtermFixtureState::Alive => failures.push(format!(
                            "SIGTERM production reaping check failed after the reaping grace: {} {} was still alive",
                            NATIVE_STREAM_SIGTERM_FIXTURES[index].0, identities[index].pid
                        )),
                        NativeStreamSigtermFixtureState::Reused => failures.push(format!(
                            "SIGTERM production reaping check failed after the reaping grace: recorded {} PID {} was reused",
                            NATIVE_STREAM_SIGTERM_FIXTURES[index].0, identities[index].pid
                        )),
                    }
                }
                Some(states)
            }
            Err(error) => {
                failures.push(format!(
                    "could not verify native stream fixture identities through the reaping grace before fixture-local emergency cleanup: {error}"
                ));
                None
            }
        };

        if let Err(error) = request_native_stream_sigterm_fixture_group_cleanup(fixture_cleanup) {
            failures.push(format!(
                "failed to request fixture-local native stream process-group cleanup: {error:#}"
            ));
        }

        let cleanup_deadline = Instant::now() + NATIVE_STREAM_SIGTERM_TEST_WATCHDOG;
        loop {
            match native_stream_sigterm_fixture_states(identities) {
                Ok(states)
                    if states
                        .iter()
                        .all(|state| *state == NativeStreamSigtermFixtureState::Reaped) =>
                {
                    match states_after_reaping_grace {
                        Some(states_after_reaping_grace) => {
                            let initial_context = states_after_reaping_grace
                                .iter()
                                .enumerate()
                                .map(|(index, state)| {
                                    format!(
                                        "{} {} was {state:?}",
                                        NATIVE_STREAM_SIGTERM_FIXTURES[index].0,
                                        identities[index].pid
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            failures.push(format!(
                                "fixture-local emergency cleanup reaped the recorded identities after the failed production check ({initial_context}); post-cleanup inspection does not erase that failure"
                            ));
                        }
                        None => failures.push(
                            "fixture-local emergency cleanup reaped the recorded identities, but the pre-cleanup identity inspection failed; post-cleanup reaping does not replace that production proof"
                                .into(),
                        ),
                    }
                    return failures;
                }
                Ok(states) if Instant::now() >= cleanup_deadline => {
                    for (index, state) in states.iter().enumerate() {
                        match state {
                            NativeStreamSigtermFixtureState::Reaped => {}
                            NativeStreamSigtermFixtureState::Alive => failures.push(format!(
                                "cancelled native stream left {} {} alive after durable cleanup",
                                NATIVE_STREAM_SIGTERM_FIXTURES[index].0, identities[index].pid
                            )),
                            NativeStreamSigtermFixtureState::Reused => failures.push(format!(
                                "recorded {} PID {} was reused; refusing to report that unrelated process as fixture reaping",
                                NATIVE_STREAM_SIGTERM_FIXTURES[index].0, identities[index].pid
                            )),
                        }
                    }
                    return failures;
                }
                Ok(_) => thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    failures.push(format!(
                        "could not verify native stream fixture identities after fixture-local emergency cleanup: {error}"
                    ));
                    return failures;
                }
            }
        }
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn native_stream_sigterm_handler_is_restored() -> Result<bool> {
        let _lock = SUPERVISED_STREAM_CANCELLATION_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut current: libc::sigaction = unsafe { std::mem::zeroed() };
        if unsafe { libc::sigaction(libc::SIGTERM, std::ptr::null(), &mut current) } != 0 {
            return Err(std::io::Error::last_os_error())
                .context("reading restored native stream SIGTERM handler");
        }
        Ok(current.sa_sigaction != cancel_supervised_stream as *const () as usize)
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn native_stream_sigterm_cancels_and_reaps_process_tree() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir()?;
        let fake_harness = temp_dir.path().join("long-lived-stream");
        let (cleanup, mut fixture_cleanup) =
            create_native_stream_sigterm_cleanup_handle(temp_dir.path())?;
        std::fs::write(
            &fake_harness,
            format!(
                r#"#!/bin/sh
(
  exec 3< fixture-cleanup.fifo
  printf '%s\n' "{cleanup_token}" > fixture-cleanup.ready
  while IFS= read -r cleanup_command <&3; do
    if [ "$cleanup_command" = "{cleanup_token}" ]; then
      kill -KILL 0
    fi
  done
) &
printf '%s\n' "$$" > harness.pid
sleep 120 </dev/null >/dev/null 2>&1 &
printf '%s\n' "$!" > descendant.pid
while :; do sleep 1; done
"#,
                cleanup_token = fixture_cleanup.token
            ),
        )?;
        let mut permissions = std::fs::metadata(&fake_harness)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_harness, permissions)?;

        let lifecycle = Arc::new(SupervisedStreamCancellationTestObserver::arm()?);
        let mut cleanup_sentinel = start_native_stream_sigterm_cleanup_sentinel(&cleanup)?;
        let signal_lifecycle = Arc::clone(&lifecycle);
        let runner_thread = unsafe { libc::pthread_self() } as usize;
        let signal_dir = temp_dir.path().to_path_buf();
        let stream_finished = Arc::new(AtomicBool::new(false));
        let signal_stream_finished = Arc::clone(&stream_finished);
        let fixture_identities = Arc::new(Mutex::new(None));
        let signal_fixture_identities = Arc::clone(&fixture_identities);
        let signal_fixture_cleanup = fixture_cleanup.readiness_path.clone();
        let signal_cleanup_token = fixture_cleanup.token.clone();
        let signaler = thread::spawn(move || -> Result<()> {
            let startup_deadline = Instant::now() + NATIVE_STREAM_SIGTERM_TEST_WATCHDOG;
            let mut last_readiness_error = None;
            let mut last_delivery_error = None;
            let mut startup_failure = None;
            loop {
                if signal_stream_finished.load(Ordering::Relaxed) {
                    anyhow::bail!(
                        "{}native stream returned before the fixture and cancellation handler were both ready; no SIGTERM was sent",
                        startup_failure
                            .as_deref()
                            .map(|failure| format!("{failure}; "))
                            .unwrap_or_default(),
                    );
                }

                let fixture_identities_ready =
                    match native_stream_sigterm_fixture_identities(&signal_dir) {
                        Ok(Some(identities)) => {
                            *signal_fixture_identities
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                Some(identities);
                            true
                        }
                        Ok(None) => false,
                        Err(error) => {
                            last_readiness_error = Some(format!("{error:#}"));
                            false
                        }
                    };

                let fixture_sentinel_ready = match native_stream_sigterm_fixture_sentinel_is_ready(
                    &signal_fixture_cleanup,
                    &signal_cleanup_token,
                ) {
                    Ok(ready) => ready,
                    Err(error) => {
                        last_readiness_error = Some(format!("{error:#}"));
                        false
                    }
                };

                if fixture_identities_ready && fixture_sentinel_ready {
                    match signal_lifecycle.send_sigterm_if_guarded(runner_thread) {
                        // Dispatch begins only after
                        // `stream_harness_with_program` has physically
                        // unmasked its installed guard.
                        Ok(SupervisedStreamCancellationTestPhase::ActiveUnmasked) => {
                            if let Some(failure) = startup_failure {
                                anyhow::bail!(
                                    "{failure}; lifecycle-verified SIGTERM was delivered to contain the stalled fixture startup"
                                );
                            }
                            return Ok(());
                        }
                        Ok(
                            SupervisedStreamCancellationTestPhase::Registered
                            | SupervisedStreamCancellationTestPhase::InstalledMasked
                            | SupervisedStreamCancellationTestPhase::Restoring,
                        ) => {
                            last_delivery_error = Some(
                                "fixture was ready before the cancellation guard was physically active; waiting for guarded delivery"
                                    .into(),
                            );
                        }
                        Ok(SupervisedStreamCancellationTestPhase::Finished) => {
                            anyhow::bail!(
                                "native stream reached the finished cancellation lifecycle before SIGTERM delivery"
                            );
                        }
                        Err(error) => last_delivery_error = Some(format!("{error:#}")),
                    }
                }
                if signal_stream_finished.load(Ordering::Relaxed) {
                    anyhow::bail!(
                        "{}native stream returned before the fixture and cancellation handler were both ready; no SIGTERM was sent",
                        startup_failure
                            .as_deref()
                            .map(|failure| format!("{failure}; "))
                            .unwrap_or_default(),
                    );
                }
                if Instant::now() >= startup_deadline && startup_failure.is_none() {
                    let failure = format!(
                        "native stream fixture-start failure: the fixture, cleanup sentinel, and cancellation handler did not become ready together before the startup watchdog expired{}{}",
                        last_readiness_error
                            .as_deref()
                            .map(|error| format!("; last readiness error: {error}"))
                            .unwrap_or_default(),
                        last_delivery_error
                            .as_deref()
                            .map(|error| format!("; last delivery error: {error}"))
                            .unwrap_or_default(),
                    );
                    // The lifecycle mutex stays held through pthread_kill.
                    // Before actual unmasking, retain the signaler instead of
                    // queueing a signal that could reach a restored handler.
                    match signal_lifecycle.send_sigterm_if_guarded(runner_thread) {
                        Ok(SupervisedStreamCancellationTestPhase::ActiveUnmasked) => {
                            anyhow::bail!(
                                "{failure}; lifecycle-verified SIGTERM was delivered to contain the stalled fixture startup"
                            );
                        }
                        Ok(
                            SupervisedStreamCancellationTestPhase::Registered
                            | SupervisedStreamCancellationTestPhase::InstalledMasked,
                        ) => {
                            startup_failure = Some(format!(
                                "{failure}; guard is not yet physically active, so SIGTERM is deferred until its disposition is safe"
                            ));
                        }
                        Ok(SupervisedStreamCancellationTestPhase::Restoring) => {
                            anyhow::bail!(
                                "{failure}; guard began restoration, so SIGTERM was not sent under a restored disposition"
                            );
                        }
                        Ok(SupervisedStreamCancellationTestPhase::Finished) => {
                            anyhow::bail!(
                                "{failure}; guard was already finished, so SIGTERM was not sent under a restored disposition"
                            );
                        }
                        Err(error) => {
                            anyhow::bail!(
                                "{failure}; lifecycle-verified SIGTERM delivery failed: {error:#}"
                            );
                        }
                    }
                }
                thread::sleep(Duration::from_millis(10));
            }
        });

        let stream_result = stream_harness_with_program(
            fake_harness.to_str().expect("fixture path is UTF-8"),
            temp_dir.path(),
            Vec::new(),
            false,
            "streamy",
            "ledger-current",
            &mut Vec::new(),
        );
        stream_finished.store(true, Ordering::Relaxed);
        let signal_result = match signaler.join() {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!("native stream signal thread panicked")),
        };
        let restoration_result = native_stream_sigterm_handler_is_restored();

        let captured_fixture_identities = fixture_identities
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let mut failures = cleanup_native_stream_sigterm_fixtures(
            &cleanup,
            &mut cleanup_sentinel,
            &mut fixture_cleanup,
            captured_fixture_identities.as_ref(),
        );
        failures.extend(finish_native_stream_sigterm_cleanup_sentinel(
            &mut cleanup_sentinel,
        ));
        if let Err(error) = signal_result {
            failures.push(format!("native stream signal thread failed: {error:#}"));
        }
        match stream_result {
            Ok(code) => failures.push(format!(
                "SIGTERM must cancel a native stream, but it exited successfully with code {code}"
            )),
            Err(error)
                if !format!("{error:#}").contains("streamy native stream cancelled by SIGTERM") =>
            {
                failures.push(format!("unexpected cancellation error: {error:#}"));
            }
            Err(_) => {}
        }
        match restoration_result {
            Ok(true) => {}
            Ok(false) => failures
                .push("native stream runner did not restore the previous SIGTERM handler".into()),
            Err(error) => failures.push(format!("{error:#}")),
        }
        drop(lifecycle);
        if !failures.is_empty() {
            anyhow::bail!("{}", failures.join("; "));
        }
        Ok(())
    }

    #[test]
    fn stream_passthrough_args_drop_stdin_format_for_one_shot_prompt() {
        let args = vec![
            "--print".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
        ];

        assert_eq!(
            stream_passthrough_args(args.clone(), false),
            vec![
                "--print".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
            ]
        );
        assert_eq!(stream_passthrough_args(args.clone(), true), args);
    }

    #[cfg(unix)]
    #[test]
    fn stream_harness_claude_includes_input_format_when_forwarding_stdin() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_claude = temp_dir.path().join("fake-claude");
        std::fs::write(
            &fake_claude,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_claude)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_claude, permissions)?;

        let mut out = Vec::new();
        let _code = stream_harness_with_claude_args(
            fake_claude.to_str().unwrap(),
            temp_dir.path(),
            "session-456",
            false,
            "hello prompt",
            // forward_stdin=true → long-lived chat mode where claude reads
            // user messages as JSONL on stdin, so --input-format stream-json
            // MUST be present.
            true,
            None,
            crate::harness::HarnessLaunchOptions::default(),
            &mut out,
        )?;

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "-p\n--input-format\nstream-json\n--output-format\nstream-json\n--verbose\n--session-id\nsession-456\n--\nhello prompt\n"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stream_harness_claude_honors_permission_bypass_opt_in() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_claude = temp_dir.path().join("fake-claude");
        std::fs::write(
            &fake_claude,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_claude)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_claude, permissions)?;

        let mut out = Vec::new();
        let _code = stream_harness_with_claude_args_and_permission_bypass(
            fake_claude.to_str().unwrap(),
            temp_dir.path(),
            "session-456",
            false,
            "hello prompt",
            false,
            None,
            crate::harness::HarnessLaunchOptions::default(),
            true,
            &mut out,
        )?;

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "-p\n--permission-mode\nbypassPermissions\n--output-format\nstream-json\n--verbose\n--session-id\nsession-456\n--\nhello prompt\n"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stream_harness_claude_resumes_with_resume_flag_not_session_id() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_claude = temp_dir.path().join("fake-claude");
        std::fs::write(
            &fake_claude,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_claude)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_claude, permissions)?;

        let mut out = Vec::new();
        let _code = stream_harness_with_claude_args(
            fake_claude.to_str().unwrap(),
            temp_dir.path(),
            "session-789",
            // is_resume=true → the session already exists. `--session-id`
            // only creates sessions and fails with "Session ID <id> is
            // already in use" on reuse, so resumed turns MUST go through
            // `--resume` or every `coven run --continue` loses the chat.
            true,
            "hello again",
            false,
            None,
            crate::harness::HarnessLaunchOptions::default(),
            &mut out,
        )?;

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "-p\n--output-format\nstream-json\n--verbose\n--resume\nsession-789\n--\nhello again\n"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stream_harness_claude_forwards_model_with_prefix_stripped() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_claude = temp_dir.path().join("fake-claude");
        std::fs::write(
            &fake_claude,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_claude)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_claude, permissions)?;

        let mut out = Vec::new();
        let _code = stream_harness_with_claude_args(
            fake_claude.to_str().unwrap(),
            temp_dir.path(),
            "session-123",
            false,
            "hello prompt",
            false,
            None,
            // Claude declares strip_provider, so only its bare model is forwarded.
            crate::harness::HarnessLaunchOptions {
                model: Some("anthropic/claude-sonnet-4"),
                ..Default::default()
            },
            &mut out,
        )?;

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "-p\n--model\nclaude-sonnet-4\n--output-format\nstream-json\n--verbose\n--session-id\nsession-123\n--\nhello prompt\n"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stream_harness_claude_forwards_think_as_effort_high() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_claude = temp_dir.path().join("fake-claude");
        std::fs::write(
            &fake_claude,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_claude)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_claude, permissions)?;

        let mut out = Vec::new();
        let _code = stream_harness_with_claude_args(
            fake_claude.to_str().unwrap(),
            temp_dir.path(),
            "session-123",
            false,
            "hello prompt",
            false,
            None,
            crate::harness::HarnessLaunchOptions {
                think: true,
                ..Default::default()
            },
            &mut out,
        )?;

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "-p\n--effort\nhigh\n--output-format\nstream-json\n--verbose\n--session-id\nsession-123\n--\nhello prompt\n"
        );
        Ok(())
    }

    #[test]
    fn detached_output_drain_invokes_callback_for_bytes() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_for_callback = captured.clone();
        let mut callback: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            captured_for_callback
                .lock()
                .unwrap()
                .extend_from_slice(&chunk);
        });
        let mut reader: &[u8] = b"hello coven";

        drain_detached_output(&mut reader, Some(&mut callback));

        assert_eq!(captured.lock().unwrap().as_slice(), b"hello coven");
    }

    /// `Read` adapter that yields a fixed sequence of byte slices, one per
    /// `read` call, then EOF. Lets us drive `drain_detached_output` with
    /// the same chunk boundaries the kernel would produce when a
    /// multi-byte UTF-8 codepoint straddles two reads.
    struct ChunkedReader<'a> {
        chunks: std::collections::VecDeque<&'a [u8]>,
    }

    impl<'a> Read for ChunkedReader<'a> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.chunks.pop_front() {
                Some(chunk) => {
                    let n = chunk.len().min(buf.len());
                    buf[..n].copy_from_slice(&chunk[..n]);
                    if n < chunk.len() {
                        self.chunks.push_front(&chunk[n..]);
                    }
                    Ok(n)
                }
                None => Ok(0),
            }
        }
    }

    #[test]
    fn drain_detached_output_reassembles_codepoint_split_across_reads() {
        // 🎉 = F0 9F 8E 89. Split across two reads so the first ends
        // mid-codepoint. The drainer must hold the trailing bytes back
        // until the continuation arrives instead of lossy-decoding to
        // U+FFFD.
        let emoji = "🎉".as_bytes();
        let (head, tail) = emoji.split_at(2);
        let mut reader = ChunkedReader {
            chunks: vec![head, tail].into(),
        };

        let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let captured_for_cb = captured.clone();
        let mut callback: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            captured_for_cb
                .lock()
                .unwrap()
                .push_str(std::str::from_utf8(&chunk).expect(
                    "drain_detached_output must only emit chunks that are themselves valid UTF-8",
                ));
        });

        drain_detached_output(&mut reader, Some(&mut callback));

        assert_eq!(
            captured.lock().unwrap().as_str(),
            "🎉",
            "split codepoint must round-trip; the drain owns per-call buffer state"
        );
    }

    #[test]
    fn attached_output_mirrors_raw_bytes_and_reassembles_observed_utf8() -> anyhow::Result<()> {
        let emoji = "🎉".as_bytes();
        let (head, tail) = emoji.split_at(2);
        let mut reader = ChunkedReader {
            chunks: vec![head, tail].into(),
        };
        let mut mirrored = Vec::new();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_observer = Arc::clone(&captured);
        let observer: AttachedOutputObserver = Box::new(move |chunk| {
            captured_for_observer.lock().unwrap().push(chunk);
            Ok(())
        });

        copy_attached_output(&mut reader, &mut mirrored, Some(observer))?;

        assert_eq!(mirrored, emoji);
        let captured = captured.lock().unwrap();
        assert_eq!(captured.as_slice(), [emoji]);
        assert!(captured
            .iter()
            .all(|chunk| std::str::from_utf8(chunk).is_ok()));
        Ok(())
    }

    #[test]
    fn attached_output_drains_after_observer_failure_and_returns_the_error() {
        let mut reader = ChunkedReader {
            chunks: vec![&b"first"[..], &b"second"[..]].into(),
        };
        let mut mirrored = Vec::new();
        let observer: AttachedOutputObserver =
            Box::new(|_| anyhow::bail!("simulated ledger failure"));

        let error = copy_attached_output(&mut reader, &mut mirrored, Some(observer))
            .expect_err("observer failure must surface");

        assert_eq!(mirrored, b"firstsecond");
        assert!(error.to_string().contains("simulated ledger failure"));
    }

    #[test]
    fn drain_detached_output_flushes_trailing_partial_codepoint_on_eof() {
        // A read that delivers only the first 2 bytes of a 4-byte
        // codepoint and then closes. The buffered tail can never
        // complete, but it shouldn't silently disappear either — flush
        // it through `from_utf8_lossy` so the user sees something.
        let half = &"🎉".as_bytes()[..2];
        let mut reader = ChunkedReader {
            chunks: vec![half].into(),
        };
        let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let captured_for_cb = captured.clone();
        let mut callback: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            captured_for_cb
                .lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&chunk));
        });

        drain_detached_output(&mut reader, Some(&mut callback));

        let final_text = captured.lock().unwrap().clone();
        assert!(
            !final_text.is_empty(),
            "EOF with a partial codepoint must flush, not drop the bytes"
        );
        assert!(
            final_text.contains('\u{FFFD}'),
            "the flushed bytes are unrecoverable; expected U+FFFD replacement, got: {final_text:?}"
        );
    }

    #[test]
    fn detached_pty_answers_split_vt_queries_without_leaking_them() {
        let emoji = "🎉".as_bytes();
        let chunks: Vec<&[u8]> = vec![
            b"ready ",
            b"\x1b[",
            b"6n",
            &emoji[..2],
            &emoji[2..],
            b"\x1b[c\x1b[0",
            b"c\x1b[5n done",
        ];
        let mut reader = ChunkedReader {
            chunks: chunks.into(),
        };
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_for_callback = captured.clone();
        let mut callback: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            captured_for_callback
                .lock()
                .unwrap()
                .extend_from_slice(&chunk);
        });
        let mut replies = Vec::new();
        let mut terminal_reply = |reply: &'static [u8]| replies.extend_from_slice(reply);

        drain_detached_pty_output(&mut reader, &mut terminal_reply, Some(&mut callback));

        assert_eq!(
            captured.lock().unwrap().as_slice(),
            "ready 🎉 done".as_bytes()
        );
        assert_eq!(
            replies, b"\x1b[1;1R\x1b[?62;c\x1b[?62;c\x1b[0n",
            "CPR, primary/explicit DA, and status queries must receive terminal replies"
        );
    }

    #[test]
    fn detached_pty_preserves_unknown_and_incomplete_escape_sequences() {
        let mut reader = ChunkedReader {
            chunks: vec![b"before\x1b[31mred\x1b[".as_slice()].into(),
        };
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_for_callback = captured.clone();
        let mut callback: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            captured_for_callback
                .lock()
                .unwrap()
                .extend_from_slice(&chunk);
        });
        let mut replies = Vec::new();
        let mut terminal_reply = |reply: &'static [u8]| replies.extend_from_slice(reply);

        drain_detached_pty_output(&mut reader, &mut terminal_reply, Some(&mut callback));

        assert_eq!(
            captured.lock().unwrap().as_slice(),
            b"before\x1b[31mred\x1b["
        );
        assert!(replies.is_empty());
    }

    #[test]
    fn detached_pty_keeps_draining_when_terminal_reply_queue_is_full() {
        let mut reader = ChunkedReader {
            chunks: vec![b"before\x1b[6nafter".as_slice()].into(),
        };
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_callback = Arc::clone(&captured);
        let mut callback: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            captured_for_callback.lock().unwrap().extend(chunk);
        });
        let (sender, _receiver) = mpsc::sync_channel(1);
        let writer = SharedPtyWriter { sender };
        assert!(writer
            .sender
            .try_send(PtyWriteRequest::Write {
                bytes: b"queue already full".to_vec(),
                flush: true,
                completion: None,
            })
            .is_ok());
        let mut terminal_reply = |reply| writer.queue_terminal_reply(reply);

        drain_detached_pty_output(&mut reader, &mut terminal_reply, Some(&mut callback));

        assert_eq!(captured.lock().unwrap().as_slice(), b"beforeafter");
    }

    #[test]
    fn terminal_replies_share_one_fifo_writer_path() -> anyhow::Result<()> {
        struct RecordingWriter(Arc<Mutex<Vec<u8>>>);

        impl Write for RecordingWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let recorded = Arc::new(Mutex::new(Vec::new()));
        let mut writer = spawn_shared_pty_writer(Box::new(RecordingWriter(Arc::clone(&recorded))));
        writer.queue_terminal_reply(b"reply-a");
        writer.queue_terminal_reply(b"reply-b");
        writer.flush()?;

        assert_eq!(recorded.lock().unwrap().as_slice(), b"reply-areply-b");
        Ok(())
    }

    #[test]
    fn startup_detector_ignores_terminal_control_traffic_across_chunks() {
        let mut detector = MeaningfulOutputDetector::default();
        assert!(!detector.push(b"\x1b[?1004"));
        assert!(!detector.push(b"\x1b[?25\x1b[?1004h\x1b]0;terminal title"));
        assert!(!detector.push(b"\x1b\\\x1b("));
        assert!(!detector.push(b"B\x1b#8   \r\n\t"));
        assert!(detector.push(b"\x1b[32mready"));
    }

    #[test]
    fn builds_claude_command_without_shell_interpolation() {
        let cwd = Path::new("/tmp/coven-project");
        let command = build_harness_command(
            "claude",
            "explain && exit",
            cwd,
            crate::harness::HarnessLaunchMode::Interactive,
        )
        .unwrap();

        assert_eq!(command.program(), "claude");
        assert_eq!(command.args(), &["--", "explain && exit"]);
        assert_eq!(command.cwd(), cwd);
    }

    #[test]
    fn codex_noninteractive_prompt_uses_stdin_when_requested() {
        let prompt = "first line\nsecond & line with %PATH%";
        let mut args = vec![
            "exec".to_string(),
            "--model".to_string(),
            "gpt-5.5".to_string(),
            "--".to_string(),
            prompt.to_string(),
        ];

        let stdin_prompt = move_codex_prompt_to_stdin(
            "codex",
            crate::harness::HarnessLaunchMode::NonInteractive,
            prompt,
            &mut args,
            true,
        );

        assert_eq!(args.last().map(String::as_str), Some("-"));
        assert_eq!(stdin_prompt.as_deref(), Some(prompt.as_bytes()));
    }

    #[test]
    fn codex_top_level_error_message_is_preserved() -> anyhow::Result<()> {
        let mut state = CodexJsonState::default();
        let mut assistant = Vec::new();

        let valid = handle_codex_json_line(
            r#"{"type":"error","message":"request rejected by Codex"}"#,
            &mut state,
            &mut |text| {
                assistant.push(text.to_string());
                Ok(())
            },
        )?;

        assert!(valid);
        assert_eq!(
            state.protocol_error.as_deref(),
            Some("request rejected by Codex")
        );
        assert!(assistant.is_empty());
        Ok(())
    }

    #[test]
    fn piped_codex_stdin_prompt_keeps_identity_unicode_and_large_input() -> anyhow::Result<()> {
        let familiar = crate::harness::FamiliarContext {
            id: "codex-local".to_string(),
            display_name: "Codex Local".to_string(),
            role: None,
        };
        let prompt = format!("diagnose the failure 🔮\n{}", "evidence ".repeat(12_000));
        let command = build_piped_harness_command_with_conversation(
            "codex",
            &prompt,
            Path::new("/project"),
            crate::harness::HarnessLaunchMode::NonInteractive,
            None,
            Some(&familiar),
            crate::harness::HarnessLaunchOptions::default(),
        )?;

        let prompt = String::from_utf8(command.stdin_prompt.expect("prompt should use stdin"))?;
        assert!(prompt.starts_with(&familiar.identity_preamble()));
        assert!(prompt.ends_with("evidence "));
        assert!(prompt.contains("diagnose the failure 🔮"));
        assert!(
            prompt.len() > 100_000,
            "fixture exceeds ordinary argv safety margins"
        );
        assert_eq!(command.args.last().map(String::as_str), Some("-"));
        Ok(())
    }

    #[test]
    fn stdin_prompt_transport_is_not_used_for_other_launches() {
        let prompt = "hello";
        for (harness, mode) in [
            ("claude", crate::harness::HarnessLaunchMode::NonInteractive),
            ("codex", crate::harness::HarnessLaunchMode::Interactive),
        ] {
            let mut args = vec!["--".to_string(), prompt.to_string()];
            let stdin_prompt = move_codex_prompt_to_stdin(harness, mode, prompt, &mut args, true);
            assert!(stdin_prompt.is_none());
            assert_eq!(args.last().map(String::as_str), Some(prompt));
        }
    }

    #[cfg(windows)]
    #[test]
    fn captured_piped_batch_receives_multiline_prompt_via_stdin() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let batch = temp_dir.path().join("fake-codex.cmd");
        std::fs::write(
            &batch,
            "@echo off\r\nset /p prompt=\r\n>&2 echo %prompt%\r\nexit /b 0\r\n",
        )?;
        let command = HarnessCommand {
            program: batch.to_string_lossy().into_owned(),
            args: vec!["exec".to_string(), "--".to_string(), "-".to_string()],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: Some(b"hello from stdin\nsecond & unsafe-looking line".to_vec()),
            env_overrides: Vec::new(),
        };
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let callback_output = captured.clone();

        let result = run_piped_attached_captured(
            &command,
            Box::new(move |chunk| {
                callback_output.lock().unwrap().extend_from_slice(&chunk);
            }),
        )?;

        assert_eq!(result.status, "completed");
        assert_eq!(result.exit_code, Some(0));
        assert!(String::from_utf8(captured.lock().unwrap().clone())?.contains("hello from stdin"));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn codex_json_batch_shim_uses_stdin_and_emits_assistant_text() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let batch = temp_dir.path().join("fake-codex.cmd");
        // Copy stdin with findstr (a native, always-present binary with no
        // cold start): PowerShell's multi-second startup on loaded runners
        // outlived even a 10s activity deadline and flaked this test twice
        // (issue #407).
        std::fs::write(
            &batch,
            concat!(
                "@echo off\r\n",
                "\"%SystemRoot%\\System32\\findstr.exe\" \"^\" > stdin.txt\r\n",
                "echo %* > args.txt\r\n",
                "echo {\"type\":\"thread.started\",\"thread_id\":\"thread-456\"}\r\n",
                "echo {\"type\":\"item.completed\",\"item\":{\"id\":\"item-1\",\"type\":\"agent_message\",\"text\":\"reply from Codex\"}}\r\n",
                "echo {\"type\":\"turn.completed\"}\r\n",
                "exit /b 0\r\n"
            ),
        )?;
        let command = HarnessCommand {
            program: batch.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--".to_string(),
                "-".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: Some(b"first line\nsecond line\n".to_vec()),
            env_overrides: Vec::new(),
        };
        let mut assistant = Vec::new();

        // Generous headroom for shim process startup on a loaded Windows
        // runner (issue #407). This test exercises stdin and JSONL framing,
        // not the activity deadline.
        let outcome = stream_codex_json_with_timeout(&command, Duration::from_secs(10), |text| {
            assistant.push(text.to_string());
            Ok(())
        })?;

        let args = read_side_effect_file(temp_dir.path().join("args.txt"))?;
        assert!(
            args.contains("exec --json -- -"),
            "unexpected argv: {args:?}"
        );
        assert!(
            !args.contains("first line") && !args.contains("second line"),
            "the multiline user prompt must not reach cmd.exe argv: {args:?}"
        );
        let stdin = read_side_effect_file(temp_dir.path().join("stdin.txt"))?;
        assert!(
            stdin.contains("first line"),
            "missing first stdin line: {stdin:?}"
        );
        assert!(
            stdin.contains("second line"),
            "missing second stdin line: {stdin:?}"
        );
        assert_eq!(assistant, vec!["reply from Codex"]);
        assert_eq!(outcome.harness_session_id.as_deref(), Some("thread-456"));
        assert!(outcome.error.is_none());
        Ok(())
    }

    #[cfg(windows)]
    fn read_side_effect_file(path: PathBuf) -> anyhow::Result<String> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut last_error = None;
        while Instant::now() < deadline {
            match std::fs::read_to_string(&path) {
                Ok(contents) => return Ok(contents),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    last_error = Some(error);
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("failed reading {path:?}"))
                }
            }
        }
        match last_error {
            Some(error) => Err(error)
                .with_context(|| format!("timed out waiting for batch side-effect file {path:?}")),
            None => anyhow::bail!("timed out waiting for batch side-effect file {path:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn codex_json_batch_shim_times_out_while_large_prompt_is_still_writing() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let batch = temp_dir.path().join("silent-codex.cmd");
        std::fs::write(&batch, "@echo off\r\n:spin\r\ngoto spin\r\n")?;
        let command = HarnessCommand {
            program: batch.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--".to_string(),
                "-".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            // The shim deliberately never reads stdin. This payload exceeds
            // the anonymous-pipe buffer, proving the activity deadline also
            // covers a blocked prompt writer rather than only stdout reads.
            stdin_prompt: Some(vec![b'x'; 1024 * 1024]),
            env_overrides: Vec::new(),
        };

        let started = Instant::now();
        let outcome =
            stream_codex_json_with_timeout(&command, Duration::from_millis(50), |_| Ok(()))?;

        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("terminated")));
        Ok(())
    }
}
