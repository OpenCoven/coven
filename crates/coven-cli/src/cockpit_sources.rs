use std::collections::HashSet;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::ambient_authority;
#[cfg(windows)]
use cap_std::fs::MetadataExt;
#[cfg(unix)]
use cap_std::fs::OpenOptionsExt;
use cap_std::fs::{Dir, File, OpenOptions};
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

const FAMILIARS_CONFIG_FILE: &str = "familiars.toml";
const SKILLS_DIR: &str = "skills";
const MEMORY_DIR: &str = "memory";
const RESEARCH_TSV: &str = "research/results.tsv";
pub(crate) const MEMORY_CONTENT_MAX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct FamiliarDto {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub emoji: String,
    /// Optional glyph hint. Either a literal emoji char (`"🐈"`) or a
    /// Phosphor icon name (`"ph:cat-fill"`). Clients use this in preference
    /// to `emoji` when they have a richer icon system — see CovenCave's
    /// glyph picker. Omitted from the wire when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub role: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_channel: Option<String>,
    pub last_seen: String,
    pub active_sessions: u32,
    pub memory_freshness: String,
    /// Explicit workspace path declared in familiars.toml. `None` means
    /// the daemon uses the conventional `~/.coven/familiars/<id>/` path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<std::path::PathBuf>,
}

#[derive(Debug, Deserialize)]
struct FamiliarsFile {
    #[serde(default)]
    familiar: Vec<FamiliarEntry>,
}

#[derive(Debug, Deserialize)]
struct FamiliarEntry {
    id: String,
    name: Option<String>,
    display_name: String,
    emoji: Option<String>,
    /// See [`FamiliarDto::icon`]. Free-form string at this layer — the
    /// renderer decides whether to treat a `ph:` prefix as an icon vs.
    /// treat anything else as an emoji literal.
    icon: Option<String>,
    role: String,
    description: String,
    pronouns: Option<String>,
    active_channel: Option<String>,
    /// Explicit workspace path for this familiar. When set, the daemon uses this
    /// instead of the conventional `~/.coven/familiars/<id>/` path.
    /// Accepts `~` expansion. Optional — most familiars do not need to set this.
    workspace: Option<String>,
}

/// Resolve the on-disk workspace (familiar home) for a familiar.
///
/// Prefers the explicit `workspace` path declared in familiars.toml when
/// present; falls back to the conventional `~/.coven/familiars/<id>/` path.
pub(crate) fn familiar_workspace(coven_home: &Path, familiar_id: &str) -> std::path::PathBuf {
    read_familiars(coven_home)
        .ok()
        .and_then(|familiars| {
            familiars
                .into_iter()
                .find(|f| f.id == familiar_id)
                .and_then(|f| f.workspace)
        })
        .unwrap_or_else(|| coven_home.join("familiars").join(familiar_id))
}

pub fn read_familiars(coven_home: &Path) -> Result<Vec<FamiliarDto>> {
    let path = coven_home.join(FAMILIARS_CONFIG_FILE);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let parsed: FamiliarsFile =
        toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))?;
    let memory_root = coven_home.join(MEMORY_DIR);
    let mut out = Vec::with_capacity(parsed.familiar.len());
    for entry in parsed.familiar {
        let memory_dir = memory_root.join(&entry.id);
        let memory_freshness = latest_mtime(&memory_dir)
            .map(relative_time)
            .unwrap_or_else(|| "—".to_string());
        out.push(FamiliarDto {
            name: entry.name.unwrap_or_else(|| entry.id.clone()),
            display_name: entry.display_name,
            emoji: entry.emoji.unwrap_or_default(),
            icon: entry.icon.and_then(|s| {
                let trimmed = s.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            }),
            role: entry.role,
            description: entry.description,
            pronouns: entry.pronouns,
            status: "offline".to_string(),
            active_channel: entry.active_channel,
            last_seen: "—".to_string(),
            active_sessions: 0,
            memory_freshness,
            workspace: entry.workspace.map(|p| {
                // Expand leading ~ to home directory
                if let Some(rest) = p.strip_prefix("~/") {
                    dirs_next::home_dir()
                        .map(|home| home.join(rest))
                        .unwrap_or_else(|| std::path::PathBuf::from(&p))
                } else if p == "~" {
                    dirs_next::home_dir().unwrap_or_else(|| std::path::PathBuf::from(&p))
                } else {
                    std::path::PathBuf::from(p)
                }
            }),
            id: entry.id,
        });
    }
    Ok(out)
}

/// Outcome of a [`write_familiar_icon`] call.
#[derive(Debug, PartialEq, Eq)]
pub enum WriteFamiliarIconOutcome {
    /// The named familiar's `icon` field was updated (or inserted) in-place.
    Updated,
    /// The named familiar's `icon` field was removed because the new value
    /// was `None` or whitespace-only.
    Cleared,
    /// No `[[familiar]]` block in `familiars.toml` has a matching `id`.
    NotFound,
}

