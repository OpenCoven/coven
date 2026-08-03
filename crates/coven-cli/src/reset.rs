//! Guarded, recoverable reset support for selected Coven-local state.
//!
//! This module deliberately knows only an allowlist of state locations below
//! `COVEN_HOME`. It never reads file contents, follows symlinks, contacts a
//! network service, or accepts arbitrary user-provided paths.

use std::ffi::{OsStr, OsString};
#[cfg(test)]
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::{ambient_authority, fs::Dir};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const BACKUP_DIR: &str = "reset-backups";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResetErrorKind {
    InvalidSelection,
    ConfirmationRequired,
    MissingState,
    PartialFailure,
    UnsafePath,
    DaemonActive,
}

#[derive(Debug)]
pub(crate) struct ResetError {
    kind: ResetErrorKind,
    message: String,
}

impl ResetError {
    fn new(kind: ResetErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn exit_code(&self) -> i32 {
        match self.kind {
            ResetErrorKind::InvalidSelection => 2,
            ResetErrorKind::ConfirmationRequired | ResetErrorKind::DaemonActive => 3,
            ResetErrorKind::MissingState => 4,
            ResetErrorKind::PartialFailure => 5,
            ResetErrorKind::UnsafePath => 6,
        }
    }
}

impl std::fmt::Display for ResetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ResetError {}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum Sensitivity {
    Standard,
    Sensitive,
}

#[derive(Clone, Copy, Debug)]
struct ResetFeature {
    name: &'static str,
    display_name: &'static str,
    description: &'static str,
    paths: &'static [&'static str],
    sensitivity: Sensitivity,
    warning: &'static str,
}

const RESET_FEATURES: &[ResetFeature] = &[
    ResetFeature {
        name: "familiars",
        display_name: "Familiars",
        description: "The familiar registry and Coven-managed familiar workspaces.",
        paths: &["familiars.toml", "familiars"],
        sensitivity: Sensitivity::Standard,
        warning:
            "Restores only from the local reset backup; it does not affect external repositories.",
    },
    ResetFeature {
        name: "projects",
        display_name: "Projects",
        description: "The legacy local project registry stored in COVEN_HOME.",
        paths: &["repos.toml"],
        sensitivity: Sensitivity::Standard,
        warning:
            "Does not delete project directories, Git repositories, or XDG settings.json entries.",
    },
    ResetFeature {
        name: "github",
        display_name: "GitHub integration",
        description: "Coven-local GitHub/Copilot adapter configuration only.",
        paths: &[
            "adapters/github-copilot.json",
            "adapters/github.json",
            "adapters/copilot.json",
        ],
        sensitivity: Sensitivity::Sensitive,
        warning:
            "Does not contact GitHub, revoke tokens, change accounts, or alter remote repositories.",
    },
    ResetFeature {
        name: "claude",
        display_name: "Claude Code integration",
        description: "A Coven-local Claude adapter record, when one was installed.",
        paths: &["adapters/claude.json"],
        sensitivity: Sensitivity::Sensitive,
        warning:
            "Does not alter Claude Code's own configuration, login, projects, or provider account.",
    },
    ResetFeature {
        name: "openclaw",
        display_name: "OpenClaw bridge integration",
        description: "A Coven-local OpenClaw bridge adapter record, when one was installed.",
        paths: &["adapters/openclaw.json"],
        sensitivity: Sensitivity::Sensitive,
        warning:
            "Does not alter OpenClaw core, bridge-plugin configuration, ACP routing, or provider credentials.",
    },
    ResetFeature {
        name: "hermes",
        display_name: "Hermes integration",
        description: "The Coven-installed Hermes runtime adapter manifest.",
        paths: &["adapters/hermes.json"],
        sensitivity: Sensitivity::Sensitive,
        warning:
            "Does not alter Hermes Agent configuration, login, models, or provider credentials.",
    },
    ResetFeature {
        name: "opencode",
        display_name: "OpenCode integration",
        description: "The Coven-installed OpenCode runtime adapter manifest.",
        paths: &["adapters/opencode.json"],
        sensitivity: Sensitivity::Sensitive,
        warning:
            "Does not alter OpenCode configuration, project files, login, or provider credentials.",
    },
    ResetFeature {
        name: "grok-build",
        display_name: "Grok Build integration",
        description: "The Coven-installed Grok Build runtime adapter manifest.",
        paths: &["adapters/grok.json"],
        sensitivity: Sensitivity::Sensitive,
        warning:
            "Does not alter Grok Build configuration, login, XAI_API_KEY, or provider account.",
    },
    ResetFeature {
        name: "gemini",
        display_name: "Gemini CLI integration",
        description: "A Coven-local Gemini CLI adapter record, when one was installed.",
        paths: &["adapters/gemini.json"],
        sensitivity: Sensitivity::Sensitive,
        warning:
            "Does not alter Gemini CLI configuration, login, MCP servers, or Google account state.",
    },
    ResetFeature {
        name: "secrets",
        display_name: "Secrets",
        description: "Reserved Coven-local secret-reference state.",
        paths: &["secrets"],
        sensitivity: Sensitivity::Sensitive,
        warning:
            "Artifact encryption keys are coupled to session reset so ciphertext remains readable.",
    },
    ResetFeature {
        name: "caches",
        display_name: "Caches",
        description: "Coven-local capability and derived-data caches.",
        paths: &["cache", "capabilities-cache.json"],
        sensitivity: Sensitivity::Standard,
        warning:
            "Caches are recreated only by normal local Coven use; no network refresh is performed.",
    },
    ResetFeature {
        name: "sessions",
        display_name: "Sessions",
        description: "The local session ledger, artifacts, and chat persistence.",
        paths: &[
            "coven.sqlite3",
            "coven.sqlite3-wal",
            "coven.sqlite3-shm",
            "session-artifacts",
            "chat-conversations",
            "keys/session-artifacts.key",
        ],
        sensitivity: Sensitivity::Sensitive,
        warning: "Includes the artifact key so encrypted session records stay coupled to their key.",
    },
    ResetFeature {
        name: "mobile",
        display_name: "Mobile gateway",
        description: "Mobile gateway configuration, paired devices, host identity, and audit state.",
        paths: &["mobile"],
        sensitivity: Sensitivity::Sensitive,
        warning: "Removes local mobile pairings and host credentials; paired devices must pair again.",
    },
    ResetFeature {
        name: "metadata",
        display_name: "Metadata",
        description: "Local research, handoff, queue, executor, and recovery metadata.",
        paths: &[
            "cave-coven-calls.json",
            "research",
            "travel",
            "pending",
            "executor.json",
            "daemon-recovery.log",
        ],
        sensitivity: Sensitivity::Standard,
        warning: "Does not reset daemon connection settings or external services.",
    },
];

