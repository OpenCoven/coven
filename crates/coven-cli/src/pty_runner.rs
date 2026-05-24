use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;

use anyhow::{Context, Result};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, PtySize, PtySystem};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessCommand {
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyRunResult {
    pub status: &'static str,
    pub exit_code: Option<i32>,
}

pub struct DetachedPtySession {
    pub input: Box<dyn Write + Send>,
    pub killer: Box<dyn ChildKiller + Send + Sync>,
}

pub struct DetachedPtyObserver {
    pub on_output: Box<dyn FnMut(Vec<u8>) + Send + 'static>,
    pub on_exit: Box<dyn FnOnce(PtyRunResult) + Send + 'static>,
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

    fn to_command_builder(&self) -> CommandBuilder {
        let mut builder = CommandBuilder::new(&self.program);
        builder.args(&self.args);
        builder.cwd(self.cwd.as_os_str());
        builder
    }
}

pub fn build_harness_command(
    harness_id: &str,
    prompt: &str,
    cwd: &Path,
    mode: crate::harness::HarnessLaunchMode,
) -> Result<HarnessCommand> {
    build_harness_command_with_conversation(harness_id, prompt, cwd, mode, None)
}

pub fn build_harness_command_with_conversation(
    harness_id: &str,
    prompt: &str,
    cwd: &Path,
    mode: crate::harness::HarnessLaunchMode,
    conversation: Option<&crate::harness::ConversationHint>,
) -> Result<HarnessCommand> {
    let (program, args) = crate::harness::command_parts_for_harness_with_conversation(
        harness_id,
        prompt,
        mode,
        conversation,
    )?;

    Ok(HarnessCommand {
        program: program.to_string(),
        args,
        cwd: cwd.to_path_buf(),
    })
}

pub fn run_attached(command: &HarnessCommand) -> Result<PtyRunResult> {
    let pty_system = native_pty_system();
    run_attached_with_pty_system(command, pty_system.as_ref())
}

#[allow(dead_code)]
pub fn spawn_detached(command: &HarnessCommand) -> Result<DetachedPtySession> {
    spawn_detached_with_observer(command, None)
}

/// Handle returned by `spawn_piped_with_observer`. The child handle itself
/// is owned by the internal wait thread (so `wait()` can block without
/// blocking the killer); the caller gets a writable stdin and the PID so
/// it can signal termination via `libc::kill` instead of needing exclusive
/// access to the `Child`.
pub struct PipedSession {
    pub input: Box<dyn Write + Send>,
    pub pid: u32,
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
) -> Result<PipedSession> {
    use std::process::Command as StdCommand;
    use std::sync::{Arc, Mutex as StdMutex};

    let mut std_command = StdCommand::new(&command.program);
    std_command.args(&command.args);
    std_command.current_dir(&command.cwd);
    std_command.stdin(Stdio::piped());
    std_command.stdout(Stdio::piped());
    std_command.stderr(Stdio::piped());
    // Put the child in its own session/process group so the daemon can
    // signal it (and any subprocesses it spawns — skills, MCP servers,
    // shells) as a single unit via `kill(-pid, …)` from `PipedKiller`.
    // Without this, signals to the pid only reach the immediate child
    // and leave grandchildren as orphans.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            std_command.pre_exec(|| {
                // setsid() makes the calling process the session leader
                // of a new session AND the leader of a new process
                // group with no controlling terminal. Returns -1 on
                // failure (we propagate as io::Error to abort the spawn).
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let mut child = std_command.spawn().with_context(|| {
        format!(
            "failed to spawn harness `{}` in piped mode",
            command.program
        )
    })?;

    let pid = child.id();
    let stdin = child
        .stdin
        .take()
        .context("failed to take child stdin in piped mode")?;
    let stdout = child
        .stdout
        .take()
        .context("failed to take child stdout in piped mode")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to take child stderr in piped mode")?;

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
    thread::spawn(move || {
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
                    let envelope = serde_json::json!({
                        "type": "system",
                        "subtype": "stderr",
                        "text": line,
                    });
                    let mut payload = envelope.to_string();
                    payload.push('\n');
                    if let Ok(mut cb) = stderr_callback.lock() {
                        cb(payload.into_bytes());
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Stdout drain + wait. The wait thread OWNS `child`; the killer never
    // touches the `Child` handle, only the PID. That removes the previous
    // deadlock risk where `wait()` and `kill()` raced on a shared mutex.
    let stdout_callback = Arc::clone(&on_output_shared);
    thread::spawn(move || {
        let mut reader = stdout;
        let mut bridge: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            if let Ok(mut cb) = stdout_callback.lock() {
                cb(chunk);
            }
        });
        drain_detached_output(&mut reader, Some(&mut bridge));
        let result = match child.wait() {
            Ok(status) => PtyRunResult {
                status: if status.success() {
                    "completed"
                } else {
                    "failed"
                },
                exit_code: status.code(),
            },
            Err(_) => PtyRunResult {
                status: "failed",
                exit_code: None,
            },
        };
        on_exit(result);
    });

    Ok(PipedSession {
        input: Box::new(stdin),
        pid,
    })
}

pub fn spawn_detached_with_observer(
    command: &HarnessCommand,
    observer: Option<DetachedPtyObserver>,
) -> Result<DetachedPtySession> {
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
    let input = pair
        .master
        .take_writer()
        .context("failed to open PTY writer")?;
    let killer = child.clone_killer();

    thread::spawn(move || {
        let mut observer = observer;
        drain_detached_output(
            &mut reader,
            observer.as_mut().map(|observer| &mut observer.on_output),
        );
        let result = wait_for_child(&mut child);
        if let Some(observer) = observer {
            (observer.on_exit)(result);
        }
    });

    Ok(DetachedPtySession { input, killer })
}

fn drain_detached_output(
    reader: &mut dyn Read,
    mut on_output: Option<&mut Box<dyn FnMut(Vec<u8>) + Send + 'static>>,
) {
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes_read) => {
                if let Some(callback) = on_output.as_deref_mut() {
                    callback(buffer[..bytes_read].to_vec());
                }
            }
            Err(_) => break,
        }
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

fn run_attached_with_pty_system(
    command: &HarnessCommand,
    pty_system: &(dyn PtySystem + Send),
) -> Result<PtyRunResult> {
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
    let _raw_mode =
        RawModeGuard::enable_if_terminal().context("failed to enable raw terminal mode")?;

    let output_thread = thread::spawn(move || {
        let mut stdout = io::stdout().lock();
        io::copy(&mut reader, &mut stdout)?;
        stdout.flush()
    });

    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let _ = io::copy(&mut stdin, &mut writer);
    });

    let exit_status = child.wait().context("failed to wait for harness process")?;
    let _ = output_thread.join();
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

fn terminal_size() -> PtySize {
    PtySize {
        rows: env_u16("LINES").unwrap_or(24),
        cols: env_u16("COLUMNS").unwrap_or(80),
        pixel_width: 0,
        pixel_height: 0,
    }
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
    use super::*;

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
        assert_eq!(command.args(), &["hello; rm -rf /"]);
        assert_eq!(command.cwd(), cwd);
    }

    #[test]
    fn spawn_detached_starts_pty_and_returns_input_and_kill_handles() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let command = HarnessCommand {
            program: "cat".to_string(),
            args: vec![],
            cwd: temp_dir.path().to_path_buf(),
        };

        let mut session = spawn_detached(&command)?;
        session.input.write_all(b"hello detached pty\n")?;
        session.input.flush()?;
        session.killer.kill()?;
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
        assert_eq!(command.args(), &["explain && exit"]);
        assert_eq!(command.cwd(), cwd);
    }
}