/// Update (or clear) a familiar's `icon` field in `~/.coven/familiars.toml`,
/// preserving the rest of the file's formatting + comments.
///
/// `icon = None` (or a whitespace-only `Some`) removes the field entirely.
/// `icon = Some("ph:cat-fill")` or `Some("🐈‍⬛")` either inserts or replaces
/// the value. Returns the [`WriteFamiliarIconOutcome`] so callers can map
/// `NotFound` → 404 without re-reading the file.
///
/// Writes are atomic via `tempfile + rename` inside the same directory so a
/// crash mid-write can never leave a half-written `familiars.toml`.
pub fn write_familiar_icon(
    coven_home: &Path,
    familiar_id: &str,
    icon: Option<&str>,
) -> Result<WriteFamiliarIconOutcome> {
    use toml_edit::{value, DocumentMut};

    let path = coven_home.join(FAMILIARS_CONFIG_FILE);
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut doc: DocumentMut = raw
        .parse()
        .with_context(|| format!("failed to parse {}", path.display()))?;

    // Normalize whitespace-only icons to None at the boundary so the file
    // never carries an empty glyph.
    let normalized: Option<&str> = icon.and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    });

    // `[[familiar]]` arrays of tables live under the top-level `familiar`
    // key as an `ArrayOfTables`. Scan for the table whose `id` matches.
    let array = match doc
        .get_mut("familiar")
        .and_then(|item| item.as_array_of_tables_mut())
    {
        Some(arr) => arr,
        None => return Ok(WriteFamiliarIconOutcome::NotFound),
    };

    let target = array.iter_mut().find(|tbl| {
        tbl.get("id")
            .and_then(|item| item.as_str())
            .map(|s| s == familiar_id)
            .unwrap_or(false)
    });

    let table = match target {
        Some(t) => t,
        None => return Ok(WriteFamiliarIconOutcome::NotFound),
    };

    let outcome = match normalized {
        Some(s) => {
            table["icon"] = value(s);
            WriteFamiliarIconOutcome::Updated
        }
        None => {
            if table.remove("icon").is_some() {
                WriteFamiliarIconOutcome::Cleared
            } else {
                // Nothing to clear, but still a successful no-op write.
                WriteFamiliarIconOutcome::Cleared
            }
        }
    };

    // Atomic write: write to a sibling tempfile in the same directory so the
    // subsequent `rename` is on the same filesystem and POSIX-atomic. A crash
    // mid-write can leave `.familiars.toml.tmp` behind but never a half-
    // written `familiars.toml`.
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_path = dir.join(".familiars.toml.tmp");
    fs::write(&tmp_path, doc.to_string())
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &path)
        .with_context(|| format!("failed to rename tempfile over {}", path.display()))?;

    Ok(outcome)
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillDto {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub category: String,
    pub tags: Vec<String>,
    pub score: f64,
    pub effective_rate: f64,
    pub applied_rate: f64,
    pub completion_rate: f64,
    pub fallback_rate: f64,
    pub version: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
struct SkillMetadata {
    name: String,
    description: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    category: Option<String>,
}

pub fn scan_skills(coven_home: &Path) -> Result<Vec<SkillDto>> {
    let root = coven_home.join(SKILLS_DIR);
    let entries = match fs::read_dir(&root) {
        Ok(it) => it,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", root.display()));
        }
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        let dir = entry.path();
        match fs::metadata(&dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => continue,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(err).with_context(|| format!("failed to inspect {}", dir.display()));
            }
        }
        let metadata_path = dir.join("metadata.json");
        let raw = match fs::read_to_string(&metadata_path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to read {}", metadata_path.display()));
            }
        };
        let meta: SkillMetadata = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", metadata_path.display()))?;
        let id = entry.file_name().to_string_lossy().into_owned();
        out.push(SkillDto {
            id,
            name: meta.name,
            owner: meta.author.unwrap_or_else(|| "unknown".to_string()),
            category: meta.category.unwrap_or_else(|| "general".to_string()),
            tags: meta.tags,
            score: 0.0,
            effective_rate: 0.0,
            applied_rate: 0.0,
            completion_rate: 0.0,
            fallback_rate: 0.0,
            version: meta.version.unwrap_or_else(|| "0.0.0".to_string()),
            description: meta.description,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryFileDto {
    pub id: String,
    pub familiar_id: String,
    pub title: String,
    pub path: String,
    pub updated_at: String,
    pub updated_at_iso: String,
    pub excerpt: String,
    pub source: MemorySourceDto,
    pub privacy_classification: Option<String>,
    pub reveal_required: Option<bool>,
    pub verification_state: String,
}

#[derive(Debug, Clone)]
struct MemoryRecord {
    id: String,
    familiar_id: String,
    title: String,
    file_name: String,
    relative_path: String,
    updated_at: String,
    updated_at_iso: String,
    source: MemorySourceDto,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemorySourceDto {
    pub kind: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryPrivacyDto {
    pub classification: Option<String>,
    pub reveal_required: Option<bool>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryVerificationDto {
    pub state: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemorySupersessionDto {
    pub supersedes: Option<String>,
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryDetailDto {
    pub id: String,
    pub familiar_id: String,
    pub title: String,
    pub updated_at: String,
    pub source: MemorySourceDto,
    pub content: String,
    pub content_format: String,
    pub privacy: MemoryPrivacyDto,
    pub verification: MemoryVerificationDto,
    pub attestation: Option<serde_json::Value>,
    pub supersession: MemorySupersessionDto,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryOverviewTotalsDto {
    pub entries: usize,
    pub familiars: usize,
    pub verified: usize,
    pub needs_review: usize,
    pub unknown: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryCapabilitiesDto {
    pub detail: bool,
    pub verification: bool,
    pub attestation_metadata: bool,
    pub supersession_history: bool,
    pub mutations: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryOverviewVerificationDto {
    pub state: String,
    pub checked_at: String,
    pub manifest: Option<String>,
    pub index: Option<String>,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryOverviewDto {
    pub generated_at: String,
    pub totals: MemoryOverviewTotalsDto,
    pub last_updated_at: Option<String>,
    pub capabilities: MemoryCapabilitiesDto,
    pub verification: MemoryOverviewVerificationDto,
}

const MEMORY_ID_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x88f4_153f_221e_4f51_9346_7f59_d9b2_8d57);

fn memory_id(relative_path: &str) -> String {
    uuid::Uuid::new_v5(&MEMORY_ID_NAMESPACE, relative_path.as_bytes()).to_string()
}

#[derive(Debug)]
pub(crate) enum MemoryContentError {
    TooLarge { max_bytes: u64 },
    InvalidUtf8,
    MissingOrUnsafe,
    Unavailable(io::Error),
}

impl fmt::Display for MemoryContentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { max_bytes } => {
                write!(formatter, "memory content exceeds {max_bytes}-byte limit")
            }
            Self::InvalidUtf8 => formatter.write_str("memory content is not valid UTF-8"),
            Self::MissingOrUnsafe => formatter.write_str("memory content is missing or unsafe"),
            Self::Unavailable(_) => formatter.write_str("memory content is unavailable"),
        }
    }
}

impl Error for MemoryContentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unavailable(error) => Some(error),
            Self::TooLarge { .. } | Self::InvalidUtf8 | Self::MissingOrUnsafe => None,
        }
    }
}

struct MemoryRoot {
    coven_home: PathBuf,
    dir: Dir,
}

impl MemoryRoot {
    fn open(coven_home: &Path) -> Result<Option<Self>> {
        let coven_dir = Dir::open_ambient_dir(coven_home, ambient_authority())
            .context("failed to open Coven home")?;
        let dir = match coven_dir.open_dir_nofollow(MEMORY_DIR) {
            Ok(dir) => dir,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .context("refusing to open memory root through a symlink or non-directory");
            }
        };
        let metadata = dir
            .dir_metadata()
            .context("failed to inspect opened memory root")?;
        if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
            anyhow::bail!("refusing to open memory root through a reparse point or non-directory");
        }
        Ok(Some(Self {
            coven_home: coven_home.to_path_buf(),
            dir,
        }))
    }

    fn open_familiar_dir(&self, familiar_id: &str) -> Result<Option<Dir>> {
        let metadata = match self.dir.symlink_metadata(familiar_id) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).context("failed to inspect a familiar memory directory");
            }
        };
        if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
            return Ok(None);
        }

        let dir = match self.dir.open_dir_nofollow(familiar_id) {
            Ok(dir) => dir,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(open_error) => {
                let current_metadata = match self.dir.symlink_metadata(familiar_id) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => {
                        return Err(error).context(
                            "failed to classify a familiar memory directory after an open failure",
                        );
                    }
                };
                if !current_metadata.is_dir()
                    || metadata_is_windows_reparse_point(&current_metadata)
                {
                    return Ok(None);
                }
                return Err(open_error).context("failed to open a familiar memory directory");
            }
        };

        let metadata = match dir.dir_metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).context("failed to inspect an opened familiar memory directory");
            }
        };
        if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
            return Ok(None);
        }
        Ok(Some(dir))
    }

    fn enumerate_metadata(&self) -> Result<Vec<MemoryRecord>> {
        let familiar_entries = self
            .dir
            .entries()
            .context("failed to enumerate memory root")?;
        let mut records = Vec::new();
        let mut seen_ids = HashSet::new();

        for familiar_entry in familiar_entries {
            let familiar_entry = match familiar_entry {
                Ok(entry) => entry,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).context("failed to enumerate a memory-root entry");
                }
            };
            let Some(familiar_id) = utf8_memory_name(familiar_entry.file_name()) else {
                continue;
            };
            let familiar_type = match familiar_entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).context("failed to inspect a memory-root entry");
                }
            };
            if !familiar_type.is_dir() {
                continue;
            }
            let Some(familiar_dir) = self.open_familiar_dir(&familiar_id)? else {
                continue;
            };
            let file_entries = match familiar_dir.entries() {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).context("failed to enumerate a familiar memory directory");
                }
            };

            for file_entry in file_entries {
                let file_entry = match file_entry {
                    Ok(entry) => entry,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(error).context("failed to enumerate a memory-file entry");
                    }
                };
                let Some(file_name) = utf8_memory_name(file_entry.file_name()) else {
                    continue;
                };
                let file_path = Path::new(&file_name);
                if file_path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    != Some("md")
                {
                    continue;
                }
                let is_regular_file = match file_entry.file_type() {
                    Ok(file_type) => file_type.is_file(),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(error).context("failed to inspect a memory-file entry type");
                    }
                };
                if !is_regular_file {
                    continue;
                }
                let Some(title) = file_path.file_stem().and_then(|stem| stem.to_str()) else {
                    continue;
                };
                let metadata = match familiar_dir.symlink_metadata(&file_name) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(error).context("failed to inspect memory-file metadata");
                    }
                };
                if !metadata.is_file() || metadata_is_windows_reparse_point(&metadata) {
                    continue;
                }
                let mut options = OpenOptions::new();
                options
                    .read(true)
                    .follow(FollowSymlinks::No)
                    .maybe_dir(true);
                #[cfg(unix)]
                options.custom_flags(libc::O_NONBLOCK);
                let mut file = match familiar_dir.open_with(&file_name, &options) {
                    Ok(file) => file,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(error).context("failed to open memory-file entry");
                    }
                };
                if crate::memory_import::opened_file_is_logically_restored(
                    &self.coven_home,
                    &familiar_id,
                    &file_name,
                    &mut file,
                ) {
                    continue;
                }
                let relative_path = format!("{familiar_id}/{file_name}");
                let id = reserve_memory_id(&mut seen_ids, memory_id(&relative_path))?;
                let modified = metadata.modified().ok().map(|modified| modified.into_std());
                let updated_at = modified
                    .map(relative_time)
                    .unwrap_or_else(|| "—".to_string());
                let modified_utc: chrono::DateTime<chrono::Utc> =
                    modified.unwrap_or(SystemTime::UNIX_EPOCH).into();
                let updated_at_iso =
                    modified_utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                records.push(MemoryRecord {
                    id,
                    familiar_id: familiar_id.clone(),
                    title: title.to_string(),
                    file_name,
                    relative_path,
                    updated_at,
                    updated_at_iso,
                    source: MemorySourceDto {
                        kind: "coven-origin".to_string(),
                        label: "Coven origin".to_string(),
                    },
                });
            }
        }

        records.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(records)
    }

    fn open_record(&self, record: &MemoryRecord) -> std::result::Result<File, MemoryContentError> {
        let familiar_dir = self
            .dir
            .open_dir_nofollow(&record.familiar_id)
            .map_err(classify_path_open_error)?;
        let familiar_metadata = familiar_dir
            .dir_metadata()
            .map_err(classify_opened_handle_error)?;
        if !familiar_metadata.is_dir() || metadata_is_windows_reparse_point(&familiar_metadata) {
            return Err(MemoryContentError::MissingOrUnsafe);
        }
        let mut options = OpenOptions::new();
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .maybe_dir(true);
        #[cfg(unix)]
        options.custom_flags(libc::O_NONBLOCK);
        let mut file = familiar_dir
            .open_with(&record.file_name, &options)
            .map_err(classify_path_open_error)?;
        let metadata = file.metadata().map_err(classify_opened_handle_error)?;
        if !metadata.is_file() || metadata_is_windows_reparse_point(&metadata) {
            return Err(MemoryContentError::MissingOrUnsafe);
        }
        if crate::memory_import::opened_file_is_logically_restored(
            &self.coven_home,
            &record.familiar_id,
            &record.file_name,
            &mut file,
        ) {
            return Err(MemoryContentError::MissingOrUnsafe);
        }
        file.seek(SeekFrom::Start(0))
            .map_err(classify_opened_handle_error)?;
        Ok(file)
    }
}