#[derive(Clone, Copy, Debug)]
pub(crate) struct ResetRequest<'a> {
    pub(crate) features: &'a [String],
    pub(crate) all: bool,
    pub(crate) apply: bool,
    pub(crate) json: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureDescription {
    name: String,
    display_name: String,
    description: String,
    paths: Vec<String>,
    sensitivity: Sensitivity,
    warning: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FeatureOutcome {
    Preview,
    Reset,
    Missing,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureReport {
    name: String,
    display_name: String,
    sensitivity: Sensitivity,
    warning: String,
    targets: Vec<String>,
    outcome: FeatureOutcome,
    moved_to_backup: Vec<String>,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResetReport {
    mode: String,
    coven_home: String,
    backup: Option<String>,
    features: Vec<FeatureReport>,
}

#[derive(Clone, Debug)]
struct PlannedTarget {
    relative: PathBuf,
}

#[derive(Clone, Debug)]
struct PlannedFeature {
    feature: &'static ResetFeature,
    targets: Vec<PlannedTarget>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResetTransaction {
    version: u8,
    backup_id: String,
    moves: Vec<TransactionMove>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TransactionMove {
    source: PathBuf,
    destination: PathBuf,
}

struct RootedHome {
    dir: Dir,
}

impl RootedHome {
    fn open_existing(path: &Path, protect: bool) -> Result<Option<Self>> {
        if path
            .components()
            .all(|component| component == Component::CurDir)
        {
            let dir = Dir::open_ambient_dir(".", ambient_authority())
                .context("failed to open COVEN_HOME current directory")?;
            validate_opened_home(&dir, path, protect)?;
            return Ok(Some(Self { dir }));
        }
        let parent_path = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty());
        let parent_path = parent_path.unwrap_or_else(|| Path::new("."));
        let name = path.file_name().ok_or_else(|| {
            ResetError::new(
                ResetErrorKind::UnsafePath,
                format!(
                    "COVEN_HOME {} must not be a filesystem root",
                    path.display()
                ),
            )
        })?;
        let Some(parent) = open_trusted_parent(parent_path)? else {
            return Ok(None);
        };
        let metadata = match parent.symlink_metadata(name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect COVEN_HOME {}", path.display()));
            }
        };
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata_is_windows_reparse_point(&metadata)
        {
            return Err(ResetError::new(
                ResetErrorKind::UnsafePath,
                format!("COVEN_HOME {} is not a real directory", path.display()),
            )
            .into());
        }
        let dir = parent
            .open_dir_nofollow(name)
            .with_context(|| format!("failed to open COVEN_HOME {}", path.display()))?;
        validate_opened_home(&dir, path, protect)?;
        Ok(Some(Self { dir }))
    }

    fn target_exists(&self, relative: &Path) -> Result<bool> {
        let Some((parent, name)) = self.open_parent(relative)? else {
            return Ok(false);
        };
        match parent.symlink_metadata(&name) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || metadata_is_windows_reparse_point(&metadata)
                {
                    return Err(ResetError::new(
                        ResetErrorKind::UnsafePath,
                        format!("refusing to reset symlinked state `{}`", relative.display()),
                    )
                    .into());
                }
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error)
                .with_context(|| format!("failed to inspect reset target {}", relative.display())),
        }
    }

    fn ensure_private_dir(&self, relative: &Path) -> Result<Dir> {
        if relative.as_os_str().is_empty() {
            return self
                .dir
                .try_clone()
                .context("failed to clone COVEN_HOME handle");
        }
        validate_relative_path(relative)?;
        let mut directory = self
            .dir
            .try_clone()
            .context("failed to clone COVEN_HOME handle")?;
        for component in normal_components(relative)? {
            let mut created = false;
            match directory.symlink_metadata(component) {
                Ok(metadata)
                    if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) =>
                {
                    return Err(ResetError::new(
                        ResetErrorKind::UnsafePath,
                        format!(
                            "refusing unsafe reset backup directory {}",
                            relative.display()
                        ),
                    )
                    .into());
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    match directory.create_dir(component) {
                        Ok(()) => created = true,
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(error) => {
                            return Err(error).with_context(|| {
                                format!("failed to create reset backup {}", relative.display())
                            });
                        }
                    }
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed to inspect reset backup {}", relative.display())
                    });
                }
            }
            let opened = directory
                .open_dir_nofollow(component)
                .with_context(|| format!("failed to open reset backup {}", relative.display()))?;
            let metadata = opened.dir_metadata().with_context(|| {
                format!("failed to inspect reset backup {}", relative.display())
            })?;
            if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
                return Err(ResetError::new(
                    ResetErrorKind::UnsafePath,
                    format!(
                        "refusing unsafe reset backup directory {}",
                        relative.display()
                    ),
                )
                .into());
            }
            protect_private_dir(&opened)?;
            if created {
                sync_directory(&opened).with_context(|| {
                    format!("failed to persist reset backup {}", relative.display())
                })?;
                sync_directory(&directory).with_context(|| {
                    format!("failed to persist reset backup {}", relative.display())
                })?;
            }
            directory = opened;
        }
        Ok(directory)
    }

    fn rename_without_following(&self, source: &Path, destination: &Path) -> Result<()> {
        let Some((source_parent, source_name)) = self.open_parent(source)? else {
            anyhow::bail!("reset source `{}` disappeared", source.display());
        };
        let source_metadata = source_parent
            .symlink_metadata(&source_name)
            .with_context(|| format!("failed to inspect reset source `{}`", source.display()))?;
        if source_metadata.file_type().is_symlink()
            || metadata_is_windows_reparse_point(&source_metadata)
        {
            return Err(ResetError::new(
                ResetErrorKind::UnsafePath,
                format!("refusing to reset symlinked state `{}`", source.display()),
            )
            .into());
        }

        let (destination_parent, destination_name) = self.prepare_destination(destination)?;

        source_parent
            .rename(&source_name, &destination_parent, &destination_name)
            .with_context(|| {
                format!(
                    "could not move `{}` to `{}`",
                    source.display(),
                    destination.display()
                )
            })
    }

    fn prepare_destination(&self, destination: &Path) -> Result<(Dir, OsString)> {
        let destination_parent_path = destination
            .parent()
            .context("reset backup target has no parent")?;
        let destination_parent = self.ensure_private_dir(destination_parent_path)?;
        let destination_name = destination
            .file_name()
            .context("reset backup target has no file name")?
            .to_os_string();
        match destination_parent.symlink_metadata(&destination_name) {
            Ok(_) => {
                return Err(ResetError::new(
                    ResetErrorKind::UnsafePath,
                    format!(
                        "refusing to overwrite reset backup {}",
                        destination.display()
                    ),
                )
                .into());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to inspect reset backup {}", destination.display())
                });
            }
        }
        Ok((destination_parent, destination_name))
    }

    fn has_transaction(&self) -> Result<bool> {
        match self
            .dir
            .symlink_metadata(crate::state_lock::RESET_TRANSACTION_FILE)
        {
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    || metadata_is_windows_reparse_point(&metadata) =>
            {
                Err(ResetError::new(
                    ResetErrorKind::UnsafePath,
                    "refusing symlinked reset transaction marker",
                )
                .into())
            }
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error).context("failed to inspect reset transaction marker"),
        }
    }

    fn write_transaction(&self, transaction: &ResetTransaction) -> Result<()> {
        if self.has_transaction()? {
            anyhow::bail!("an incomplete reset transaction already exists");
        }
        let temp_name = format!(".reset-transaction-{}.tmp", Uuid::new_v4().simple());
        let mut options = cap_std::fs::OpenOptions::new();
        options
            .create_new(true)
            .write(true)
            .follow(FollowSymlinks::No);
        let mut file = self
            .dir
            .open_with(&temp_name, &options)
            .context("failed to create reset transaction marker")?;
        let bytes =
            serde_json::to_vec(transaction).context("failed to encode reset transaction")?;
        file.write_all(&bytes)
            .context("failed to write reset transaction marker")?;
        file.sync_all()
            .context("failed to persist reset transaction marker")?;
        drop(file);
        if let Err(error) = self.dir.rename(
            &temp_name,
            &self.dir,
            crate::state_lock::RESET_TRANSACTION_FILE,
        ) {
            let _ = self.dir.remove_file(&temp_name);
            return Err(error).context("failed to activate reset transaction marker");
        }
        sync_directory(&self.dir).context("failed to persist reset transaction activation")?;
        Ok(())
    }

    fn read_transaction(&self) -> Result<Option<ResetTransaction>> {
        if !self.has_transaction()? {
            return Ok(None);
        }
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let file = self
            .dir
            .open_with(crate::state_lock::RESET_TRANSACTION_FILE, &options)
            .context("failed to open reset transaction marker")?;
        let metadata = file
            .metadata()
            .context("failed to inspect reset transaction marker")?;
        const MAX_TRANSACTION_BYTES: u64 = 1024 * 1024;
        if !metadata.is_file()
            || metadata_is_windows_reparse_point(&metadata)
            || metadata.len() > MAX_TRANSACTION_BYTES
        {
            return Err(ResetError::new(
                ResetErrorKind::UnsafePath,
                "reset transaction marker is unsafe or too large",
            )
            .into());
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_TRANSACTION_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("failed to read reset transaction marker")?;
        if bytes.len() as u64 > MAX_TRANSACTION_BYTES {
            return Err(ResetError::new(
                ResetErrorKind::UnsafePath,
                "reset transaction marker is too large",
            )
            .into());
        }
        let transaction: ResetTransaction =
            serde_json::from_slice(&bytes).context("reset transaction marker is invalid")?;
        if transaction.version != 1 {
            anyhow::bail!(
                "unsupported reset transaction version {}",
                transaction.version
            );
        }
        for item in &transaction.moves {
            validate_relative_path(&item.source)?;
            validate_relative_path(&item.destination)?;
            if !item
                .destination
                .starts_with(Path::new(BACKUP_DIR).join(&transaction.backup_id))
            {
                return Err(ResetError::new(
                    ResetErrorKind::UnsafePath,
                    "reset transaction destination is outside its backup",
                )
                .into());
            }
        }
        Ok(Some(transaction))
    }

    fn recover_incomplete_transaction(&self) -> Result<()> {
        let Some(transaction) = self.read_transaction()? else {
            return Ok(());
        };
        for item in transaction.moves.iter().rev() {
            let source_exists = self.target_exists(&item.source)?;
            let destination_exists = self.target_exists(&item.destination)?;
            match (source_exists, destination_exists) {
                (true, false) => {}
                (false, true) => {
                    self.rename_without_following(&item.destination, &item.source)
                        .with_context(|| {
                            format!(
                                "failed to roll back interrupted reset target `{}`",
                                item.source.display()
                            )
                        })?;
                }
                (true, true) => {
                    return Err(ResetError::new(
                        ResetErrorKind::UnsafePath,
                        format!(
                            "cannot recover reset target `{}` because both source and backup exist",
                            item.source.display()
                        ),
                    )
                    .into());
                }
                (false, false) => {
                    return Err(ResetError::new(
                        ResetErrorKind::UnsafePath,
                        format!(
                            "cannot recover reset target `{}` because both source and backup are missing",
                            item.source.display()
                        ),
                    )
                    .into());
                }
            }
        }
        self.sync_transaction_parents(&transaction)?;
        self.remove_transaction()
    }

    fn complete_transaction(&self, transaction: &ResetTransaction) -> Result<()> {
        for item in &transaction.moves {
            let source_exists = self.target_exists(&item.source)?;
            let destination_exists = self.target_exists(&item.destination)?;
            if source_exists == destination_exists {
                anyhow::bail!(
                    "reset target `{}` is not in exactly one recoverable location",
                    item.source.display()
                );
            }
        }
        self.sync_transaction_parents(transaction)?;
        self.remove_transaction()
    }

    fn sync_transaction_parents(&self, transaction: &ResetTransaction) -> Result<()> {
        for item in &transaction.moves {
            for path in [&item.source, &item.destination] {
                let parent = path.parent().context("reset target has no parent")?;
                if let Some(directory) = self.open_existing_dir(parent)? {
                    sync_directory(&directory).with_context(|| {
                        format!("failed to persist reset directory `{}`", parent.display())
                    })?;
                }
            }
        }
        Ok(())
    }

    fn open_existing_dir(&self, relative: &Path) -> Result<Option<Dir>> {
        if relative.as_os_str().is_empty() {
            return self
                .dir
                .try_clone()
                .context("failed to clone COVEN_HOME handle")
                .map(Some);
        }
        let Some((parent, name)) = self.open_parent(relative)? else {
            return Ok(None);
        };
        let metadata = match parent.symlink_metadata(&name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to inspect reset directory `{}`", relative.display())
                });
            }
        };
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata_is_windows_reparse_point(&metadata)
        {
            return Err(ResetError::new(
                ResetErrorKind::UnsafePath,
                format!("reset directory `{}` is unsafe", relative.display()),
            )
            .into());
        }
        parent
            .open_dir_nofollow(&name)
            .with_context(|| format!("failed to open reset directory `{}`", relative.display()))
            .map(Some)
    }

    fn remove_transaction(&self) -> Result<()> {
        self.dir
            .remove_file(crate::state_lock::RESET_TRANSACTION_FILE)
            .context("failed to remove completed reset transaction marker")?;
        sync_directory(&self.dir).context("failed to persist reset transaction completion")
    }

    fn open_parent(&self, relative: &Path) -> Result<Option<(Dir, OsString)>> {
        validate_relative_path(relative)?;
        let components = normal_components(relative)?;
        let Some((name, parents)) = components.split_last() else {
            return Err(ResetError::new(
                ResetErrorKind::UnsafePath,
                format!("unsafe reset state selector `{}`", relative.display()),
            )
            .into());
        };
        let mut directory = self
            .dir
            .try_clone()
            .context("failed to clone COVEN_HOME handle")?;
        for component in parents {
            let metadata = match directory.symlink_metadata(component) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed to inspect reset target {}", relative.display())
                    });
                }
            };
            if metadata.file_type().is_symlink() || metadata_is_windows_reparse_point(&metadata) {
                return Err(ResetError::new(
                    ResetErrorKind::UnsafePath,
                    format!(
                        "reset target `{}` has a symlinked ancestor",
                        relative.display()
                    ),
                )
                .into());
            }
            if !metadata.is_dir() {
                return Ok(None);
            }
            let opened = directory
                .open_dir_nofollow(component)
                .with_context(|| format!("failed to open reset target {}", relative.display()))?;
            let opened_metadata = opened.dir_metadata().with_context(|| {
                format!("failed to inspect reset target {}", relative.display())
            })?;
            if !opened_metadata.is_dir() || metadata_is_windows_reparse_point(&opened_metadata) {
                return Err(ResetError::new(
                    ResetErrorKind::UnsafePath,
                    format!(
                        "reset target `{}` has a symlinked or non-directory ancestor",
                        relative.display()
                    ),
                )
                .into());
            }
            directory = opened;
        }
        Ok(Some((directory, name.to_os_string())))
    }
}

