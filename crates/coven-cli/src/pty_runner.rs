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
    build_harness_command_with_conversation(harness_id, prompt, cwd, mode, None, None)
}

pub fn build_harness_command_with_conversation(
    harness_id: &str,
    prompt: &str,
    cwd: &Path,
    mode: crate::harness::HarnessLaunchMode,
    conversation: Option<&crate::harness::ConversationHint>,
    familiar: Option<&crate::harness::FamiliarContext>,
) -> Result<HarnessCommand> {
    let (program, args) = crate::harness::command_parts_for_harness_with_conversation(
        harness_id,
        prompt,
        mode,
        conversation,
        familiar,
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

/// Run `claude` in its native stream-JSON mode, framed by the caller (which
/// emits Coven's own `system.init` / `result` around the call).
///
/// `claude -p --input-format stream-json --output-format stream-json --verbose
/// --session-id <id> <prompt>` already emits the Coven-compatible JSONL
/// schema; we just forward each line untouched to `out`.
///
/// When `forward_stdin` is true, lines on our stdin are piped to claude's
/// stdin so callers can feed additional user messages mid-run. Stderr is
/// inherited so claude's own diagnostics land on the terminal.
pub fn stream_claude<W: Write>(
    cwd: &Path,
    session_id: &str,
    prompt: &str,
    forward_stdin: bool,
    system_prompt: Option<&str>,
    out: &mut W,
) -> Result<i32> {
    stream_claude_with_program("claude", cwd, session_id, prompt, forward_stdin, system_prompt, out)
}

fn stream_claude_with_program<W: Write>(
    program: &str,
    cwd: &Path,
    session_id: &str,
    prompt: &str,
    forward_stdin: bool,
    system_prompt: Option<&str>,
    out: &mut W,
) -> Result<i32> {
    let mut args: Vec<&str> = vec!["-p"];
    if forward_stdin {
        args.extend_from_slice(&["--input-format", "stream-json"]);
    }
    if let Some(sp) = system_prompt {
        args.extend_from_slice(&["--system-prompt", sp]);
    }
    args.extend_from_slice(&[
        "--output-format",
        "stream-json",
        "--verbose",
        "--session-id",
        session_id,
    ]);

    let mut child = std::process::Command::new(program)
        .args(&args)
        .arg(prompt)
        .current_dir(cwd)
        .stdin(if forward_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to spawn claude in stream-json mode")?;

    if forward_stdin {
        let mut child_stdin = child
            .stdin
            .take()
            .expect("stdin requested but child has no piped stdin");
        thread::spawn(move || {
            let stdin = io::stdin();
            let mut handle = stdin.lock();
            let mut buf = String::new();
            loop {
                buf.clear();
                match handle.read_line(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if child_stdin.write_all(buf.as_bytes()).is_err() {
                            break;
                        }
                        let _ = child_stdin.flush();
                    }
                }
            }
        });
    }

    let child_stdout = child.stdout.take().expect("stdout was requested as piped");
    let reader = BufReader::new(child_stdout);
    for line in reader.lines() {
        let line = line.context("reading claude stdout")?;
        if line.trim().is_empty() {
            continue;
        }
        writeln!(out, "{line}").context("forwarding claude stdout")?;
        out.flush().context("flushing claude stdout")?;
    }

    let status = child.wait().context("waiting on claude")?;
    Ok(status.code().unwrap_or(1))
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
    // Per-drain UTF-8 reassembly buffer. Each call to this function
    // owns its own buffer, so concurrent stdout+stderr drains in
    // `spawn_piped_with_observer` (which share an `on_output` via
    // Arc<Mutex>) can't corrupt each other's codepoint state. Each
    // chunk we hand to the callback is guaranteed valid UTF-8.
    let mut utf8_buf: Vec<u8> = Vec::with_capacity(8192);
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
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
                utf8_buf.extend_from_slice(&buffer[..bytes_read]);
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

    #[cfg(unix)]
    #[test]
    fn stream_claude_forwards_jsonl_and_returns_exit_code() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

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
        let code = stream_claude_with_program(
            fake_claude.to_str().unwrap(),
            temp_dir.path(),
            "session-123",
            "hello prompt",
            false,
            None,
            &mut out,
        )?;

        assert_eq!(code, 7);
        // One-shot mode (forward_stdin=false): `--input-format stream-json`
        // is omitted so the positional prompt is honored. Including it
        // makes claude wait for JSONL on stdin and ignore the positional —
        // which is the bug this commit fixes.
        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "-p\n--output-format\nstream-json\n--verbose\n--session-id\nsession-123\nhello prompt\n"
        );
        assert_eq!(
            String::from_utf8(out)?,
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]},\"session_id\":\"session-123\",\"stop_reason\":\"end_turn\"}\n"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stream_claude_includes_input_format_when_forwarding_stdin() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

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
        let _code = stream_claude_with_program(
            fake_claude.to_str().unwrap(),
            temp_dir.path(),
            "session-456",
            "hello prompt",
            true,
            None,
            &mut out,
        )?;

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "-p\n--input-format\nstream-json\n--output-format\nstream-json\n--verbose\n--session-id\nsession-456\nhello prompt\n"
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