#[cfg(windows)]
fn windows_attributes_are_reparse_point(attributes: u32) -> bool {
    attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(windows)]
fn metadata_is_windows_reparse_point(metadata: &cap_std::fs::Metadata) -> bool {
    windows_attributes_are_reparse_point(metadata.file_attributes())
}

#[cfg(not(windows))]
fn metadata_is_windows_reparse_point(_metadata: &cap_std::fs::Metadata) -> bool {
    false
}

fn utf8_memory_name(name: OsString) -> Option<String> {
    name.into_string().ok()
}

fn reserve_memory_id(seen_ids: &mut HashSet<String>, id: String) -> Result<String> {
    if !seen_ids.insert(id.clone()) {
        anyhow::bail!("duplicate memory id generated");
    }
    Ok(id)
}

fn read_memory_content(file: &mut File) -> std::result::Result<String, MemoryContentError> {
    let metadata = file.metadata().map_err(MemoryContentError::Unavailable)?;
    if metadata.len() > MEMORY_CONTENT_MAX_BYTES {
        return Err(MemoryContentError::TooLarge {
            max_bytes: MEMORY_CONTENT_MAX_BYTES,
        });
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MEMORY_CONTENT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(MemoryContentError::Unavailable)?;
    if bytes.len() as u64 > MEMORY_CONTENT_MAX_BYTES {
        return Err(MemoryContentError::TooLarge {
            max_bytes: MEMORY_CONTENT_MAX_BYTES,
        });
    }
    String::from_utf8(bytes).map_err(|_| MemoryContentError::InvalidUtf8)
}

fn classify_path_open_error(error: io::Error) -> MemoryContentError {
    let missing_or_unsafe = matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory | io::ErrorKind::InvalidData
    ) || open_record_error_is_platform_symlink_loop(&error);

    if missing_or_unsafe {
        MemoryContentError::MissingOrUnsafe
    } else {
        MemoryContentError::Unavailable(error)
    }
}

fn classify_opened_handle_error(error: io::Error) -> MemoryContentError {
    MemoryContentError::Unavailable(error)
}

#[cfg(unix)]
fn open_record_error_is_platform_symlink_loop(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ELOOP)
}

#[cfg(windows)]
fn open_record_error_is_platform_symlink_loop(error: &io::Error) -> bool {
    error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_STOPPED_ON_SYMLINK as i32)
}

#[cfg(not(any(unix, windows)))]
fn open_record_error_is_platform_symlink_loop(_error: &io::Error) -> bool {
    false
}

fn read_record_content(
    root: &MemoryRoot,
    record: &MemoryRecord,
) -> std::result::Result<String, MemoryContentError> {
    let mut file = root.open_record(record)?;
    read_memory_content(&mut file)
}

pub fn scan_memory(coven_home: &Path) -> Result<Vec<MemoryFileDto>> {
    let Some(root) = MemoryRoot::open(coven_home)? else {
        return Ok(Vec::new());
    };
    let records = root.enumerate_metadata()?;
    let mut entries = Vec::with_capacity(records.len());
    for record in records {
        let excerpt = match read_record_content(&root, &record) {
            Ok(body) => first_paragraph(&body, 200),
            Err(_) => String::new(),
        };
        entries.push(MemoryFileDto {
            id: record.id,
            familiar_id: record.familiar_id,
            title: record.title,
            path: record.relative_path,
            updated_at: record.updated_at,
            updated_at_iso: record.updated_at_iso,
            excerpt,
            source: record.source,
            privacy_classification: None,
            reveal_required: None,
            verification_state: "unknown".to_string(),
        });
    }
    Ok(entries)
}