fn validate_opened_home(dir: &Dir, path: &Path, protect: bool) -> Result<()> {
    let metadata = dir
        .dir_metadata()
        .with_context(|| format!("failed to inspect COVEN_HOME {}", path.display()))?;
    if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
        return Err(ResetError::new(
            ResetErrorKind::UnsafePath,
            format!("COVEN_HOME {} is not a real directory", path.display()),
        )
        .into());
    }
    validate_private_home(dir, path, protect)?;
    Ok(())
}

#[cfg(unix)]
fn open_trusted_parent(path: &Path) -> Result<Option<Dir>> {
    use std::os::unix::fs::MetadataExt;

    let before = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to inspect COVEN_HOME parent {}", path.display())
            });
        }
    };
    // SAFETY: geteuid() only reads the effective uid and cannot fail.
    let euid = unsafe { libc::geteuid() };
    if !before.is_dir()
        || before.file_type().is_symlink()
        || before.uid() != euid
        || before.mode() & 0o022 != 0
    {
        return Err(ResetError::new(
            ResetErrorKind::UnsafePath,
            format!(
                "COVEN_HOME parent {} must be a real directory owned by the current user and not writable by other users",
                path.display()
            ),
        )
        .into());
    }
    let dir = Dir::open_ambient_dir(path, ambient_authority())
        .with_context(|| format!("failed to open COVEN_HOME parent {}", path.display()))?;
    let after = dir
        .dir_metadata()
        .with_context(|| format!("failed to inspect COVEN_HOME parent {}", path.display()))?;
    if before.dev() != cap_fs_ext::MetadataExt::dev(&after)
        || before.ino() != cap_fs_ext::MetadataExt::ino(&after)
    {
        return Err(ResetError::new(
            ResetErrorKind::UnsafePath,
            format!(
                "COVEN_HOME parent {} changed while it was being opened",
                path.display()
            ),
        )
        .into());
    }
    Ok(Some(dir))
}

