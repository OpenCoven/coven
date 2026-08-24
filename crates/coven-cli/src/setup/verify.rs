use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::{claude, codex, copilot, CommandSpec, ProviderId};

pub const PROMPT: &str = "Reply with OK.";

pub fn command(provider: ProviderId) -> CommandSpec {
    match provider {
        ProviderId::Codex => CommandSpec::new(
            codex::NON_INTERACTIVE_PREFIX_ARGS
                .iter()
                .copied()
                .chain(["--", PROMPT]),
        ),
        ProviderId::Claude => CommandSpec::new(
            claude::NON_INTERACTIVE_PREFIX_ARGS
                .iter()
                .copied()
                .chain(["--", PROMPT]),
        ),
        ProviderId::Copilot => CommandSpec::new(
            copilot::NON_INTERACTIVE_PREFIX_ARGS
                .iter()
                .map(|argument| (*argument).to_owned())
                .chain([copilot::verification_prompt_arg(PROMPT)]),
        ),
    }
}

pub struct EphemeralState {
    root: PathBuf,
    coven_home: PathBuf,
    cleaned: bool,
}

impl EphemeralState {
    pub fn create() -> io::Result<Self> {
        let temporary_root = std::env::temp_dir();
        for _ in 0..4 {
            let root = temporary_root.join(format!("coven-verify-{}", Uuid::new_v4()));
            let builder = {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    let mut builder = fs::DirBuilder::new();
                    builder.mode(0o700);
                    builder
                }
                #[cfg(not(unix))]
                {
                    fs::DirBuilder::new()
                }
            };
            match builder.create(&root) {
                Ok(()) => {
                    let coven_home = root.join("coven-home");
                    if let Err(error) = fs::create_dir(&coven_home) {
                        let _ = fs::remove_dir(&root);
                        return Err(error);
                    }
                    return Ok(Self {
                        root,
                        coven_home,
                        cleaned: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate unique verification state",
        ))
    }

    pub fn current_dir(&self) -> &Path {
        &self.root
    }

    pub fn env_overrides(&self) -> Vec<(OsString, Option<OsString>)> {
        vec![(
            OsString::from("COVEN_HOME"),
            Some(self.coven_home.as_os_str().to_owned()),
        )]
    }

    pub fn cleanup(mut self) -> io::Result<()> {
        fs::remove_dir_all(&self.root)?;
        if self.root.exists() {
            return Err(io::Error::other(
                "verification state still exists after cleanup",
            ));
        }
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for EphemeralState {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