pub fn read_memory_detail(coven_home: &Path, id: &str) -> Result<Option<MemoryDetailDto>> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Ok(None);
    }
    let Some(root) = MemoryRoot::open(coven_home)? else {
        return Ok(None);
    };
    let record = root
        .enumerate_metadata()?
        .into_iter()
        .find(|record| record.id == id);
    let Some(record) = record else {
        return Ok(None);
    };
    let content = read_record_content(&root, &record)?;
    Ok(Some(MemoryDetailDto {
        id: record.id,
        familiar_id: record.familiar_id,
        title: record.title,
        updated_at: record.updated_at_iso,
        source: record.source,
        content,
        content_format: "markdown".to_string(),
        privacy: MemoryPrivacyDto {
            classification: None,
            reveal_required: None,
            reason: "privacy taxonomy unavailable".to_string(),
        },
        verification: MemoryVerificationDto {
            state: "unknown".to_string(),
            reason: "verification metadata unavailable".to_string(),
        },
        attestation: None,
        supersession: MemorySupersessionDto {
            supersedes: None,
            superseded_by: None,
        },
    }))
}

pub fn memory_overview(coven_home: &Path) -> Result<MemoryOverviewDto> {
    let records = match MemoryRoot::open(coven_home)? {
        Some(root) => root.enumerate_metadata()?,
        None => Vec::new(),
    };
    let familiars = records
        .iter()
        .map(|record| record.familiar_id.as_str())
        .collect::<HashSet<_>>()
        .len();
    let last_updated_at = records
        .iter()
        .map(|record| record.updated_at_iso.as_str())
        .max()
        .map(str::to_string);
    let generated_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    Ok(MemoryOverviewDto {
        generated_at: generated_at.clone(),
        totals: MemoryOverviewTotalsDto {
            entries: records.len(),
            familiars,
            verified: 0,
            needs_review: 0,
            unknown: records.len(),
        },
        last_updated_at,
        capabilities: MemoryCapabilitiesDto {
            detail: true,
            verification: false,
            attestation_metadata: false,
            supersession_history: false,
            mutations: false,
        },
        verification: MemoryOverviewVerificationDto {
            state: "unavailable".to_string(),
            checked_at: generated_at,
            manifest: None,
            index: None,
            issues: Vec::new(),
        },
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct ResearchRowDto {
    pub iteration: u32,
    pub topic: String,
    pub score: f64,
    pub delta: f64,
    pub decision: String,
    pub source: String,
}

pub fn read_research(coven_home: &Path) -> Result<Vec<ResearchRowDto>> {
    let path = coven_home.join(RESEARCH_TSV);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = trimmed.split('\t').collect();
        if cols.len() < 6 {
            continue;
        }
        // First non-numeric column means a header row — skip it.
        let Ok(iteration) = cols[0].parse::<u32>() else {
            continue;
        };
        out.push(ResearchRowDto {
            iteration,
            topic: cols[1].to_string(),
            score: cols[2].parse().unwrap_or(0.0),
            delta: cols[3].parse().unwrap_or(0.0),
            decision: cols[4].to_string(),
            source: cols[5].to_string(),
        });
    }
    Ok(out)
}

fn latest_mtime(dir: &Path) -> Option<SystemTime> {
    let entries = fs::read_dir(dir).ok()?;
    let mut latest: Option<SystemTime> = None;
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                latest = Some(latest.map_or(modified, |cur| cur.max(modified)));
            }
        }
    }
    latest
}

fn relative_time(then: SystemTime) -> String {
    let Ok(elapsed) = SystemTime::now().duration_since(then) else {
        return "future".to_string();
    };
    let secs = elapsed.as_secs();
    if secs < 60 {
        return "now".to_string();
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days < 30 {
        return format!("{days}d ago");
    }
    let months = days / 30;
    format!("{months}mo ago")
}

fn first_paragraph(body: &str, cap: usize) -> String {
    let mut buf = String::new();
    let mut in_frontmatter = false;
    let mut saw_frontmatter_open = false;
    for line in body.lines() {
        let trimmed = line.trim();
        // Skip a leading YAML frontmatter block (--- ... ---).
        if !saw_frontmatter_open && trimmed == "---" {
            in_frontmatter = true;
            saw_frontmatter_open = true;
            continue;
        }
        if in_frontmatter {
            if trimmed == "---" {
                in_frontmatter = false;
            }
            continue;
        }
        if trimmed.is_empty() {
            if !buf.is_empty() {
                break;
            }
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        if !buf.is_empty() {
            buf.push(' ');
        }
        buf.push_str(trimmed);
        if buf.len() >= cap {
            break;
        }
    }
    if buf.chars().count() > cap {
        buf = buf.chars().take(cap).collect::<String>();
        buf.push('…');
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn read_familiars_returns_empty_when_config_missing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(read_familiars(temp.path())?.is_empty());
        Ok(())
    }

    #[test]
    fn read_familiars_parses_toml_entries() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"
[[familiar]]
id = "sage"
display_name = "Sage"
emoji = "🌿"
role = "Research familiar"
description = "Reads, synthesizes."
pronouns = "they/them"
active_channel = "telegram"

[[familiar]]
id = "cody"
display_name = "Cody"
emoji = "⚡"
role = "Code"
description = "Builds and debugs."
"#,
        )?;
        let out = read_familiars(temp.path())?;
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "sage");
        assert_eq!(out[0].name, "sage");
        assert_eq!(out[0].display_name, "Sage");
        assert_eq!(out[0].emoji, "🌿");
        assert_eq!(out[0].pronouns.as_deref(), Some("they/them"));
        assert_eq!(out[0].active_channel.as_deref(), Some("telegram"));
        assert_eq!(out[0].status, "offline");
        assert_eq!(out[0].active_sessions, 0);
        assert_eq!(out[0].memory_freshness, "—");
        assert_eq!(out[1].id, "cody");
        assert!(out[1].pronouns.is_none());
        assert!(out[1].active_channel.is_none());
        // No `icon` field set in this fixture — must round-trip as None.
        assert!(out[0].icon.is_none());
        assert!(out[1].icon.is_none());
        Ok(())
    }

    #[test]
    fn read_familiars_carries_icon_field_for_both_shapes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"
[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "..."
icon = "ph:lightning-fill"

[[familiar]]
id = "kitten"
display_name = "Kitten"
role = "General"
description = "..."
icon = "🐈‍⬛"

[[familiar]]
id = "whitespace"
display_name = "Whitespace"
role = "Edge case"
description = "..."
icon = "   "

[[familiar]]
id = "no-icon"
display_name = "No icon"
role = "Edge case"
description = "..."
"#,
        )?;
        let out = read_familiars(temp.path())?;
        assert_eq!(out[0].icon.as_deref(), Some("ph:lightning-fill"));
        assert_eq!(out[1].icon.as_deref(), Some("🐈‍⬛"));
        // Whitespace-only icon must normalize to None so clients don't try to
        // render an empty glyph.
        assert!(
            out[2].icon.is_none(),
            "whitespace icon should normalize to None"
        );
        assert!(out[3].icon.is_none());
        Ok(())
    }

    #[test]
    fn familiar_dto_skips_serializing_absent_icon() -> Result<()> {
        let dto_without = FamiliarDto {
            id: "sage".to_string(),
            name: "sage".to_string(),
            display_name: "Sage".to_string(),
            emoji: "🌿".to_string(),
            icon: None,
            role: "Research".to_string(),
            description: "...".to_string(),
            pronouns: None,
            status: "offline".to_string(),
            active_channel: None,
            last_seen: "—".to_string(),
            active_sessions: 0,
            memory_freshness: "—".to_string(),
            workspace: None,
        };
        let json = serde_json::to_string(&dto_without)?;
        assert!(
            !json.contains("\"icon\""),
            "absent icon must not appear on the wire: {json}"
        );
        let dto_with = FamiliarDto {
            icon: Some("ph:cat-fill".to_string()),
            ..dto_without
        };
        let json = serde_json::to_string(&dto_with)?;
        assert!(json.contains("\"icon\":\"ph:cat-fill\""), "got {json}");
        Ok(())
    }

    #[test]
    fn write_familiar_icon_inserts_when_absent_and_preserves_other_fields() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"# top of file
