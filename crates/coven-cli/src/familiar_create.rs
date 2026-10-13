//! `coven familiar create`: add a familiar to the roster without Cave
//! (coven#1282).
//!
//! The command appends one `[[familiar]]` entry to `familiars.toml` and seeds
//! the familiar's workspace with `SOUL.md` and `IDENTITY.md`, the two files
//! `coven familiar-ledger register` needs. It never rewrites or overwrites
//! anything that already exists:
//!
//! - the roster is changed under an exclusive lock, refused when it does not
//!   parse, and replaced atomically by its old bytes plus the new entry;
//! - workspace files are created only when absent.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Subcommand;
use serde::Serialize;
use serde_json::json;

use crate::{cockpit_sources, paths, state_lock};

/// Role written when `--role` is omitted or blank.
pub(crate) const DEFAULT_ROLE: &str = "Familiar";
/// Longest id derived from a display name. Explicit ids may use up to
/// [`ID_MAX_CHARS`].
const DERIVED_ID_MAX_CHARS: usize = 48;
const ID_MAX_CHARS: usize = 64;
/// Serializes roster writers. Cave does not take it yet; see
/// [`commit_roster`] for how a concurrent unlocked write is detected.
const ROSTER_LOCK_FILE: &str = "familiars.toml.lock";

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub(crate) enum FamiliarsCommand {
    #[command(about = "Add a familiar to familiars.toml and create its workspace")]
    #[command(
        long_about = "Add a familiar to familiars.toml and create its workspace.\n\nAppends one [[familiar]] entry (id, display_name, role, description, optional emoji, and an explicit workspace path) and creates <COVEN_HOME>/workspaces/familiars/<id>/ with starter SOUL.md and IDENTITY.md files. Existing roster bytes and existing workspace files are never changed."
    )]
    #[command(after_help = "Examples:
  coven familiar create Astra
  coven familiar create \"Night Owl\" --id owl --role Research --emoji 🦉
  coven familiar create Astra --json")]
    Create {
        #[arg(value_name = "DISPLAY_NAME", help = "Name shown for the familiar")]
        display_name: String,
        #[arg(
            long,
            value_name = "ID",
            help = "Familiar id (default: derived from the display name)"
        )]
        id: Option<String>,
        #[arg(long, value_name = "EMOJI", help = "Emoji shown next to the familiar")]
        emoji: Option<String>,
        #[arg(long, value_name = "ROLE", help = "Role [default: Familiar]")]
        role: Option<String>,
        #[arg(long, value_name = "TEXT", help = "One-line description")]
        description: Option<String>,
        #[arg(long, help = "Print the result as JSON (machine-readable)")]
        json: bool,
    },
}

/// What to create, as the user asked for it. Every field is validated by
/// [`create`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CreateRequest {
    pub(crate) display_name: String,
    pub(crate) id: Option<String>,
    pub(crate) emoji: Option<String>,
    pub(crate) role: Option<String>,
    pub(crate) description: Option<String>,
}

/// A familiar that was added to the roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Created {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) workspace: String,
    pub(crate) created: bool,
    #[serde(skip)]
    pub(crate) roster: PathBuf,
    /// Workspace files that already existed and were left as they were.
    #[serde(skip)]
    pub(crate) kept: Vec<&'static str>,
}

/// Why a familiar was not created. Nothing in the roster changed.
#[derive(Debug)]
pub(crate) enum CreateError {
    InvalidArgument(String),
    InvalidId(String),
    Exists(String),
    RosterInvalid(String),
    RosterChanged(String),
    Io(anyhow::Error),
}

impl CreateError {
    /// Stable machine-readable code for `--json` output.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::InvalidArgument(_) => "invalid_argument",
            Self::InvalidId(_) => "invalid_id",
            Self::Exists(_) => "familiar_exists",
            Self::RosterInvalid(_) => "roster_invalid",
            Self::RosterChanged(_) => "roster_changed",
            Self::Io(_) => "io_error",
        }
    }
}

impl fmt::Display for CreateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument(message)
            | Self::InvalidId(message)
            | Self::Exists(message)
            | Self::RosterInvalid(message)
            | Self::RosterChanged(message) => f.write_str(message),
            Self::Io(error) => write!(f, "{error:#}"),
        }
    }
}