#[cfg(windows)]
fn open_trusted_parent(path: &Path) -> Result<Option<Dir>> {
    let absolute = std::path::absolute(path)
        .with_context(|| format!("failed to resolve COVEN_HOME parent {}", path.display()))?;
    let root = absolute.ancestors().last().ok_or_else(|| {
        ResetError::new(
            ResetErrorKind::UnsafePath,
            format!(
                "COVEN_HOME parent {} has no filesystem root",
                path.display()
            ),
        )
    })?;
    let mut dir = Dir::open_ambient_dir(root, ambient_authority())
        .with_context(|| format!("failed to open filesystem root {}", root.display()))?;
    let relative = absolute.strip_prefix(root).with_context(|| {
        format!(
            "failed to anchor COVEN_HOME parent {} at {}",
            path.display(),
            root.display()
        )
    })?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(ResetError::new(
                ResetErrorKind::UnsafePath,
                format!("COVEN_HOME parent {} is not normalized", path.display()),
            )
            .into());
        };
        let metadata = match dir.symlink_metadata(name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to inspect COVEN_HOME parent {}", path.display())
                });
            }
        };
        if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
            return Err(ResetError::new(
                ResetErrorKind::UnsafePath,
                format!(
                    "COVEN_HOME parent {} crosses a reparse point or non-directory",
                    path.display()
                ),
            )
            .into());
        }
        dir = dir
            .open_dir_nofollow(name)
            .with_context(|| format!("failed to open COVEN_HOME parent {}", path.display()))?;
    }
    Ok(Some(dir))
}

#[cfg(not(any(unix, windows)))]
fn open_trusted_parent(path: &Path) -> Result<Option<Dir>> {
    match Dir::open_ambient_dir(path, ambient_authority()) {
        Ok(dir) => Ok(Some(dir)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("failed to open COVEN_HOME parent {}", path.display())),
    }
}

fn normal_components(path: &Path) -> Result<Vec<&OsStr>> {
    validate_relative_path(path)?;
    path.components()
        .map(|component| match component {
            Component::Normal(value) => Ok(value),
            _ => Err(ResetError::new(
                ResetErrorKind::UnsafePath,
                format!("unsafe reset state selector `{}`", path.display()),
            )
            .into()),
        })
        .collect()
}

#[cfg(unix)]
fn protect_private_dir(dir: &Dir) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    open_syncable_directory(dir)?
        .into_std()
        .set_permissions(std::fs::Permissions::from_mode(0o700))
        .context("failed to protect reset backup directory")
}