[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "Builds and debugs."
# trailing comment
"#,
        )?;
        let outcome = write_familiar_icon(temp.path(), "cody", Some("ph:lightning-fill"))?;
        assert_eq!(outcome, WriteFamiliarIconOutcome::Updated);
        let raw = fs::read_to_string(temp.path().join(FAMILIARS_CONFIG_FILE))?;
        assert!(raw.contains("icon = \"ph:lightning-fill\""), "got {raw}");
        // Existing fields + comments must be preserved.
        assert!(raw.contains("display_name = \"Cody\""));
        assert!(raw.contains("# top of file"));
        assert!(raw.contains("# trailing comment"));
        // Round-trip through the reader.
        let read = read_familiars(temp.path())?;
        assert_eq!(read[0].icon.as_deref(), Some("ph:lightning-fill"));
        Ok(())
    }

    #[test]
    fn write_familiar_icon_replaces_existing_value() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "..."
icon = "ph:lightning-fill"
"#,
        )?;
        let outcome = write_familiar_icon(temp.path(), "cody", Some("🐈"))?;
        assert_eq!(outcome, WriteFamiliarIconOutcome::Updated);
        let raw = fs::read_to_string(temp.path().join(FAMILIARS_CONFIG_FILE))?;
        assert!(raw.contains("icon = \"🐈\""), "got {raw}");
        assert!(
            !raw.contains("ph:lightning-fill"),
            "old icon should be gone"
        );
        Ok(())
    }

    #[test]
    fn write_familiar_icon_clears_field_when_value_is_none() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "..."
icon = "ph:lightning-fill"
"#,
        )?;
        let outcome = write_familiar_icon(temp.path(), "cody", None)?;
        assert_eq!(outcome, WriteFamiliarIconOutcome::Cleared);
        let raw = fs::read_to_string(temp.path().join(FAMILIARS_CONFIG_FILE))?;
        assert!(!raw.contains("icon ="), "icon line must be removed: {raw}");
        let read = read_familiars(temp.path())?;
        assert!(read[0].icon.is_none());
        Ok(())
    }

    #[test]
    fn write_familiar_icon_treats_whitespace_as_clear() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "..."
icon = "ph:lightning-fill"
"#,
        )?;
        let outcome = write_familiar_icon(temp.path(), "cody", Some("   "))?;
        assert_eq!(outcome, WriteFamiliarIconOutcome::Cleared);
        let raw = fs::read_to_string(temp.path().join(FAMILIARS_CONFIG_FILE))?;
        assert!(!raw.contains("icon ="), "icon line must be removed: {raw}");
        Ok(())
    }

    #[test]
    fn write_familiar_icon_returns_not_found_for_unknown_id() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "..."
"#,
        )?;
        let outcome = write_familiar_icon(temp.path(), "ghost", Some("ph:ghost-fill"))?;
        assert_eq!(outcome, WriteFamiliarIconOutcome::NotFound);
        // File must be unchanged when not found.
        let raw = fs::read_to_string(temp.path().join(FAMILIARS_CONFIG_FILE))?;
        assert!(!raw.contains("ph:ghost-fill"));
        Ok(())
    }

    #[test]
    fn write_familiar_icon_leaves_no_tempfile_on_success() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "..."
"#,
        )?;
        write_familiar_icon(temp.path(), "cody", Some("ph:cat-fill"))?;
        let tmp_path = temp.path().join(".familiars.toml.tmp");
        assert!(!tmp_path.exists(), "atomic write left a tempfile behind");
        Ok(())
    }

    #[test]
    fn read_familiars_memory_freshness_reflects_recent_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            r#"