impl std::error::Error for CreateError {}

impl From<anyhow::Error> for CreateError {
    fn from(error: anyhow::Error) -> Self {
        Self::Io(error)
    }
}

/// The error was already printed to stdout as JSON; `main` exits non-zero
/// without printing it again.
#[derive(Debug)]
pub(crate) struct ReportedAsJson;

impl fmt::Display for ReportedAsJson {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the error was reported as JSON")
    }
}

impl std::error::Error for ReportedAsJson {}

pub(crate) fn run(command: FamiliarsCommand) -> anyhow::Result<()> {
    let FamiliarsCommand::Create {
        display_name,
        id,
        emoji,
        role,
        description,
        json,
    } = command;
    let request = CreateRequest {
        display_name,
        id,
        emoji,
        role,
        description,
    };
    let outcome = paths::coven_home_dir()
        .map_err(CreateError::Io)
        .and_then(|coven_home| create(&coven_home, &request));
    match (outcome, json) {
        (Ok(created), true) => {
            println!("{}", serde_json::to_string_pretty(&created)?);
            Ok(())
        }
        (Ok(created), false) => {
            print!("{}", render(&created));
            Ok(())
        }
        (Err(error), true) => {
            let body = json!({
                "created": false,
                "error": { "code": error.code(), "message": error.to_string() },
            });
            println!("{}", serde_json::to_string_pretty(&body)?);
            Err(ReportedAsJson.into())
        }
        (Err(error), false) => Err(error.into()),
    }
}

/// Adds one familiar to `coven_home`'s roster and creates its workspace.
pub(crate) fn create(coven_home: &Path, request: &CreateRequest) -> Result<Created, CreateError> {
    let display_name = single_line("the display name", &request.display_name)?;
    if display_name.is_empty() {
        return Err(CreateError::InvalidArgument(
            "the display name must not be empty".to_owned(),
        ));
    }
    let role = match request.role.as_deref() {
        Some(role) => single_line("--role", role)?,
        None => String::new(),
    };
    let role = if role.is_empty() {
        DEFAULT_ROLE.to_owned()
    } else {
        role
    };
    let description = match request.description.as_deref() {
        Some(description) => single_line("--description", description)?,
        None => String::new(),
    };
    let emoji = match request.emoji.as_deref() {
        Some(emoji) => Some(single_line("--emoji", emoji)?).filter(|emoji| !emoji.is_empty()),
        None => None,
    };
    let id = match request.id.as_deref() {
        Some(id) => id.to_owned(),
        None => {
            let id = derive_id(&display_name);
            if id.is_empty() {
                return Err(CreateError::InvalidId(format!(
                    "cannot derive a familiar id from {display_name:?}; pass --id with 1-{ID_MAX_CHARS} characters from a-z, 0-9, `_` and `-`"
                )));
            }
            id
        }
    };
    validate_id(&id)?;

    fs::create_dir_all(coven_home)
        .with_context(|| format!("failed to create COVEN_HOME {}", coven_home.display()))?;
    let home = std::path::absolute(coven_home)
        .with_context(|| format!("failed to resolve COVEN_HOME {}", coven_home.display()))?;
    let workspace = home.join("workspaces").join("familiars").join(&id);
    let workspace_text = workspace
        .to_str()
        .ok_or_else(|| {
            CreateError::InvalidArgument(format!(
                "the workspace path {} is not UTF-8, so it cannot be written to familiars.toml; set COVEN_HOME to a UTF-8 path",
                workspace.display()
            ))
        })?
        .to_owned();

    let roster = home.join(cockpit_sources::FAMILIARS_CONFIG_FILE);
    let _lock = RosterLock::acquire(&home)?;
    let target = roster_target(&roster)?;
    let before = read_optional(&target)?;
    let before_text = match &before {
        Some(bytes) => std::str::from_utf8(bytes).map_err(|_| {
            CreateError::RosterInvalid(format!(
                "{} is not UTF-8 text; fix it, then retry. Nothing was changed.",
                roster.display()
            ))
        })?,
        None => "",
    };
    let entries = cockpit_sources::parse_familiar_entries(before_text).map_err(|error| {
        CreateError::RosterInvalid(format!(
            "{} does not parse, so no familiar was added; fix it, then retry. Nothing was changed.\n{error}",
            roster.display()
        ))
    })?;
    if entries.iter().any(|entry| entry.id == id) {
        return Err(CreateError::Exists(format!(
            "familiar `{id}` already exists in {}; choose another id with --id. Nothing was changed.",
            roster.display()
        )));
    }

    let entry = RosterEntry {
        id: &id,
        display_name: &display_name,
        role: &role,
        description: &description,
        emoji: emoji.as_deref(),
        workspace: &workspace_text,
    };
    let after_text = append_entry(before_text, &entry);
    let after = cockpit_sources::parse_familiar_entries(&after_text).map_err(|error| {
        CreateError::RosterInvalid(format!(
            "{} cannot take another [[familiar]] entry, so no familiar was added; fix it, then retry. Nothing was changed.\n{error}",
            roster.display()
        ))
    })?;
    let appended_once = after.len() == entries.len() + 1
        && after.iter().filter(|entry| entry.id == id).count() == 1
        && after.last().is_some_and(|entry| entry.id == id);
    if !appended_once {
        return Err(CreateError::RosterInvalid(format!(
            "{} does not keep familiars in one [[familiar]] list, so no familiar was added; fix it, then retry. Nothing was changed.",
            roster.display()
        )));
    }

    let kept = seed_workspace(&workspace, &entry)?;
    commit_roster(&roster, &target, before.as_deref(), after_text.as_bytes())?;

    Ok(Created {
        id,
        display_name,
        workspace: workspace_text,
        created: true,
        roster,
        kept,
    })
}

