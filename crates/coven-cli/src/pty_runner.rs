use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::thread;

use anyhow::{anyhow, Context, Result};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use portable_pty::{native_pty_system, CommandBuilder, PtySize, PtySystem};

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

pub fn build_harness_command(harness_id: &str, prompt: &str, cwd: &Path) -> Result<HarnessCommand> {
    let program = match harness_id {
        "codex" => "codex",
        "claude" => "claude",
        _ => return Err(anyhow!("unsupported harness `{harness_id}`")),
    };

    Ok(HarnessCommand {
        program: program.to_string(),
        args: vec![prompt.to_string()],
        cwd: cwd.to_path_buf(),
    })
}

pub fn run_attached(command: &HarnessCommand) -> Result<PtyRunResult> {
    let pty_system = native_pty_system();
    run_attached_with_pty_system(command, pty_system.as_ref())
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
        let command = build_harness_command("codex", "hello; rm -rf /", cwd).unwrap();

        assert_eq!(command.program(), "codex");
        assert_eq!(command.args(), &["hello; rm -rf /"]);
        assert_eq!(command.cwd(), cwd);
    }

    #[test]
    fn builds_claude_command_without_shell_interpolation() {
        let cwd = Path::new("/tmp/coven-project");
        let command = build_harness_command("claude", "explain && exit", cwd).unwrap();

        assert_eq!(command.program(), "claude");
        assert_eq!(command.args(), &["explain && exit"]);
        assert_eq!(command.cwd(), cwd);
    }
}