[[familiar]]
id = "sage"
display_name = "Sage"
role = "Research"
description = "..."
"#,
        )?;
        let sage_dir = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage_dir)?;
        fs::write(sage_dir.join("note.md"), "hi")?;
        let out = read_familiars(temp.path())?;
        assert_ne!(out[0].memory_freshness, "—");
        Ok(())
    }

    #[test]
    fn scan_skills_returns_empty_when_dir_missing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(scan_skills(temp.path())?.is_empty());
        Ok(())
    }

    #[test]
    fn scan_skills_parses_metadata_per_subdir_and_skips_subdirs_without_metadata() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let skills_root = temp.path().join(SKILLS_DIR);
        let alpha = skills_root.join("alpha");
        let beta = skills_root.join("beta");
        let gamma_no_meta = skills_root.join("gamma");
        fs::create_dir_all(&alpha)?;
        fs::create_dir_all(&beta)?;
        fs::create_dir_all(&gamma_no_meta)?;
        fs::write(
            alpha.join("metadata.json"),
            r#"{"name":"Alpha","description":"A","version":"1.0.0","author":"sage","tags":["x"],"category":"research"}"#,
        )?;
        fs::write(
            beta.join("metadata.json"),
            r#"{"name":"Beta","description":"B"}"#,
        )?;
        let out = scan_skills(temp.path())?;
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "alpha");
        assert_eq!(out[0].name, "Alpha");
        assert_eq!(out[0].owner, "sage");
        assert_eq!(out[0].version, "1.0.0");
        assert_eq!(out[0].tags, vec!["x"]);
        assert_eq!(out[0].category, "research");
        assert_eq!(out[0].score, 0.0);
        assert_eq!(out[1].id, "beta");
        assert_eq!(out[1].owner, "unknown");
        assert_eq!(out[1].version, "0.0.0");
        assert_eq!(out[1].category, "general");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn scan_skills_follows_symlinked_skill_dirs() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let canonical = temp.path().join("canonical").join("delta");
        fs::create_dir_all(&canonical)?;
        fs::write(
            canonical.join("metadata.json"),
            r#"{"name":"Delta","description":"D","author":"coven","category":"operations"}"#,
        )?;

        let skills_root = temp.path().join(SKILLS_DIR);
        fs::create_dir_all(&skills_root)?;
        std::os::unix::fs::symlink(&canonical, skills_root.join("delta"))?;

        let out = scan_skills(temp.path())?;

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "delta");
        assert_eq!(out[0].name, "Delta");
        assert_eq!(out[0].owner, "coven");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn scan_skills_skips_dangling_symlinks() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let skills_root = temp.path().join(SKILLS_DIR);
        fs::create_dir_all(&skills_root)?;
        std::os::unix::fs::symlink(
            temp.path().join("missing-skill"),
            skills_root.join("missing-skill"),
        )?;

        let out = scan_skills(temp.path())?;

        assert!(out.is_empty());
        Ok(())
    }

    #[test]
    fn scan_memory_returns_empty_when_dir_missing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(scan_memory(temp.path())?.is_empty());
        Ok(())
    }

    #[test]
    fn scan_memory_groups_md_files_by_familiar_with_excerpts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        let echo = temp.path().join(MEMORY_DIR).join("echo");
        fs::create_dir_all(&sage)?;
        fs::create_dir_all(&echo)?;
        fs::write(
            sage.join("notes.md"),
            "# Title\n\nFirst paragraph about research synthesis.\n\nSecond paragraph ignored.",
        )?;
        fs::write(sage.join("ignored.txt"), "not a markdown file")?;
        fs::write(
            echo.join("reflections.md"),
            "---\nfrontmatter: skipped\n---\n\nReflection excerpt body.",
        )?;
        let out = scan_memory(temp.path())?;
        assert_eq!(out.len(), 2);
        // sorted by path
        assert_eq!(out[0].familiar_id, "echo");
        assert_eq!(out[0].path, "echo/reflections.md");
        assert_eq!(out[0].excerpt, "Reflection excerpt body.");
        assert_eq!(out[1].familiar_id, "sage");
        assert_eq!(out[1].path, "sage/notes.md");
        assert!(out[1].excerpt.starts_with("First paragraph"));
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn opened_memory_record_rechecks_logical_restore_state() -> Result<()> {
        let test_root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/memory-reader-tests");
        fs::create_dir_all(&test_root)?;
        let test_root = fs::canonicalize(test_root)?;
        let temp = tempfile::Builder::new()
            .prefix("reader")
            .tempdir_in(test_root)?;
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace)?;
        fs::write(
            temp.path().join(FAMILIARS_CONFIG_FILE),
            format!(
                "[[familiar]]\nid = \"sage\"\ndisplay_name = \"Sage\"\nrole = \"test\"\ndescription = \"test\"\nworkspace = {}\n",
                serde_json::to_string(&workspace.to_string_lossy())?
            ),
        )?;
        let sources = vec![crate::memory_import::DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"restored content".to_vec(),
        }];
        let plan = crate::memory_import::build_import_plan(
            temp.path(),
            "sage",
            crate::memory_import::MemoryImportSourceKind::Native,
            &sources,
        )?;
        crate::memory_import::apply_import_plan(temp.path(), &plan, &sources)?;
        let root = MemoryRoot::open(temp.path())?.expect("memory root exists");
        let record = root
            .enumerate_metadata()?
            .into_iter()
            .next()
            .expect("applied memory is visible");
        crate::memory_import::restore_import_bundle_for_test(temp.path(), "sage", &plan.bundle_id)?;

        let error = read_record_content(&root, &record)
            .expect_err("restored memory must not be read from a stale record");

        assert!(matches!(error, MemoryContentError::MissingOrUnsafe));
        Ok(())
    }

    #[test]
    fn scan_memory_returns_stable_opaque_ids() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        fs::write(sage.join("notes.md"), "Durable fact.")?;

        let first = scan_memory(temp.path())?;
        let second = scan_memory(temp.path())?;

        assert_eq!(first.len(), 1);
        assert_eq!(first[0].id, second[0].id);
        assert!(uuid::Uuid::parse_str(&first[0].id).is_ok());
        assert!(!first[0].id.contains("sage"));
        assert!(!first[0].id.contains("notes"));
        Ok(())
    }

    #[test]
    fn memory_id_matches_pinned_vector_across_roots() -> Result<()> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        for root in [first.path(), second.path()] {
            let sage = root.join(MEMORY_DIR).join("sage");
            fs::create_dir_all(&sage)?;
            fs::write(sage.join("notes.md"), "Durable fact.")?;
        }

        assert_eq!(
            memory_id("sage/notes.md"),
            "98ef2809-6bc3-5309-add6-0f39d676b52f"
        );
        assert_eq!(
            scan_memory(first.path())?.remove(0).id,
            scan_memory(second.path())?.remove(0).id
        );
        Ok(())
    }

    #[test]
    fn memory_overview_does_not_read_invalid_utf8_bodies() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        fs::write(sage.join("valid.md"), "Durable fact.")?;
        fs::write(sage.join("corrupt.md"), [0xff, 0xfe])?;

        let overview = memory_overview(temp.path())?;

        assert_eq!(overview.totals.entries, 2);
        assert_eq!(overview.totals.unknown, 2);
        Ok(())
    }

    #[test]
    fn memory_detail_reads_only_the_selected_entry() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        fs::write(sage.join("valid.md"), "Durable fact.")?;
        fs::write(sage.join("corrupt.md"), [0xff, 0xfe])?;

        let detail = read_memory_detail(temp.path(), &memory_id("sage/valid.md"))?
            .expect("valid memory detail");

        assert_eq!(detail.content, "Durable fact.");
        Ok(())
    }

    #[test]
    fn unknown_memory_detail_does_not_read_a_corrupt_neighbor() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        fs::write(sage.join("corrupt.md"), [0xff, 0xfe])?;

        let detail = read_memory_detail(temp.path(), "00000000-0000-0000-0000-000000000000")?;

        assert!(detail.is_none());
        Ok(())
    }

    #[test]
    fn open_record_error_classification_separates_missing_or_unsafe_from_unavailable() {
        for kind in [
            io::ErrorKind::NotFound,
            io::ErrorKind::NotADirectory,
            io::ErrorKind::InvalidData,
        ] {
            assert!(
                matches!(
                    classify_path_open_error(io::Error::new(kind, "test error")),
                    MemoryContentError::MissingOrUnsafe
                ),
                "{kind:?} must be treated as missing or unsafe"
            );
        }

        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::Other] {
            match classify_path_open_error(io::Error::new(kind, "test error")) {
                MemoryContentError::Unavailable(error) => assert_eq!(error.kind(), kind),
                error => panic!("{kind:?} must remain unavailable, got {error:?}"),
            }
        }
    }

    #[test]
    fn post_open_error_classification_keeps_not_found_and_invalid_data_unavailable() {
        for kind in [io::ErrorKind::NotFound, io::ErrorKind::InvalidData] {
            match classify_opened_handle_error(io::Error::new(kind, "post-open test error")) {
                MemoryContentError::Unavailable(error) => assert_eq!(error.kind(), kind),
                error => panic!("{kind:?} after open must remain unavailable, got {error:?}"),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn open_record_error_classification_distinguishes_eloop_from_eio() {
        assert!(matches!(
            classify_path_open_error(io::Error::from_raw_os_error(libc::ELOOP)),
            MemoryContentError::MissingOrUnsafe
        ));

        match classify_path_open_error(io::Error::from_raw_os_error(libc::EIO)) {
            MemoryContentError::Unavailable(error) => {
                assert_eq!(error.raw_os_error(), Some(libc::EIO));
            }
            error => panic!("EIO must remain unavailable, got {error:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn open_record_error_classification_treats_stopped_on_symlink_as_missing_or_unsafe() {
        use windows_sys::Win32::Foundation::ERROR_STOPPED_ON_SYMLINK;

        assert!(matches!(
            classify_path_open_error(io::Error::from_raw_os_error(
                ERROR_STOPPED_ON_SYMLINK as i32
            )),
            MemoryContentError::MissingOrUnsafe
        ));
    }

    #[cfg(unix)]
    #[test]
    fn memory_scan_propagates_permission_denied_familiar_directory() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        struct PermissionRestore {
            path: std::path::PathBuf,
            permissions: Option<fs::Permissions>,
        }

        impl PermissionRestore {
            fn restore(mut self) -> io::Result<()> {
                let permissions = self.permissions.as_ref().expect("permissions").clone();
                fs::set_permissions(&self.path, permissions)?;
                self.permissions = None;
                Ok(())
            }
        }

        impl Drop for PermissionRestore {
            fn drop(&mut self) {
                if let Some(permissions) = self.permissions.take() {
                    let _ = fs::set_permissions(&self.path, permissions);
                }
            }
        }

        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        fs::write(sage.join("notes.md"), "private")?;
        let restore = PermissionRestore {
            path: sage.clone(),
            permissions: Some(fs::metadata(&sage)?.permissions()),
        };
        fs::set_permissions(&sage, fs::Permissions::from_mode(0o000))?;

        let result = scan_memory(temp.path());
        restore.restore()?;
        let error = result.expect_err("permission denial must fail the scan");

        assert!(error.chain().any(|cause| {
            cause
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::PermissionDenied)
        }));
        assert!(!format!("{error:#}").contains(temp.path().to_string_lossy().as_ref()));
        Ok(())
    }

    #[test]
    fn memory_list_keeps_a_corrupt_entry_without_losing_valid_neighbors() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        fs::write(sage.join("valid.md"), "Durable fact.")?;
        fs::write(sage.join("corrupt.md"), [0xff, 0xfe])?;

        let entries = scan_memory(temp.path())?;

        assert_eq!(entries.len(), 2);
        let valid = entries
            .iter()
            .find(|entry| entry.path == "sage/valid.md")
            .expect("valid row");
        let corrupt = entries
            .iter()
            .find(|entry| entry.path == "sage/corrupt.md")
            .expect("corrupt row");
        assert_eq!(valid.excerpt, "Durable fact.");
        assert_eq!(corrupt.excerpt, "");
        Ok(())
    }

    #[test]
    fn memory_list_keeps_an_oversize_entry_without_losing_valid_neighbors() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        fs::write(sage.join("valid.md"), "Durable fact.")?;
        fs::write(
            sage.join("large.md"),
            vec![b'x'; MEMORY_CONTENT_MAX_BYTES as usize + 1],
        )?;

        let entries = scan_memory(temp.path())?;

        assert_eq!(entries.len(), 2);
        let valid = entries
            .iter()
            .find(|entry| entry.path == "sage/valid.md")
            .expect("valid row");
        let large = entries
            .iter()
            .find(|entry| entry.path == "sage/large.md")
            .expect("oversize row");
        assert_eq!(valid.excerpt, "Durable fact.");
        assert_eq!(large.excerpt, "");
        Ok(())
    }

    #[test]
    fn memory_detail_rejects_entries_over_four_mibibytes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        fs::write(
            sage.join("large.md"),
            vec![b'x'; MEMORY_CONTENT_MAX_BYTES as usize + 1],
        )?;

        let error = read_memory_detail(temp.path(), &memory_id("sage/large.md"))
            .expect_err("oversize detail must fail closed");

        assert!(
            error
                .to_string()
                .contains("memory content exceeds 4194304-byte limit"),
            "unexpected error: {error:#}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn validated_memory_handle_is_the_handle_that_gets_read() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        let path = sage.join("notes.md");
        fs::write(&path, "validated bytes")?;

        let root = MemoryRoot::open(temp.path())?.expect("memory root");
        let record = root.enumerate_metadata()?.remove(0);
        let mut handle = root.open_record(&record)?;

        fs::rename(&path, sage.join("original.md"))?;
        fs::write(&path, "replacement bytes")?;

        assert_eq!(read_memory_content(&mut handle)?, "validated bytes");
        Ok(())
    }

    #[test]
    fn open_record_rejects_a_non_regular_replacement() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        let path = sage.join("notes.md");
        fs::write(&path, "validated bytes")?;

        let root = MemoryRoot::open(temp.path())?.expect("memory root");
        let record = root.enumerate_metadata()?.remove(0);

        fs::remove_file(&path)?;
        fs::create_dir(&path)?;

        assert!(matches!(
            read_record_content(&root, &record),
            Err(MemoryContentError::MissingOrUnsafe)
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn read_record_content_rejects_a_fifo_replacement_without_blocking() -> Result<()> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::sync::mpsc::RecvTimeoutError;
        use std::time::Duration;

        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        let path = sage.join("notes.md");
        fs::write(&path, "validated bytes")?;

        let root = MemoryRoot::open(temp.path())?.expect("memory root");
        let record = root.enumerate_metadata()?.remove(0);

        fs::remove_file(&path)?;
        let c_path = CString::new(path.as_os_str().as_bytes())?;
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        if rc != 0 {
            return Err(io::Error::last_os_error().into());
        }

        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = sender.send(read_record_content(&root, &record));
        });

        let result = match receiver.recv_timeout(Duration::from_millis(500)) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                let writer = fs::OpenOptions::new().write(true).open(&path)?;
                drop(writer);
                let late_result = receiver
                    .recv_timeout(Duration::from_secs(2))
                    .map_err(|error| anyhow::anyhow!("FIFO worker did not finish: {error}"))?;
                worker
                    .join()
                    .map_err(|_| anyhow::anyhow!("FIFO worker panicked"))?;
                panic!("memory FIFO open blocked before validation; late result: {late_result:?}");
            }
            Err(RecvTimeoutError::Disconnected) => {
                worker
                    .join()
                    .map_err(|_| anyhow::anyhow!("FIFO worker panicked"))?;
                panic!("FIFO worker disconnected without a result");
            }
        };
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("FIFO worker panicked"))?;

        assert!(matches!(result, Err(MemoryContentError::MissingOrUnsafe)));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn open_record_rejects_an_external_symlink_replacement_after_enumeration() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        let path = sage.join("notes.md");
        fs::write(&path, "validated bytes")?;
        let outside_file = outside.path().join("outside.md");
        fs::write(&outside_file, "outside private bytes")?;

        let root = MemoryRoot::open(temp.path())?.expect("memory root");
        let record = root.enumerate_metadata()?.remove(0);

        fs::remove_file(&path)?;
        symlink(&outside_file, &path)?;

        assert!(matches!(
            read_record_content(&root, &record),
            Err(MemoryContentError::MissingOrUnsafe)
        ));
        Ok(())
    }

    #[test]
    fn duplicate_memory_ids_fail_closed() {
        let mut seen = std::collections::HashSet::new();
        assert!(reserve_memory_id(&mut seen, "duplicate".to_string()).is_ok());
        let error = reserve_memory_id(&mut seen, "duplicate".to_string())
            .expect_err("duplicate ids must fail closed");
        assert!(error.to_string().contains("duplicate memory id"));
    }

    #[cfg(unix)]
    #[test]
    fn distinct_non_utf8_familiar_names_are_rejected_without_aliasing() {
        use std::os::unix::ffi::OsStringExt;

        let names = [
            OsString::from_vec(vec![b'f', 0x80]),
            OsString::from_vec(vec![b'f', 0x81]),
        ];

        assert!(names
            .into_iter()
            .all(|name| utf8_memory_name(name).is_none()));
    }

    #[cfg(unix)]
    #[test]
    fn distinct_non_utf8_file_names_are_rejected_without_aliasing() {
        use std::os::unix::ffi::OsStringExt;

        let names = [
            OsString::from_vec(vec![b'n', 0x80, b'.', b'm', b'd']),
            OsString::from_vec(vec![b'n', 0x81, b'.', b'm', b'd']),
        ];

        assert!(names
            .into_iter()
            .all(|name| utf8_memory_name(name).is_none()));
    }

    #[cfg(windows)]
    #[test]
    fn distinct_ill_formed_utf16_familiar_names_are_rejected_without_aliasing() {
        use std::os::windows::ffi::OsStringExt;

        let names = [
            OsString::from_wide(&[b'f' as u16, 0xd800]),
            OsString::from_wide(&[b'f' as u16, 0xd801]),
        ];

        assert!(names
            .into_iter()
            .all(|name| utf8_memory_name(name).is_none()));
    }

    #[cfg(windows)]
    #[test]
    fn distinct_ill_formed_utf16_file_names_are_rejected_without_aliasing() {
        use std::os::windows::ffi::OsStringExt;

        let names = [
            OsString::from_wide(&[b'n' as u16, 0xd800, b'.' as u16, b'm' as u16, b'd' as u16]),
            OsString::from_wide(&[b'n' as u16, 0xd801, b'.' as u16, b'm' as u16, b'd' as u16]),
        ];

        assert!(names
            .into_iter()
            .all(|name| utf8_memory_name(name).is_none()));
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_attribute_gate_rejects_every_reparse_point() {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        };

        assert!(!windows_attributes_are_reparse_point(0));
        assert!(windows_attributes_are_reparse_point(
            FILE_ATTRIBUTE_REPARSE_POINT
        ));
        assert!(windows_attributes_are_reparse_point(
            FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn scan_memory_skips_distinct_non_utf8_familiar_names_without_aliasing() -> Result<()> {
        use std::os::unix::ffi::OsStringExt;

        let temp = tempfile::tempdir()?;
        let root = temp.path().join(MEMORY_DIR);
        fs::create_dir_all(&root)?;
        for name in [
            OsString::from_vec(vec![b'f', 0x80]),
            OsString::from_vec(vec![b'f', 0x81]),
        ] {
            let familiar = root.join(name);
            fs::create_dir_all(&familiar)?;
            fs::write(familiar.join("notes.md"), "private")?;
        }

        assert!(scan_memory(temp.path())?.is_empty());
        assert_eq!(memory_overview(temp.path())?.totals.entries, 0);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn scan_memory_skips_distinct_non_utf8_file_names_without_aliasing() -> Result<()> {
        use std::os::unix::ffi::OsStringExt;

        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        for name in [
            OsString::from_vec(vec![b'n', 0x80, b'.', b'm', b'd']),
            OsString::from_vec(vec![b'n', 0x81, b'.', b'm', b'd']),
        ] {
            fs::write(sage.join(name), "private")?;
        }

        assert!(scan_memory(temp.path())?.is_empty());
        assert_eq!(memory_overview(temp.path())?.totals.entries, 0);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn scan_memory_skips_distinct_ill_formed_utf16_familiar_names_without_aliasing() -> Result<()> {
        use std::os::windows::ffi::OsStringExt;

        let temp = tempfile::tempdir()?;
        let root = temp.path().join(MEMORY_DIR);
        fs::create_dir_all(&root)?;
        for name in [
            OsString::from_wide(&[b'f' as u16, 0xd800]),
            OsString::from_wide(&[b'f' as u16, 0xd801]),
        ] {
            let familiar = root.join(name);
            fs::create_dir_all(&familiar)?;
            fs::write(familiar.join("notes.md"), "private")?;
        }

        assert!(scan_memory(temp.path())?.is_empty());
        assert_eq!(memory_overview(temp.path())?.totals.entries, 0);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn scan_memory_skips_distinct_ill_formed_utf16_file_names_without_aliasing() -> Result<()> {
        use std::os::windows::ffi::OsStringExt;

        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        for name in [
            OsString::from_wide(&[b'n' as u16, 0xd800, b'.' as u16, b'm' as u16, b'd' as u16]),
            OsString::from_wide(&[b'n' as u16, 0xd801, b'.' as u16, b'm' as u16, b'd' as u16]),
        ] {
            fs::write(sage.join(name), "private")?;
        }

        assert!(scan_memory(temp.path())?.is_empty());
        assert_eq!(memory_overview(temp.path())?.totals.entries, 0);
        Ok(())
    }

    #[test]
    fn scan_memory_exposes_machine_readable_unknown_metadata() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        fs::write(sage.join("notes.md"), "Durable fact.")?;

        let entry = scan_memory(temp.path())?.remove(0);

        assert!(entry.updated_at_iso.ends_with('Z'));
        assert!(chrono::DateTime::parse_from_rfc3339(&entry.updated_at_iso).is_ok());
        assert_eq!(entry.privacy_classification, None);
        assert_eq!(entry.reveal_required, None);
        assert_eq!(entry.verification_state, "unknown");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn scan_memory_skips_symlinked_markdown_files() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        let outside = temp.path().join("outside.md");
        fs::write(&outside, "private outside content")?;
        symlink(&outside, sage.join("leak.md"))?;

        assert!(scan_memory(temp.path())?.is_empty());
        assert_eq!(memory_overview(temp.path())?.totals.entries, 0);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn scan_memory_rejects_a_symlinked_memory_root() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let sage = outside.path().join("sage");
        fs::create_dir_all(&sage)?;
        fs::write(sage.join("leak.md"), "private outside content")?;
        symlink(outside.path(), temp.path().join(MEMORY_DIR))?;

        let error = scan_memory(temp.path()).expect_err("symlinked root must fail closed");

        assert!(format!("{error:#}").contains("symlink"));
        Ok(())
    }

    #[test]
    fn read_memory_detail_returns_content_without_a_filesystem_path() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        fs::write(sage.join("notes.md"), "# Notes\n\nDurable fact.")?;
        let id = scan_memory(temp.path())?.remove(0).id;

        let detail = read_memory_detail(temp.path(), &id)?.expect("memory detail");

        assert_eq!(detail.id, id);
        assert_eq!(detail.content, "# Notes\n\nDurable fact.");
        assert_eq!(detail.privacy.classification, None);
        assert_eq!(detail.privacy.reveal_required, None);
        assert_eq!(detail.verification.state, "unknown");
        let json = serde_json::to_value(&detail)?;
        assert!(json.get("path").is_none());
        assert!(!json
            .to_string()
            .contains(temp.path().to_string_lossy().as_ref()));
        Ok(())
    }

    #[test]
    fn memory_overview_reports_unsupported_capabilities_honestly() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sage = temp.path().join(MEMORY_DIR).join("sage");
        fs::create_dir_all(&sage)?;
        fs::write(sage.join("one.md"), "one")?;
        fs::write(sage.join("two.md"), "two")?;

        let overview = memory_overview(temp.path())?;

        assert_eq!(overview.totals.entries, 2);
        assert_eq!(overview.totals.familiars, 1);
        assert_eq!(overview.totals.verified, 0);
        assert_eq!(overview.totals.needs_review, 0);
        assert_eq!(overview.totals.unknown, 2);
        assert!(overview.capabilities.detail);
        assert!(!overview.capabilities.verification);
        assert!(!overview.capabilities.attestation_metadata);
        assert!(!overview.capabilities.supersession_history);
        assert!(!overview.capabilities.mutations);
        assert_eq!(overview.verification.state, "unavailable");
        assert_eq!(overview.verification.manifest, None);
        assert_eq!(overview.verification.index, None);
        Ok(())
    }

    #[test]
    fn read_research_returns_empty_when_file_missing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(read_research(temp.path())?.is_empty());
        Ok(())
    }

    #[test]
    fn read_research_parses_tsv_and_skips_header_and_blank_rows() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let research_dir = temp.path().join("research");
        fs::create_dir_all(&research_dir)?;
        let body = "iteration\ttopic\tscore\tdelta\tdecision\tsource\n\
                    1\tHarness landscape\t0.61\t0.00\taccepted\tweb research\n\
                    \n\
                    # comment line\n\
                    2\tEval awareness\t0.68\t0.07\twatch\tpaper synthesis\n";
        fs::write(research_dir.join("results.tsv"), body)?;
        let out = read_research(temp.path())?;
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].iteration, 1);
        assert_eq!(out[0].topic, "Harness landscape");
        assert_eq!(out[0].score, 0.61);
        assert_eq!(out[0].decision, "accepted");
        assert_eq!(out[1].iteration, 2);
        assert_eq!(out[1].decision, "watch");
        Ok(())
    }

    #[test]
    fn first_paragraph_truncates_long_bodies_with_ellipsis() {
        let body = "x".repeat(500);
        let excerpt = first_paragraph(&body, 100);
        assert_eq!(excerpt.chars().count(), 101);
        assert!(excerpt.ends_with('…'));
    }
}