/// Derives an id from a display name: lowercase, every run of characters
/// outside `[a-z0-9]` becomes one `-`, dashes are trimmed from both ends, and
/// the result is cut to [`DERIVED_ID_MAX_CHARS`]. May return an empty string.
pub(crate) fn derive_id(display_name: &str) -> String {
    let mut id = String::new();
    let mut gap = false;
    for ch in display_name.to_lowercase().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if gap && !id.is_empty() {
                id.push('-');
            }
            gap = false;
            id.push(ch);
        } else {
            gap = true;
        }
    }
    // Every character is ASCII, so byte truncation is character truncation.
    id.truncate(DERIVED_ID_MAX_CHARS);
    id.trim_end_matches('-').to_owned()
}

/// Accepts ids matching `^[a-z0-9][a-z0-9_-]{0,63}$`.
pub(crate) fn validate_id(id: &str) -> Result<(), CreateError> {
    let mut chars = id.chars();
    let valid = chars
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && id.len() <= ID_MAX_CHARS
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-');
    if valid {
        Ok(())
    } else {
        Err(CreateError::InvalidId(format!(
            "familiar id {id:?} is invalid; use 1-{ID_MAX_CHARS} characters from a-z, 0-9, `_` and `-`, starting with a letter or digit"
        )))
    }
}

/// Trims `value` and refuses control characters, so every field stays one
/// line in the roster and in the workspace files.
fn single_line(field: &str, value: &str) -> Result<String, CreateError> {
    let value = value.trim();
    if value.chars().any(char::is_control) {
        return Err(CreateError::InvalidArgument(format!(
            "{field} must be one line of text without control characters"
        )));
    }
    Ok(value.to_owned())
}

struct RosterEntry<'a> {
    id: &'a str,
    display_name: &'a str,
    role: &'a str,
    description: &'a str,
    emoji: Option<&'a str>,
    workspace: &'a str,
}

/// `before` followed by a blank line and the new entry. Only basic strings
/// are written, one key per line, so simple line-based readers (Cave's
/// roster parser) read the entry too.
fn append_entry(before: &str, entry: &RosterEntry<'_>) -> String {
    let newline = if before.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut lines = vec![
        "[[familiar]]".to_owned(),
        format!("id = {}", toml_string(entry.id)),
        format!("display_name = {}", toml_string(entry.display_name)),
        format!("role = {}", toml_string(entry.role)),
        format!("description = {}", toml_string(entry.description)),
    ];
    if let Some(emoji) = entry.emoji {
        lines.push(format!("emoji = {}", toml_string(emoji)));
    }
    lines.push(format!("workspace = {}", toml_string(entry.workspace)));

    let mut out = String::with_capacity(before.len() + 256);
    out.push_str(before);
    if !before.is_empty() {
        if !before.ends_with('\n') {
            out.push_str(newline);
        }
        out.push_str(newline);
    }
    for line in lines {
        out.push_str(&line);
        out.push_str(newline);
    }
    out
}