#[cfg(unix)]
fn validate_private_home(dir: &Dir, path: &Path, protect: bool) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let file = open_syncable_directory(dir)?.into_std();
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect COVEN_HOME {}", path.display()))?;
    // SAFETY: geteuid() only reads the effective uid and cannot fail.
    let euid = unsafe { libc::geteuid() };
    if metadata.uid() != euid {
        anyhow::bail!(
            "refusing to use COVEN_HOME {}: it is owned by uid {}, not the current user (uid {euid})",
            path.display(),
            metadata.uid()
        );
    }
    if protect {
        file.set_permissions(std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to protect COVEN_HOME {}", path.display()))?;
    } else if metadata.mode() & 0o077 != 0 {
        anyhow::bail!(
            "refusing to preview COVEN_HOME {} because its permissions are not private",
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_home(_dir: &Dir, _path: &Path, _protect: bool) -> Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn protect_private_dir(_dir: &Dir) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(dir: &Dir) -> Result<()> {
    open_syncable_directory(dir)?
        .sync_all()
        .context("failed to sync directory")
}

#[cfg(windows)]
fn sync_directory(_dir: &Dir) -> Result<()> {
    // Windows does not support FlushFileBuffers on directory handles. File
    // writes are flushed before marker activation, and rename/remove calls are
    // synchronous; attempting a directory flush would make every reset fail.
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn sync_directory(_dir: &Dir) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn open_syncable_directory(dir: &Dir) -> Result<cap_std::fs::File> {
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    dir.open_with(".", &options)
        .context("failed to open directory for durability")
}

#[cfg(windows)]
fn metadata_is_windows_reparse_point(metadata: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_windows_reparse_point(_metadata: &cap_std::fs::Metadata) -> bool {
    false
}

pub(crate) fn run(request: ResetRequest<'_>, list_features: bool) -> Result<()> {
    if list_features {
        if request.all || !request.features.is_empty() || request.apply {
            return Err(ResetError::new(
                ResetErrorKind::InvalidSelection,
                "--list-features cannot be combined with --feature, --all, or --apply",
            )
            .into());
        }
        return render_features(request.json);
    }

    reject_unsupported_apply(request.apply)?;
    let report = execute(crate::coven_home_dir()?, request)?;
    render_report(&report, request.json)?;
    report_exit(&report)
}

#[cfg(windows)]
fn reject_unsupported_apply(apply: bool) -> Result<()> {
    if apply {
        return Err(ResetError::new(
            ResetErrorKind::InvalidSelection,
            "`coven reset --apply` is unavailable on Windows because Windows does not provide the durable directory-entry ordering required for recoverable reset; preview remains available",
        )
        .into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn reject_unsupported_apply(_apply: bool) -> Result<()> {
    Ok(())
}

fn render_features(json: bool) -> Result<()> {
    let features: Vec<FeatureDescription> =
        RESET_FEATURES.iter().map(feature_description).collect();
    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({ "features": features }))?
        );
        return Ok(());
    }

    println!("Coven reset features (all state is local to COVEN_HOME):");
    for feature in features {
        let sensitivity = match feature.sensitivity {
            Sensitivity::Standard => "standard",
            Sensitivity::Sensitive => "sensitive",
        };
        println!(
            "- {} ({sensitivity}): {}",
            feature.name, feature.description
        );
        println!("  state: {}", feature.paths.join(", "));
        println!("  note: {}", feature.warning);
    }
    println!("Use `coven reset --feature <name>` to preview a reset.");
    Ok(())
}

fn feature_description(feature: &ResetFeature) -> FeatureDescription {
    FeatureDescription {
        name: feature.name.to_string(),
        display_name: feature.display_name.to_string(),
        description: feature.description.to_string(),
        paths: feature
            .paths
            .iter()
            .map(|path| (*path).to_string())
            .collect(),
        sensitivity: feature.sensitivity,
        warning: feature.warning.to_string(),
    }
}

fn execute(coven_home: PathBuf, request: ResetRequest<'_>) -> Result<ResetReport> {
    let selected = select_features(request.features, request.all)?;
    if request.all && !request.apply {
        return Err(ResetError::new(
            ResetErrorKind::ConfirmationRequired,
            "--all resets every registered local category and requires explicit confirmation; rerun with --all --apply",
        )
        .into());
    }

    let rooted_home = RootedHome::open_existing(&coven_home, request.apply)?;
    let home = rooted_home.as_ref().map(|_| coven_home);
    if !request.apply {
        if rooted_home
            .as_ref()
            .map(RootedHome::has_transaction)
            .transpose()?
            .unwrap_or(false)
        {
            return Err(ResetError::new(
                ResetErrorKind::ConfirmationRequired,
                "an interrupted reset requires rollback; rerun this reset selection with --apply",
            )
            .into());
        }
        let plan = build_plan(rooted_home.as_ref(), &selected)?;
        return Ok(report_from_plan("preview", &home, None, &plan));
    }

    let _state_lock = acquire_exclusive_state(&home, rooted_home.as_ref())?;
    let _daemon_guard = acquire_daemon_reset_guard(&home, rooted_home.as_ref())?;
    if let Some(rooted_home) = rooted_home.as_ref() {
        rooted_home.recover_incomplete_transaction()?;
    }
    let plan = build_plan(rooted_home.as_ref(), &selected)?;
    let backup_id = backup_id();
    apply_plan(&home, rooted_home.as_ref(), &plan, &backup_id)
}

fn select_features(requested: &[String], all: bool) -> Result<Vec<&'static ResetFeature>> {
    if all && !requested.is_empty() {
        return Err(ResetError::new(
            ResetErrorKind::InvalidSelection,
            "use either --all or one or more --feature values, not both",
        )
        .into());
    }
    if all {
        return Ok(RESET_FEATURES.iter().collect());
    }
    if requested.is_empty() {
        return Err(ResetError::new(
            ResetErrorKind::InvalidSelection,
            "select at least one feature with --feature <name>, use --all --apply, or run --list-features",
        )
        .into());
    }

    let mut selected = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    for name in requested {
        if !names.insert(name) {
            return Err(ResetError::new(
                ResetErrorKind::InvalidSelection,
                format!("reset feature `{name}` was selected more than once"),
            )
            .into());
        }
        let feature = RESET_FEATURES
            .iter()
            .find(|feature| feature.name == name)
            .ok_or_else(|| {
                ResetError::new(
                    ResetErrorKind::InvalidSelection,
                    format!("unknown reset feature `{name}`; run `coven reset --list-features`"),
                )
            })?;
        selected.push(feature);
    }
    selected.sort_by_key(|feature| feature.name);
    Ok(selected)
}

fn build_plan(
    home: Option<&RootedHome>,
    selected: &[&'static ResetFeature],
) -> Result<Vec<PlannedFeature>> {
    selected
        .iter()
        .map(|feature| {
            let mut targets = Vec::new();
            if let Some(home) = home {
                for raw in feature.paths {
                    let relative = PathBuf::from(raw);
                    validate_relative_path(&relative)?;
                    if home.target_exists(&relative)? {
                        targets.push(PlannedTarget { relative });
                    }
                }
            }
            Ok(PlannedFeature { feature, targets })
        })
        .collect()
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err(ResetError::new(
            ResetErrorKind::UnsafePath,
            format!("unsafe reset state selector `{}`", path.display()),
        )
        .into());
    }
    Ok(())
}

fn acquire_exclusive_state(
    home: &Option<PathBuf>,
    rooted_home: Option<&RootedHome>,
) -> Result<Option<crate::state_lock::StateLock>> {
    let (Some(home), Some(rooted_home)) = (home, rooted_home) else {
        return Ok(None);
    };
    crate::state_lock::try_acquire_exclusive_in(&rooted_home.dir, home)?.map_or_else(
        || {
            Err(ResetError::new(
                ResetErrorKind::DaemonActive,
                "refusing to reset state while another Coven command is active; stop the daemon and close active Coven commands, then retry",
            )
            .into())
        },
        |guard| Ok(Some(guard)),
    )
}

fn acquire_daemon_reset_guard(
    home: &Option<PathBuf>,
    rooted_home: Option<&RootedHome>,
) -> Result<Option<crate::daemon::ResetDaemonGuard>> {
    let (Some(home), Some(rooted_home)) = (home, rooted_home) else {
        return Ok(None);
    };
    crate::daemon::try_acquire_reset_guard(home, &rooted_home.dir)?.map_or_else(
        || {
            Err(ResetError::new(
                ResetErrorKind::DaemonActive,
                "refusing to reset state while a Coven daemon is active; stop the daemon, then retry",
            )
            .into())
        },
        |guard| Ok(Some(guard)),
    )
}

fn report_from_plan(
    mode: &str,
    home: &Option<PathBuf>,
    backup: Option<String>,
    plan: &[PlannedFeature],
) -> ResetReport {
    ResetReport {
        mode: mode.to_string(),
        coven_home: home.as_ref().map_or_else(
            || "unavailable".to_string(),
            |path| path.display().to_string(),
        ),
        backup,
        features: plan
            .iter()
            .map(|item| FeatureReport {
                name: item.feature.name.to_string(),
                display_name: item.feature.display_name.to_string(),
                sensitivity: item.feature.sensitivity,
                warning: item.feature.warning.to_string(),
                targets: item
                    .targets
                    .iter()
                    .map(|target| target.relative.display().to_string())
                    .collect(),
                outcome: if item.targets.is_empty() {
                    FeatureOutcome::Missing
                } else {
                    FeatureOutcome::Preview
                },
                moved_to_backup: Vec::new(),
                error: None,
            })
            .collect(),
    }
}

fn apply_plan(
    home: &Option<PathBuf>,
    rooted_home: Option<&RootedHome>,
    plan: &[PlannedFeature],
    backup_id: &str,
) -> Result<ResetReport> {
    let Some(home) = home else {
        return Ok(report_from_plan("apply", home, None, plan));
    };
    let rooted_home = rooted_home.context("resolved COVEN_HOME handle is unavailable")?;
    if plan.iter().all(|feature| feature.targets.is_empty()) {
        return Ok(report_from_plan("apply", &Some(home.clone()), None, plan));
    }
    let backup_root = Path::new(BACKUP_DIR).join(backup_id);
    rooted_home.ensure_private_dir(&backup_root)?;
    let mut report = report_from_plan(
        "apply",
        &Some(home.clone()),
        Some(Path::new(BACKUP_DIR).join(backup_id).display().to_string()),
        plan,
    );

    let mut transaction = ResetTransaction {
        version: 1,
        backup_id: backup_id.to_string(),
        moves: Vec::new(),
    };
    let mut rollback_failed = false;
    for (planned, feature_report) in plan.iter().zip(&mut report.features) {
        if planned.targets.is_empty() {
            continue;
        }
        let feature_backup = backup_root.join(planned.feature.name);
        let prepared = rooted_home
            .ensure_private_dir(&feature_backup)
            .and_then(|_| {
                for target in &planned.targets {
                    rooted_home.prepare_destination(&feature_backup.join(&target.relative))?;
                }
                Ok(())
            });
        if let Err(error) = prepared {
            feature_report.outcome = FeatureOutcome::Failed;
            feature_report.error = Some(format!("could not prepare local backup: {error}"));
            continue;
        }
        transaction
            .moves
            .extend(planned.targets.iter().map(|target| TransactionMove {
                source: target.relative.clone(),
                destination: feature_backup.join(&target.relative),
            }));
    }
    if !transaction.moves.is_empty() {
        rooted_home.write_transaction(&transaction)?;
    }

    for (planned, feature_report) in plan.iter().zip(&mut report.features) {
        if planned.targets.is_empty() || feature_report.outcome == FeatureOutcome::Failed {
            continue;
        }
        let feature_backup = backup_root.join(planned.feature.name);
        let mut failed = None;
        let mut moved = Vec::new();
        for target in &planned.targets {
            let destination = feature_backup.join(&target.relative);
            match rooted_home.rename_without_following(&target.relative, &destination) {
                Ok(()) => moved.push((target.relative.clone(), destination)),
                Err(error) => {
                    failed = Some(format!(
                        "could not back up `{}`: {error}",
                        target.relative.display()
                    ));
                    break;
                }
            }
        }
        if let Some(error) = failed {
            let rollback_errors = rollback_moves(rooted_home, &moved);
            rollback_failed |= !rollback_errors.is_empty();
            feature_report.outcome = FeatureOutcome::Failed;
            feature_report.error = Some(if rollback_errors.is_empty() {
                error
            } else {
                format!("{error}; rollback failed: {}", rollback_errors.join("; "))
            });
        } else {
            feature_report.outcome = FeatureOutcome::Reset;
            feature_report.moved_to_backup = planned
                .targets
                .iter()
                .map(|target| target.relative.display().to_string())
                .collect();
        }
    }
    if !transaction.moves.is_empty() && !rollback_failed {
        rooted_home.complete_transaction(&transaction)?;
    }
    Ok(report)
}

fn rollback_moves(home: &RootedHome, moved: &[(PathBuf, PathBuf)]) -> Vec<String> {
    let mut errors = Vec::new();
    for (source, destination) in moved.iter().rev() {
        if let Err(error) = home.rename_without_following(destination, source) {
            errors.push(format!("could not restore `{}`: {error}", source.display()));
        }
    }
    errors
}

fn backup_id() -> String {
    format!(
        "{}-{}",
        Utc::now()
            .to_rfc3339_opts(SecondsFormat::Secs, true)
            .replace(':', "-"),
        Uuid::new_v4().simple()
    )
}

fn render_report(report: &ResetReport, json: bool) -> Result<()> {
    if json {
        println!("{}", json_report(report)?);
        return Ok(());
    }

    let heading = if report.mode == "apply" {
        "Coven reset result"
    } else {
        "Coven reset preview (no state changed)"
    };
    println!("{heading}");
    println!("COVEN_HOME: {}", report.coven_home);
    for feature in &report.features {
        let sensitivity = match feature.sensitivity {
            Sensitivity::Standard => "standard",
            Sensitivity::Sensitive => "sensitive",
        };
        println!("- {} ({sensitivity}): {:?}", feature.name, feature.outcome);
        if feature.targets.is_empty() {
            println!("  state: none present");
        } else {
            println!("  state: {}", feature.targets.join(", "));
        }
        if !feature.moved_to_backup.is_empty() {
            println!("  backed up: {}", feature.moved_to_backup.join(", "));
        }
        println!("  note: {}", feature.warning);
        if let Some(error) = &feature.error {
            println!("  failed: {error}");
        }
    }
    if let Some(backup) = &report.backup {
        println!(
            "Recovery: move state from COVEN_HOME/{backup}/ back to its original feature path."
        );
    } else if report
        .features
        .iter()
        .all(|feature| feature.targets.is_empty())
    {
        println!("No selected local state is present.");
    } else {
        println!("Rerun with --apply to move selected state into a recoverable local backup.");
    }
    Ok(())
}

fn json_report(report: &ResetReport) -> Result<String> {
    let mut redacted = report.clone();
    if redacted.coven_home != "unavailable" {
        for feature in &mut redacted.features {
            if let Some(error) = &mut feature.error {
                *error = error.replace(&redacted.coven_home, "<coven-home>");
            }
        }
        redacted.coven_home = "<coven-home>".to_string();
    }
    serde_json::to_string(&redacted).context("failed to serialize reset report")
}

fn report_exit(report: &ResetReport) -> Result<()> {
    if report
        .features
        .iter()
        .any(|feature| feature.outcome == FeatureOutcome::Failed)
    {
        return Err(ResetError::new(
            ResetErrorKind::PartialFailure,
            "one or more selected reset features could not be completed; inspect the local backup report",
        )
        .into());
    }
    if report
        .features
        .iter()
        .all(|feature| feature.outcome == FeatureOutcome::Missing)
    {
        return Err(ResetError::new(
            ResetErrorKind::MissingState,
            "none of the selected reset features has local state to reset",
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(features: &[&str], all: bool, apply: bool) -> ResetRequest<'static> {
        ResetRequest {
            features: Box::leak(
                features
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
            all,
            apply,
            json: false,
        }
    }

    fn write_state(home: &Path, relative: &str, contents: &str) -> Result<()> {
        let path = home.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(home, fs::Permissions::from_mode(0o700))?;
        }
        fs::write(path, contents)?;
        Ok(())
    }

    #[cfg(unix)]
    fn protect_test_home(home: &Path) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(home, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    #[test]
    fn registry_lists_stable_feature_names() {
        let names: Vec<_> = RESET_FEATURES.iter().map(|feature| feature.name).collect();
        assert_eq!(
            names,
            vec![
                "familiars",
                "projects",
                "github",
                "claude",
                "openclaw",
                "hermes",
                "opencode",
                "grok-build",
                "gemini",
                "secrets",
                "caches",
                "sessions",
                "mobile",
                "metadata"
            ]
        );
    }

    #[test]
    fn preview_each_feature_never_moves_state() -> Result<()> {
        for feature in RESET_FEATURES {
            let temp = tempfile::tempdir()?;
            let home = temp.path().join("coven-home");
            write_state(&home, feature.paths[0], "fixture")?;
            let report = execute(home.clone(), request(&[feature.name], false, false))?;
            assert_eq!(report.features[0].outcome, FeatureOutcome::Preview);
            assert!(home.join(feature.paths[0]).exists(), "{}", feature.name);
        }
        Ok(())
    }

    #[test]
    fn all_requires_explicit_apply() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let error = execute(temp.path().join("coven-home"), request(&[], true, false)).unwrap_err();
        let error = error.downcast::<ResetError>()?;
        assert_eq!(error.exit_code(), 3);
        Ok(())
    }

    #[test]
    fn all_apply_expands_the_registered_feature_registry() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "repos.toml", "project")?;
        let report = execute(home.clone(), request(&[], true, true))?;
        assert_eq!(report.features.len(), RESET_FEATURES.len());
        assert!(report
            .features
            .iter()
            .any(|feature| feature.name == "projects" && feature.outcome == FeatureOutcome::Reset));
        assert!(!home.join("repos.toml").exists());
        assert!(report_exit(&report).is_ok());
        Ok(())
    }

    #[test]
    fn multiple_features_move_only_selected_state_to_backup() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "familiars.toml", "familiar")?;
        write_state(&home, "repos.toml", "project")?;
        write_state(&home, "coven.sqlite3", "keep-session")?;
        let report = execute(
            home.clone(),
            request(&["projects", "familiars"], false, true),
        )?;
        assert!(report
            .features
            .iter()
            .all(|feature| feature.outcome == FeatureOutcome::Reset));
        assert!(!home.join("familiars.toml").exists());
        assert!(!home.join("repos.toml").exists());
        assert!(home.join("coven.sqlite3").exists());
        let backup = report.backup.expect("apply creates a backup");
        assert_eq!(
            fs::read_to_string(home.join(backup).join("familiars/familiars.toml"))?,
            "familiar"
        );
        Ok(())
    }

    #[test]
    fn unknown_and_duplicate_features_are_rejected() -> Result<()> {
        let temp = tempfile::tempdir()?;
        for requested in [["unknown"].as_slice(), ["projects", "projects"].as_slice()] {
            let error = execute(
                temp.path().join("coven-home"),
                request(requested, false, false),
            )
            .unwrap_err();
            let error = error.downcast::<ResetError>()?;
            assert_eq!(error.exit_code(), 2);
        }
        Ok(())
    }

    #[test]
    fn missing_state_is_reported_with_its_documented_exit_code() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let report = execute(
            temp.path().join("coven-home"),
            request(&["projects"], false, false),
        )?;
        assert_eq!(report.features[0].outcome, FeatureOutcome::Missing);
        let error = report_exit(&report).unwrap_err().downcast::<ResetError>()?;
        assert_eq!(error.exit_code(), 4);
        Ok(())
    }

    #[test]
    fn independent_features_continue_after_a_backup_failure() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "familiars.toml", "familiar")?;
        write_state(&home, "repos.toml", "project")?;
        let selected = select_features(&["familiars".to_string(), "projects".to_string()], false)?;
        let rooted = RootedHome::open_existing(&home, true)?;
        let resolved = rooted.as_ref().map(|_| home.clone());
        let plan = build_plan(rooted.as_ref(), &selected)?;
        let blocked_backup = home.join(BACKUP_DIR).join("fixture").join("familiars");
        fs::create_dir_all(blocked_backup.parent().expect("backup parent"))?;
        fs::write(&blocked_backup, "not a directory")?;
        let report = apply_plan(&resolved, rooted.as_ref(), &plan, "fixture")?;
        assert!(report.features.iter().any(
            |feature| feature.name == "familiars" && feature.outcome == FeatureOutcome::Failed
        ));
        assert!(report
            .features
            .iter()
            .any(|feature| feature.name == "projects" && feature.outcome == FeatureOutcome::Reset));
        let error = report_exit(&report).unwrap_err().downcast::<ResetError>()?;
        assert_eq!(error.exit_code(), 5);
        Ok(())
    }

    #[test]
    fn project_reset_never_deletes_registered_source_directory() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        let project = temp.path().join("source-project");
        fs::create_dir_all(&project)?;
        write_state(
            &home,
            "repos.toml",
            &format!("[repos.demo]\npath = \"{}\"\n", project.display()),
        )?;
        execute(home.clone(), request(&["projects"], false, true))?;
        assert!(project.exists());
        assert!(!home.join("repos.toml").exists());
        Ok(())
    }

    #[test]
    fn sensitive_content_is_never_rendered_in_human_or_json_output() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "keys/session-artifacts.key", "super-secret-token")?;
        let report = execute(home.clone(), request(&["sessions"], false, false))?;
        let json = json_report(&report)?;
        assert!(!json.contains("super-secret-token"));
        assert!(!json.contains(&home.display().to_string()));
        assert!(json.contains("\"covenHome\":\"<coven-home>\""));
        assert!(!format!("{report:?}").contains("super-secret-token"));
        Ok(())
    }

    #[test]
    fn github_reset_only_handles_local_adapter_state() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "adapters/github.json", "local-only")?;
        let report = execute(home.clone(), request(&["github"], false, true))?;
        assert_eq!(report.features[0].outcome, FeatureOutcome::Reset);
        assert!(!home.join("adapters/github.json").exists());
        Ok(())
    }

    #[test]
    fn runtime_adapter_resets_are_independent_and_local() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "adapters/hermes.json", "hermes-local")?;
        write_state(&home, "adapters/gemini.json", "gemini-local")?;
        write_state(&home, "adapters/opencode.json", "keep-local")?;
        let report = execute(home.clone(), request(&["hermes", "gemini"], false, true))?;
        assert!(report
            .features
            .iter()
            .all(|feature| feature.outcome == FeatureOutcome::Reset));
        assert!(!home.join("adapters/hermes.json").exists());
        assert!(!home.join("adapters/gemini.json").exists());
        assert!(home.join("adapters/opencode.json").exists());
        Ok(())
    }

    #[test]
    fn unsafe_relative_paths_are_rejected() {
        for path in [
            Path::new("../outside"),
            Path::new("/outside"),
            Path::new(""),
        ] {
            let error = validate_relative_path(path)
                .unwrap_err()
                .downcast::<ResetError>()
                .unwrap();
            assert_eq!(error.exit_code(), 6);
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_state_is_refused_without_touching_the_target() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&home)?;
        protect_test_home(&home)?;
        fs::write(&outside, "outside-state")?;
        symlink(&outside, home.join("repos.toml"))?;
        let error = execute(home, request(&["projects"], false, false)).unwrap_err();
        let error = error.downcast::<ResetError>()?;
        assert_eq!(error.exit_code(), 6);
        assert_eq!(fs::read_to_string(outside)?, "outside-state");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_state_ancestor_is_refused_without_touching_the_target() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&home)?;
        protect_test_home(&home)?;
        fs::create_dir_all(&outside)?;
        fs::write(outside.join("github.json"), "outside-state")?;
        symlink(&outside, home.join("adapters"))?;

        let error = execute(home, request(&["github"], false, true)).unwrap_err();
        let error = error.downcast::<ResetError>()?;
        assert_eq!(error.exit_code(), 6);
        assert_eq!(
            fs::read_to_string(outside.join("github.json"))?,
            "outside-state"
        );
        Ok(())
    }

    #[test]
    fn stale_daemon_markers_do_not_block_reset() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "coven.sqlite3", "session")?;
        write_state(&home, "daemon.json", "{}")?;
        write_state(&home, "daemon.lock", "")?;
        write_state(&home, "daemon-serve.lock", "")?;
        let report = execute(home.clone(), request(&["sessions"], false, true))?;
        assert_eq!(report.features[0].outcome, FeatureOutcome::Reset);
        assert!(!home.join("coven.sqlite3").exists());
        Ok(())
    }

    #[test]
    fn active_coven_command_blocks_every_apply_category() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "repos.toml", "project")?;
        let _state_lock = crate::state_lock::acquire_shared(&home)?;
        let error = execute(home, request(&["projects"], false, true)).unwrap_err();
        let error = error.downcast::<ResetError>()?;
        assert_eq!(error.exit_code(), 3);
        Ok(())
    }

    #[test]
    fn legacy_daemon_serve_lock_blocks_apply() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "repos.toml", "project")?;
        let _serve_lock = crate::daemon::acquire_serve_lock(&home)?;
        let error = execute(home, request(&["projects"], false, true)).unwrap_err();
        let error = error.downcast::<ResetError>()?;
        assert_eq!(error.exit_code(), 3);
        Ok(())
    }

    #[test]
    fn daemon_lifecycle_contention_blocks_apply_without_waiting() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "repos.toml", "project")?;
        let lifecycle = crate::state_lock::open_lock_file(&home.join("daemon.lock"))?;
        fs2::FileExt::lock_exclusive(&lifecycle)?;
        let error = execute(home, request(&["projects"], false, true)).unwrap_err();
        let error = error.downcast::<ResetError>()?;
        assert_eq!(error.exit_code(), 3);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn any_process_accepting_on_legacy_daemon_socket_blocks_apply() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "repos.toml", "project")?;
        let _listener = std::os::unix::net::UnixListener::bind(home.join("coven.sock"))?;
        let error = execute(home, request(&["projects"], false, true)).unwrap_err();
        let error = error.downcast::<ResetError>()?;
        assert_eq!(error.exit_code(), 3);
        Ok(())
    }

    #[test]
    fn session_reset_keeps_artifact_key_with_encrypted_ledger() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "coven.sqlite3", "encrypted-session-record")?;
        write_state(&home, "keys/session-artifacts.key", "artifact-key")?;
        write_state(&home, "secrets/provider-ref", "keep-independent")?;

        let report = execute(home.clone(), request(&["sessions"], false, true))?;
        let backup = report.backup.expect("apply creates a backup");
        assert_eq!(
            fs::read_to_string(home.join(&backup).join("sessions/coven.sqlite3"))?,
            "encrypted-session-record"
        );
        assert_eq!(
            fs::read_to_string(
                home.join(&backup)
                    .join("sessions/keys/session-artifacts.key")
            )?,
            "artifact-key"
        );
        assert!(home.join("secrets/provider-ref").exists());
        Ok(())
    }

    #[test]
    fn interrupted_session_reset_blocks_commands_and_rolls_back_before_retry() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "coven.sqlite3", "encrypted-session-record")?;
        write_state(&home, "keys/session-artifacts.key", "artifact-key")?;
        let rooted = RootedHome::open_existing(&home, true)?.expect("opened home");
        let transaction = ResetTransaction {
            version: 1,
            backup_id: "interrupted".to_string(),
            moves: vec![
                TransactionMove {
                    source: PathBuf::from("coven.sqlite3"),
                    destination: PathBuf::from("reset-backups/interrupted/sessions/coven.sqlite3"),
                },
                TransactionMove {
                    source: PathBuf::from("keys/session-artifacts.key"),
                    destination: PathBuf::from(
                        "reset-backups/interrupted/sessions/keys/session-artifacts.key",
                    ),
                },
            ],
        };
        rooted.write_transaction(&transaction)?;
        rooted.rename_without_following(
            Path::new("coven.sqlite3"),
            Path::new("reset-backups/interrupted/sessions/coven.sqlite3"),
        )?;

        let blocked = match crate::state_lock::acquire_shared(&home) {
            Ok(_) => panic!("normal command should be blocked by interrupted reset"),
            Err(error) => error,
        };
        assert!(blocked
            .to_string()
            .contains("interrupted reset transaction"));

        let report = execute(home.clone(), request(&["sessions"], false, true))?;
        let backup = report.backup.expect("retry creates a fresh backup");
        assert_eq!(
            fs::read_to_string(home.join(&backup).join("sessions/coven.sqlite3"))?,
            "encrypted-session-record"
        );
        assert_eq!(
            fs::read_to_string(
                home.join(&backup)
                    .join("sessions/keys/session-artifacts.key")
            )?,
            "artifact-key"
        );
        assert!(!home
            .join(crate::state_lock::RESET_TRANSACTION_FILE)
            .exists());
        assert!(!home
            .join("reset-backups/interrupted/sessions/coven.sqlite3")
            .exists());
        Ok(())
    }

    #[test]
    fn mobile_reset_moves_credentials_as_one_category() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "mobile/gateway.json", "{}")?;
        write_state(&home, "mobile/devices.json", "[]")?;
        write_state(&home, "mobile/host-key.pem", "private-key")?;

        let report = execute(home.clone(), request(&["mobile"], false, true))?;
        let backup = report.backup.expect("apply creates a backup");
        assert!(!home.join("mobile").exists());
        assert!(home
            .join(backup)
            .join("mobile/mobile/host-key.pem")
            .exists());
        Ok(())
    }

    #[test]
    fn feature_failure_rolls_back_earlier_moves() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        write_state(&home, "familiars.toml", "registry")?;
        write_state(&home, "familiars/identity.md", "identity")?;
        let selected = select_features(&["familiars".to_string()], false)?;
        let rooted = RootedHome::open_existing(&home, true)?;
        let resolved = rooted.as_ref().map(|_| home.clone());
        let plan = build_plan(rooted.as_ref(), &selected)?;
        write_state(
            &home,
            "reset-backups/fixture/familiars/familiars",
            "collision",
        )?;

        let report = apply_plan(&resolved, rooted.as_ref(), &plan, "fixture")?;
        assert_eq!(report.features[0].outcome, FeatureOutcome::Failed);
        assert_eq!(fs::read_to_string(home.join("familiars.toml"))?, "registry");
        assert_eq!(
            fs::read_to_string(home.join("familiars/identity.md"))?,
            "identity"
        );
        assert!(report.features[0].moved_to_backup.is_empty());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_backup_root_is_refused_without_moving_state() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&home)?;
        protect_test_home(&home)?;
        fs::create_dir_all(&outside)?;
        write_state(&home, "repos.toml", "project")?;
        symlink(&outside, home.join(BACKUP_DIR))?;
        let selected = select_features(&["projects".to_string()], false)?;
        let rooted = RootedHome::open_existing(&home, true)?;
        let resolved = rooted.as_ref().map(|_| home.clone());
        let plan = build_plan(rooted.as_ref(), &selected)?;

        let error = apply_plan(&resolved, rooted.as_ref(), &plan, "fixture").unwrap_err();
        let error = error.downcast::<ResetError>()?;
        assert_eq!(error.exit_code(), 6);
        assert_eq!(fs::read_to_string(home.join("repos.toml"))?, "project");
        assert!(fs::read_dir(outside)?.next().is_none());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn opened_home_stays_anchored_after_path_replacement() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        let moved = temp.path().join("moved-home");
        let outside = temp.path().join("outside");
        write_state(&home, "repos.toml", "original")?;
        fs::create_dir_all(&outside)?;
        fs::write(outside.join("repos.toml"), "outside")?;

        let rooted = RootedHome::open_existing(&home, false)?.expect("opened home");
        fs::rename(&home, &moved)?;
        symlink(&outside, &home)?;

        let plan = build_plan(
            Some(&rooted),
            &select_features(&["projects".to_string()], false)?,
        )?;
        assert_eq!(plan[0].targets.len(), 1);
        assert_eq!(fs::read_to_string(moved.join("repos.toml"))?, "original");
        assert_eq!(fs::read_to_string(outside.join("repos.toml"))?, "outside");
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn symlinked_home_is_refused_without_touching_target() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside)?;
        fs::write(outside.join("repos.toml"), "outside")?;
        create_dir_symlink(&outside, &home)?;

        let error = match RootedHome::open_existing(&home, false) {
            Ok(_) => panic!("symlinked COVEN_HOME should be refused"),
            Err(error) => error,
        };
        let error = error.downcast::<ResetError>()?;
        assert_eq!(error.exit_code(), 6);
        assert_eq!(fs::read_to_string(outside.join("repos.toml"))?, "outside");
        Ok(())
    }

    #[test]
    fn current_directory_home_is_not_misclassified_as_a_filesystem_root() {
        if let Err(error) = RootedHome::open_existing(Path::new("."), false) {
            assert!(
                !error.to_string().contains("filesystem root"),
                "current-directory COVEN_HOME must not be treated as a root: {error}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_home_parent_is_refused_without_touching_target() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let real_parent = temp.path().join("real-parent");
        let linked_parent = temp.path().join("linked-parent");
        let real_home = real_parent.join("coven-home");
        fs::create_dir_all(&real_home)?;
        protect_test_home(&real_home)?;
        fs::write(real_home.join("repos.toml"), "outside")?;
        symlink(&real_parent, &linked_parent)?;

        let error = match RootedHome::open_existing(&linked_parent.join("coven-home"), false) {
            Ok(_) => panic!("symlinked COVEN_HOME parent should be refused"),
            Err(error) => error,
        };
        let error = error.downcast::<ResetError>()?;
        assert_eq!(error.exit_code(), 6);
        assert_eq!(fs::read_to_string(real_home.join("repos.toml"))?, "outside");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn writable_home_parent_is_refused() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir()?;
        let parent = temp.path().join("shared-parent");
        let home = parent.join("coven-home");
        fs::create_dir_all(&home)?;
        protect_test_home(&home)?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o777))?;

        let error = match RootedHome::open_existing(&home, false) {
            Ok(_) => panic!("other-writable COVEN_HOME parent should be refused"),
            Err(error) => error,
        };
        let error = error.downcast::<ResetError>()?;
        assert_eq!(error.exit_code(), 6);
        Ok(())
    }

    #[cfg(unix)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other("failed to create test junction"))
        }
    }
}