/// A TOML basic string.
fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(ch))),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Creates the workspace and any missing starter files. Returns the files
/// that already existed and were left untouched.
fn seed_workspace(
    workspace: &Path,
    entry: &RosterEntry<'_>,
) -> Result<Vec<&'static str>, CreateError> {
    fs::create_dir_all(workspace)
        .with_context(|| format!("failed to create workspace {}", workspace.display()))?;
    let mut kept = Vec::new();
    for (name, contents) in [
        ("SOUL.md", soul_md(entry)),
        ("IDENTITY.md", identity_md(entry)),
    ] {
        let path = workspace.join(name);
        // `create_new` refuses an existing file or symlink, so nothing that
        // is already there is overwritten or followed.
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                kept.push(name);
                continue;
            }
            Err(error) => {
                return Err(anyhow::Error::new(error)
                    .context(format!("failed to create {}", path.display()))
                    .into());
            }
        };
        if let Err(error) = file
            .write_all(contents.as_bytes())
            .and_then(|()| file.sync_all())
        {
            // Remove the partial file this call created, so a retry writes
            // it whole instead of keeping it.
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(anyhow::Error::new(error)
                .context(format!("failed to write {}", path.display()))
                .into());
        }
    }
    Ok(kept)
}

fn soul_md(entry: &RosterEntry<'_>) -> String {
    let name = entry.display_name;
    let role = entry.role;
    let mut out = format!(
        "# SOUL.md - Who I Am\n\n## I am {name}\n\nI am {name}, a familiar in this Coven. My purpose is to help my person in my role: {role}.\n"
    );
    if !entry.description.is_empty() {
        out.push_str(&format!("\n{}\n", entry.description));
    }
    out.push_str(
        "\n## Core Work\n\n- The work my person brings to my role, within my lane.\n- Keeping my notes and memory current and honest.\n\n## What I Am Not\n\n- Not a replacement for another familiar's lane.\n\n## My Boundaries\n\n- I ask my person before anything irreversible.\n- I say when I do not know, and I never invent facts.\n",
    );
    out
}

fn identity_md(entry: &RosterEntry<'_>) -> String {
    let name = entry.display_name;
    let mut out = format!(
        "# IDENTITY.md - {name}\n\n- **Name:** {name}\n- **Creature:** Familiar\n- **Role:** {}\n",
        entry.role
    );
    if let Some(emoji) = entry.emoji {
        out.push_str(&format!("- **Emoji:** {emoji}\n"));
    }
    out.push_str("\n## Purpose\n\n");
    if entry.description.is_empty() {
        out.push_str(&format!("I help my person as their {}.\n", entry.role));
    } else {
        out.push_str(&format!("{}\n", entry.description));
    }
    out
}

/// Exclusive hold on [`ROSTER_LOCK_FILE`]; released on drop.
struct RosterLock(fs::File);

impl RosterLock {
    fn acquire(coven_home: &Path) -> Result<Self, CreateError> {
        let path = coven_home.join(ROSTER_LOCK_FILE);
        let file = state_lock::open_lock_file(&path)?;
        fs2::FileExt::lock_exclusive(&file)
            .with_context(|| format!("failed to lock {}", path.display()))?;
        Ok(Self(file))
    }
}

impl Drop for RosterLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

/// The file to replace. A symlinked roster is written through to its target,
/// so the link stays a link.
fn roster_target(roster: &Path) -> Result<PathBuf, CreateError> {
    match fs::symlink_metadata(roster) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(fs::canonicalize(roster)
            .with_context(|| format!("failed to resolve the symlink {}", roster.display()))?),
        Ok(_) => Ok(roster.to_path_buf()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(roster.to_path_buf()),
        Err(error) => Err(anyhow::Error::new(error)
            .context(format!("failed to inspect {}", roster.display()))
            .into()),
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, CreateError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(anyhow::Error::new(error)
            .context(format!("failed to read {}", path.display()))
            .into()),
    }
}

/// Replaces `target` with `after` through a staged sibling file and a rename.
///
/// Writers that skip the lock (Cave's summoning flow today) can still change
/// the roster after it was read. The roster is read again just before the
/// rename and the write is abandoned if it no longer matches `before`, which
/// narrows that window to the rename itself.
fn commit_roster(
    roster: &Path,
    target: &Path,
    before: Option<&[u8]>,
    after: &[u8],
) -> Result<(), CreateError> {
    let dir = target
        .parent()
        .with_context(|| format!("{} has no parent directory", target.display()))?;
    let mut staged = tempfile::Builder::new()
        .prefix(".familiars.toml.")
        .suffix(".staged")
        .tempfile_in(dir)
        .with_context(|| format!("failed to stage a new {}", roster.display()))?;
    staged
        .write_all(after)
        .and_then(|()| staged.as_file().sync_all())
        .with_context(|| format!("failed to write a new {}", roster.display()))?;
    // Keep the existing file's permissions. A new roster is private to its
    // owner, like the rest of COVEN_HOME.
    if before.is_some() {
        let permissions = fs::metadata(target)
            .with_context(|| format!("failed to inspect {}", roster.display()))?
            .permissions();
        fs::set_permissions(staged.path(), permissions)
            .with_context(|| format!("failed to set permissions for {}", roster.display()))?;
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(staged.path(), fs::Permissions::from_mode(0o600))
                .with_context(|| format!("failed to set permissions for {}", roster.display()))?;
        }
    }
    if read_optional(target)?.as_deref() != before {
        return Err(CreateError::RosterChanged(format!(
            "{} changed while the familiar was being added; nothing was changed, retry",
            roster.display()
        )));
    }
    staged
        .persist(target)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace {}", roster.display()))?;
    Ok(())
}

/// The human-readable result.
fn render(created: &Created) -> String {
    let mut out = format!(
        "Created familiar `{}` ({}).\n  roster     {}\n  workspace  {}\n",
        created.id,
        created.display_name,
        created.roster.display(),
        created.workspace
    );
    if !created.kept.is_empty() {
        out.push_str(&format!(
            "  kept       existing {}\n",
            created.kept.join(", ")
        ));
    }
    out.push_str(&format!(
        "\nNext: coven run <harness> --familiar {} \"<task>\"\n",
        created.id
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(display_name: &str) -> CreateRequest {
        CreateRequest {
            display_name: display_name.to_owned(),
            ..CreateRequest::default()
        }
    }

    fn roster(home: &Path) -> PathBuf {
        home.join("familiars.toml")
    }

    #[test]
    fn creates_the_roster_entry_and_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        let created = create(home, &request("Astra")).unwrap();

        let workspace = std::path::absolute(home)
            .unwrap()
            .join("workspaces")
            .join("familiars")
            .join("astra");
        assert_eq!(created.id, "astra");
        assert_eq!(created.display_name, "Astra");
        assert_eq!(created.workspace, workspace.to_str().unwrap());
        assert!(created.created);
        assert!(created.kept.is_empty());
        assert_eq!(
            fs::read_to_string(roster(home)).unwrap(),
            format!(
                "[[familiar]]\nid = \"astra\"\ndisplay_name = \"Astra\"\nrole = \"Familiar\"\ndescription = \"\"\nworkspace = {}\n",
                toml_string(&created.workspace)
            )
        );
        let soul = fs::read_to_string(workspace.join("SOUL.md")).unwrap();
        assert!(soul.contains("## I am Astra"), "{soul}");
        let identity = fs::read_to_string(workspace.join("IDENTITY.md")).unwrap();
        assert!(
            identity.starts_with("# IDENTITY.md - Astra\n"),
            "{identity}"
        );
        assert_eq!(
            serde_json::to_value(&created).unwrap(),
            json!({
                "id": "astra",
                "displayName": "Astra",
                "workspace": created.workspace,
                "created": true,
            })
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(roster(home)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn writes_every_optional_field_as_given() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        let created = create(
            home,
            &CreateRequest {
                display_name: "  Night \"Owl\"  ".to_owned(),
                id: Some("owl".to_owned()),
                emoji: Some("🦉".to_owned()),
                role: Some("Research".to_owned()),
                description: Some("Reads late into the night.".to_owned()),
            },
        )
        .unwrap();

        assert_eq!(created.id, "owl");
        assert_eq!(created.display_name, "Night \"Owl\"");
        let entries = cockpit_sources::read_familiars(home).expect("the written roster parses");
        assert_eq!(entries.len(), 1);
        let familiar = &entries[0];
        assert_eq!(familiar.id, "owl");
        assert_eq!(familiar.display_name, "Night \"Owl\"");
        assert_eq!(familiar.emoji, "🦉");
        assert_eq!(familiar.role, "Research");
        assert_eq!(familiar.description, "Reads late into the night.");
        assert_eq!(
            familiar.workspace.as_deref(),
            Some(Path::new(&created.workspace))
        );
        let identity =
            fs::read_to_string(Path::new(&created.workspace).join("IDENTITY.md")).unwrap();
        assert!(identity.contains("- **Role:** Research\n"), "{identity}");
        assert!(identity.contains("- **Emoji:** 🦉\n"), "{identity}");
        assert!(
            identity.contains("Reads late into the night."),
            "{identity}"
        );
    }

    #[test]
    fn derives_ids_from_display_names() {
        for (name, id) in [
            ("Astra", "astra"),
            ("Night Owl", "night-owl"),
            ("  --Hello__World--  ", "hello-world"),
            ("Agent 007!", "agent-007"),
            ("Ünïcode Ñame", "n-code-ame"),
            ("🐈", ""),
            ("---", ""),
        ] {
            assert_eq!(derive_id(name), id, "{name:?}");
        }
        assert_eq!(derive_id(&"a".repeat(60)), "a".repeat(48));
        // Cutting at 48 characters must not leave a trailing dash.
        assert_eq!(
            derive_id(&format!("{} {}", "a".repeat(47), "b".repeat(5))),
            "a".repeat(47)
        );
    }

    #[test]
    fn rejects_invalid_ids_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for id in [
            "",
            "Astra",
            "-astra",
            "_astra",
            "night owl",
            "a/b",
            "..",
            &"a".repeat(65),
        ] {
            let error = create(
                home,
                &CreateRequest {
                    id: Some(id.to_owned()),
                    ..request("Astra")
                },
            )
            .unwrap_err();
            assert_eq!(error.code(), "invalid_id", "{id:?}: {error}");
        }
        assert!(validate_id(&"a".repeat(64)).is_ok());
        assert!(validate_id("0_a-b").is_ok());

        let underivable = create(home, &request("🐈")).unwrap_err();
        assert_eq!(underivable.code(), "invalid_id");
        assert!(underivable.to_string().contains("--id"), "{underivable}");

        assert!(!roster(home).exists());
        assert!(!home.join("workspaces").exists());
    }

    #[test]
    fn rejects_blank_and_multiline_fields() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let blank = create(home, &request("   ")).unwrap_err();
        assert_eq!(blank.code(), "invalid_argument");
        let multiline = create(
            home,
            &CreateRequest {
                description: Some("one\nline".to_owned()),
                ..request("Astra")
            },
        )
        .unwrap_err();
        assert_eq!(multiline.code(), "invalid_argument");
        assert!(multiline.to_string().contains("--description"));
        assert!(!roster(home).exists());
    }

    #[test]
    fn refuses_a_duplicate_id_and_leaves_the_roster_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        create(home, &request("Astra")).unwrap();
        let before = fs::read(roster(home)).unwrap();

        let error = create(home, &request("astra")).unwrap_err();

        assert_eq!(error.code(), "familiar_exists");
        assert!(error.to_string().contains("`astra`"), "{error}");
        assert_eq!(fs::read(roster(home)).unwrap(), before);
    }

    #[test]
    fn refuses_a_roster_that_does_not_parse_and_leaves_it_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for broken in [
            "not = [valid".as_bytes(),
            b"[[familiar]]\nid = \"sage\"\n",
            b"[familiar]\nid = \"sage\"\n",
            &[0xff, 0xfe],
            // Parses, but appending a [[familiar]] table would not.
            b"familiar = []\n",
        ] {
            fs::write(roster(home), broken).unwrap();
            let error = create(home, &request("Astra")).unwrap_err();
            assert_eq!(error.code(), "roster_invalid", "{broken:?}: {error}");
            assert!(error.to_string().contains("Nothing was changed"));
            assert_eq!(fs::read(roster(home)).unwrap(), broken);
            assert!(!home.join("workspaces").exists());
        }
    }

    #[test]
    fn keeps_existing_roster_bytes_and_appends_after_them() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for (existing, separator) in [
            (
                "# My coven.\n[[familiar]]\nid = \"sage\"   # keep this\ndisplay_name = \"Sage\"\nrole = \"Research\"\ndescription = \"Finds things.\"\nharness = \"codex\"\n",
                "\n",
            ),
            (
                "[[familiar]]\nid = \"sage\"\ndisplay_name = \"Sage\"\nrole = \"Research\"\ndescription = \"\"",
                "\n\n",
            ),
            (
                "[[familiar]]\r\nid = \"sage\"\r\ndisplay_name = \"Sage\"\r\nrole = \"Research\"\r\ndescription = \"\"\r\n",
                "\r\n",
            ),
            ("# Only a comment.\n", "\n"),
        ] {
            fs::write(roster(home), existing).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(roster(home), fs::Permissions::from_mode(0o640)).unwrap();
            }

            create(home, &request("Astra")).unwrap();

            let after = fs::read_to_string(roster(home)).unwrap();
            let appended = after
                .strip_prefix(existing)
                .unwrap_or_else(|| panic!("existing bytes changed:\n{after}"));
            let block = appended
                .strip_prefix(separator)
                .unwrap_or_else(|| panic!("unexpected separator:\n{appended:?}"));
            assert!(block.starts_with("[[familiar]]"), "{block:?}");
            if existing.contains("\r\n") {
                assert!(!block.replace("\r\n", "").contains('\n'), "{block:?}");
            }
            let ids: Vec<String> = cockpit_sources::read_familiars(home)
                .unwrap()
                .into_iter()
                .map(|familiar| familiar.id)
                .collect();
            let expected: Vec<&str> = if existing.contains("sage") {
                vec!["sage", "astra"]
            } else {
                vec!["astra"]
            };
            assert_eq!(ids, expected);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(roster(home)).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o640, "permissions were not kept");
            }
            let leftovers: Vec<_> = fs::read_dir(home)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .filter(|name| name.to_string_lossy().ends_with(".staged"))
                .collect();
            assert!(leftovers.is_empty(), "{leftovers:?}");
            fs::remove_dir_all(home.join("workspaces")).unwrap();
        }
    }

    #[test]
    fn never_overwrites_existing_workspace_files() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let workspace = home.join("workspaces").join("familiars").join("astra");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("SOUL.md"), "# My own soul\n").unwrap();

        let created = create(home, &request("Astra")).unwrap();

        assert_eq!(created.kept, vec!["SOUL.md"]);
        assert_eq!(
            fs::read_to_string(workspace.join("SOUL.md")).unwrap(),
            "# My own soul\n"
        );
        assert!(workspace.join("IDENTITY.md").is_file());
        assert!(render(&created).contains("kept       existing SOUL.md"));
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_a_symlinked_roster() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir(&home).unwrap();
        let real = dir.path().join("dotfiles-familiars.toml");
        fs::write(&real, "# Kept elsewhere.\n").unwrap();
        std::os::unix::fs::symlink(&real, roster(&home)).unwrap();

        create(&home, &request("Astra")).unwrap();

        assert!(fs::symlink_metadata(roster(&home))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(fs::read_to_string(&real)
            .unwrap()
            .starts_with("# Kept elsewhere.\n\n[[familiar]]\nid = \"astra\"\n"));
    }

    #[test]
    fn the_new_familiar_resolves_for_runs_and_the_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        create(
            home,
            &CreateRequest {
                role: Some("Research".to_owned()),
                ..request("Astra")
            },
        )
        .unwrap();

        // `coven run --familiar astra` resolves through this function.
        let context = crate::familiar_identity::resolve_optional(home, Some("astra"))
            .unwrap()
            .expect("astra resolves");
        assert_eq!(context.id, "astra");
        assert_eq!(context.display_name, "Astra");
        assert_eq!(context.role.as_deref(), Some("Research"));

        // `coven familiar-ledger register astra` finds the roster entry and
        // both declaration files in the declared workspace.
        let declarations = crate::familiar_ledger::read_declarations(home, "astra").unwrap();
        assert!(declarations.is_ok(), "{declarations:?}");
    }
}
