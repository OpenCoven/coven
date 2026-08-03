#![cfg_attr(windows, allow(dead_code))]

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::ambient_authority;
#[cfg(any(unix, windows))]
use cap_std::fs::MetadataExt;
#[cfg(unix)]
use cap_std::fs::OpenOptionsExt;
use cap_std::fs::{Dir, OpenOptions};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

const MAX_SOURCE_FILES: usize = 256;
const MAX_AGGREGATE_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SOURCE_TRAVERSAL_DEPTH: usize = 32;
const MAX_SOURCE_VISITED_ENTRIES: usize = 1024;
const MAX_SOURCE_VISITED_DIRECTORIES: usize = 256;
#[cfg(any(windows, test))]
const WINDOWS_FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const SOURCE_FILE_LIMIT_ERROR: &str = "source discovery exceeds maximum file count";
const SOURCE_BYTE_LIMIT_ERROR: &str = "source discovery exceeds maximum aggregate bytes";
const SOURCE_DEPTH_LIMIT_ERROR: &str = "source discovery exceeds maximum traversal depth";
const SOURCE_ENTRY_LIMIT_ERROR: &str = "source discovery exceeds maximum visited entry count";
const SOURCE_DIRECTORY_LIMIT_ERROR: &str = "source discovery exceeds maximum directory count";
#[cfg(target_vendor = "apple")]
const RESTORED_XATTR: &str = "com.opencoven.memory-import-restored-v1";
#[cfg(all(unix, not(target_vendor = "apple")))]
const RESTORED_XATTR: &str = "user.coven.memory-import-restored-v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MemoryImportSourceKind {
    Native,
    Openclaw,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImportPlanStatus {
    Preview,
    Conflict,
    Verified,
    Restored,
    RolledBack,
    ManualRecovery,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlanEntryStatus {
    Create,
    Unchanged,
    Restored,
    Conflict,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct PlanEntry {
    #[serde(rename = "source_label")]
    pub(crate) logical_label: String,
    pub(crate) target_name: String,
    pub(crate) digest: String,
    pub(crate) status: PlanEntryStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ImportPlan {
    pub(crate) familiar_id: String,
    pub(crate) source_kind: MemoryImportSourceKind,
    pub(crate) bundle_id: String,
    pub(crate) status: ImportPlanStatus,
    #[serde(skip)]
    pub(crate) apply_eligible: bool,
    pub(crate) file_count: usize,
    #[serde(rename = "created_count")]
    pub(crate) create_count: usize,
    pub(crate) unchanged_count: usize,
    pub(crate) restored_count: usize,
    pub(crate) conflict_count: usize,
    pub(crate) entries: Vec<PlanEntry>,
}

#[derive(Clone, Copy)]
pub(crate) struct DiscoverSourcesRequest<'a> {
    pub(crate) familiar: &'a str,
    pub(crate) source: MemoryImportSourceKind,
    pub(crate) openclaw_root: Option<&'a Path>,
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct DiscoveredSource {
    pub(crate) source_label: String,
    pub(crate) bytes: Vec<u8>,
}

impl fmt::Debug for DiscoveredSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiscoveredSource")
            .field("source_label", &self.source_label)
            .field("byte_len", &self.bytes.len())
            .finish()
    }
}

trait SourceAdapter {
    fn discover(&self) -> Result<Vec<DiscoveredSource>>;
}

struct NativeSourceAdapter {
    workspace: PathBuf,
}

struct OpenClawSourceAdapter {
    root: PathBuf,
}

enum SourceAdapterKind {
    Native(NativeSourceAdapter),
    OpenClaw(OpenClawSourceAdapter),
}

impl SourceAdapter for SourceAdapterKind {
    fn discover(&self) -> Result<Vec<DiscoveredSource>> {
        match self {
            Self::Native(adapter) => adapter.discover(),
            Self::OpenClaw(adapter) => adapter.discover(),
        }
    }
}

impl SourceAdapter for NativeSourceAdapter {
    fn discover(&self) -> Result<Vec<DiscoveredSource>> {
        let root = SourceRoot::open(&self.workspace)?;
        root.discover(&["MEMORY.md"], &["memory", "notes"])
    }
}

impl SourceAdapter for OpenClawSourceAdapter {
    fn discover(&self) -> Result<Vec<DiscoveredSource>> {
        let root = SourceRoot::open(&self.root)?;
        root.discover(&["MEMORY.md", "DREAMS.md"], &["memory"])
    }
}

struct SourceRoot {
    dir: Dir,
}

#[derive(Default)]
struct DiscoveryBudget {
    source_files: usize,
    aggregate_bytes: u64,
    visited_entries: usize,
    visited_directories: usize,
}

impl DiscoveryBudget {
    fn claim_entry(&mut self) -> Result<()> {
        if self.visited_entries >= MAX_SOURCE_VISITED_ENTRIES {
            bail!(SOURCE_ENTRY_LIMIT_ERROR);
        }
        self.visited_entries += 1;
        Ok(())
    }

    fn claim_directory(&mut self) -> Result<()> {
        if self.visited_directories >= MAX_SOURCE_VISITED_DIRECTORIES {
            bail!(SOURCE_DIRECTORY_LIMIT_ERROR);
        }
        self.visited_directories += 1;
        Ok(())
    }

    fn claim_file(&mut self) -> Result<()> {
        if self.source_files >= MAX_SOURCE_FILES {
            bail!(SOURCE_FILE_LIMIT_ERROR);
        }
        self.source_files += 1;
        Ok(())
    }

    fn ensure_bytes_available(&self, bytes: u64) -> Result<()> {
        if self
            .aggregate_bytes
            .checked_add(bytes)
            .is_none_or(|total| total > MAX_AGGREGATE_SOURCE_BYTES)
        {
            bail!(SOURCE_BYTE_LIMIT_ERROR);
        }
        Ok(())
    }

    fn charge_bytes(&mut self, bytes: u64) -> Result<()> {
        self.ensure_bytes_available(bytes)?;
        self.aggregate_bytes += bytes;
        Ok(())
    }
}

impl SourceRoot {
    fn open(path: &Path) -> Result<Self> {
        let dir = open_dir_path_nofollow(path)?;
        let metadata = dir
            .dir_metadata()
            .map_err(|_| anyhow!("source root is unavailable"))?;
        if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
            bail!("source root must be a real directory");
        }
        Ok(Self { dir })
    }

    fn discover(
        &self,
        root_files: &[&str],
        allowed_directories: &[&str],
    ) -> Result<Vec<DiscoveredSource>> {
        let mut discovered = Vec::new();
        let mut budget = DiscoveryBudget::default();
        for root_file in root_files {
            if let Some(source) = read_allowed_file(&self.dir, root_file, root_file, &mut budget)? {
                discovered.push(source);
            }
        }
        for directory_name in allowed_directories {
            let Some(directory) = open_optional_real_directory(&self.dir, directory_name)? else {
                continue;
            };
            discover_markdown_tree(&directory, directory_name, 0, &mut discovered, &mut budget)?;
        }
        discovered.sort_by(|left, right| left.source_label.cmp(&right.source_label));
        Ok(discovered)
    }
}

pub(crate) fn discover_sources(
    coven_home: &Path,
    request: DiscoverSourcesRequest<'_>,
) -> Result<Vec<DiscoveredSource>> {
    let registered = crate::cockpit_sources::read_familiars(coven_home)
        .map_err(|_| anyhow!("unable to read familiar registry"))?
        .into_iter()
        .any(|familiar| familiar.id == request.familiar);
    if !registered {
        bail!("unknown familiar `{}`", request.familiar);
    }

    let adapter = match request.source {
        MemoryImportSourceKind::Native => {
            if request.openclaw_root.is_some() {
                bail!("native discovery does not accept an OpenClaw root");
            }
            SourceAdapterKind::Native(NativeSourceAdapter {
                workspace: crate::cockpit_sources::familiar_workspace(coven_home, request.familiar),
            })
        }
        MemoryImportSourceKind::Openclaw => {
            let root = request
                .openclaw_root
                .ok_or_else(|| anyhow!("OpenClaw discovery requires an explicit OpenClaw root"))?;
            SourceAdapterKind::OpenClaw(OpenClawSourceAdapter {
                root: root.to_path_buf(),
            })
        }
    };
    adapter.discover()
}

fn open_dir_path_nofollow(path: &Path) -> Result<Dir> {
    let (anchor, components) = split_source_root_from_trusted_anchor(path)?;
    let mut directory = Dir::open_ambient_dir(anchor, ambient_authority())
        .map_err(|_| anyhow!("source root is unavailable"))?;
    let anchor_metadata = directory
        .dir_metadata()
        .map_err(|_| anyhow!("source root is unavailable"))?;
    if !anchor_metadata.is_dir() || metadata_is_windows_reparse_point(&anchor_metadata) {
        bail!("source root must be a real directory");
    }

    for component in components {
        let metadata = directory
            .symlink_metadata(&component)
            .map_err(|_| anyhow!("source root is unavailable"))?;
        if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
            bail!("source root must be a real directory");
        }
        directory = directory
            .open_dir_nofollow(&component)
            .map_err(|_| anyhow!("source root must be a real directory"))?;
        let opened_metadata = directory
            .dir_metadata()
            .map_err(|_| anyhow!("source root is unavailable"))?;
        if !opened_metadata.is_dir() || metadata_is_windows_reparse_point(&opened_metadata) {
            bail!("source root must be a real directory");
        }
    }
    Ok(directory)
}

fn split_source_root_from_trusted_anchor(path: &Path) -> Result<(PathBuf, Vec<OsString>)> {
    if path.as_os_str().is_empty() {
        bail!("source root is unavailable");
    }
    if path.is_absolute() {
        let mut anchor = PathBuf::new();
        let mut components = Vec::new();
        let mut found_root = false;
        for component in path.components() {
            match component {
                Component::Prefix(prefix) if !found_root => anchor.push(prefix.as_os_str()),
                Component::RootDir if !found_root => {
                    anchor.push(component.as_os_str());
                    found_root = true;
                }
                Component::Normal(name) if found_root => components.push(name.to_os_string()),
                Component::CurDir => {}
                _ => bail!("source root must be a real directory"),
            }
        }
        if !found_root {
            bail!("source root is unavailable");
        }
        return Ok((anchor, components));
    }

    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => components.push(name.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("source root must be a real directory");
            }
        }
    }
    Ok((PathBuf::from("."), components))
}

fn open_optional_real_directory(parent: &Dir, name: &str) -> Result<Option<Dir>> {
    let metadata = match parent.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("unable to inspect allowed source directory `{name}`"),
    };
    if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
        return Ok(None);
    }
    let directory = parent
        .open_dir_nofollow(name)
        .map_err(|_| anyhow!("unable to open allowed source directory `{name}`"))?;
    let opened_metadata = directory
        .dir_metadata()
        .map_err(|_| anyhow!("unable to inspect allowed source directory `{name}`"))?;
    if !opened_metadata.is_dir() || metadata_is_windows_reparse_point(&opened_metadata) {
        return Ok(None);
    }
    Ok(Some(directory))
}

fn discover_markdown_tree(
    directory: &Dir,
    logical_directory: &str,
    depth: usize,
    discovered: &mut Vec<DiscoveredSource>,
    budget: &mut DiscoveryBudget,
) -> Result<()> {
    let entries = directory
        .entries()
        .map_err(|_| anyhow!("unable to enumerate allowed source directory"))?;
    for entry in entries {
        let entry = entry.map_err(|_| anyhow!("unable to enumerate allowed source directory"))?;
        budget.claim_entry()?;
        let Some(name) = visible_utf8_name(entry.file_name()) else {
            continue;
        };
        let metadata = match directory.symlink_metadata(&name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => bail!("unable to inspect allowed source entry"),
        };
        if metadata_is_windows_reparse_point(&metadata) {
            continue;
        }

        let source_label = format!("{logical_directory}/{name}");
        if metadata.is_dir() {
            budget.claim_directory()?;
            let child_depth = depth + 1;
            if child_depth > MAX_SOURCE_TRAVERSAL_DEPTH {
                bail!(SOURCE_DEPTH_LIMIT_ERROR);
            }
            let Some(child) = open_optional_real_directory(directory, &name)? else {
                continue;
            };
            discover_markdown_tree(&child, &source_label, child_depth, discovered, budget)?;
            continue;
        }
        if Path::new(&name)
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("md")
        {
            continue;
        }
        if let Some(source) = read_allowed_file(directory, &name, &source_label, budget)? {
            discovered.push(source);
        }
    }
    Ok(())
}

fn visible_utf8_name(name: OsString) -> Option<String> {
    let name = name.into_string().ok()?;
    (!name.starts_with('.') && !name.contains(['\\', ':']) && !name.chars().any(char::is_control))
        .then_some(name)
}

fn read_allowed_file(
    directory: &Dir,
    name: &str,
    source_label: &str,
    budget: &mut DiscoveryBudget,
) -> Result<Option<DiscoveredSource>> {
    let metadata = match directory.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("unable to inspect source `{source_label}`"),
    };
    if !metadata.is_file() || metadata_is_windows_reparse_point(&metadata) {
        return Ok(None);
    }
    budget.claim_file()?;
    if metadata.len() > crate::cockpit_sources::MEMORY_CONTENT_MAX_BYTES {
        return Ok(None);
    }

    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let mut file = match directory.open_with(name, &options) {
        Ok(file) => file,
        Err(error) if path_open_error_is_symlink(&error) => return Ok(None),
        Err(_) => bail!("unable to open source `{source_label}`"),
    };
    let opened_metadata = file
        .metadata()
        .map_err(|_| anyhow!("unable to inspect opened source `{source_label}`"))?;
    if !opened_metadata.is_file()
        || metadata_is_windows_reparse_point(&opened_metadata)
        || opened_metadata.len() > crate::cockpit_sources::MEMORY_CONTENT_MAX_BYTES
    {
        return Ok(None);
    }
    budget.ensure_bytes_available(opened_metadata.len())?;

    let Some(bytes) = read_source_bytes_with_budget(&mut file, budget, source_label)? else {
        return Ok(None);
    };
    Ok(Some(DiscoveredSource {
        source_label: source_label.to_owned(),
        bytes,
    }))
}

fn read_source_bytes_with_budget<R: Read>(
    reader: &mut R,
    budget: &mut DiscoveryBudget,
    source_label: &str,
) -> Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| anyhow!("unable to read source `{source_label}`"))?;
        if read == 0 {
            break;
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|total| total > crate::cockpit_sources::MEMORY_CONTENT_MAX_BYTES as usize)
        {
            return Ok(None);
        }
        budget.charge_bytes(read as u64)?;
        bytes.extend_from_slice(&buffer[..read]);
    }
    if std::str::from_utf8(&bytes).is_err() {
        return Ok(None);
    }
    Ok(Some(bytes))
}

#[cfg(unix)]
fn path_open_error_is_symlink(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ELOOP)
}

#[cfg(windows)]
fn path_open_error_is_symlink(error: &io::Error) -> bool {
    error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_STOPPED_ON_SYMLINK as i32)
}

#[cfg(not(any(unix, windows)))]
fn path_open_error_is_symlink(_error: &io::Error) -> bool {
    false
}

#[cfg(windows)]
fn metadata_is_windows_reparse_point(metadata: &cap_std::fs::Metadata) -> bool {
    windows_attributes_are_reparse_point(metadata.file_attributes())
}

#[cfg(not(windows))]
fn metadata_is_windows_reparse_point(_metadata: &cap_std::fs::Metadata) -> bool {
    false
}

#[cfg(any(windows, test))]
fn windows_attributes_are_reparse_point(attributes: u32) -> bool {
    attributes & WINDOWS_FILE_ATTRIBUTE_REPARSE_POINT != 0
}

struct ProposedEntry<'a> {
    source: &'a DiscoveredSource,
    target_name: String,
    digest: String,
}

enum TargetRoot {
    Missing,
    Unsafe,
    Ready(Dir),
}

pub(crate) fn build_import_plan(
    coven_home: &Path,
    familiar: &str,
    source_kind: MemoryImportSourceKind,
    discovered: &[DiscoveredSource],
) -> Result<ImportPlan> {
    validate_familiar_component(familiar)?;
    validate_registered_familiar(coven_home, familiar)?;

    let mut sorted = discovered.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.source_label.cmp(&right.source_label));
    let mut logical_keys = HashSet::with_capacity(sorted.len());
    for source in &sorted {
        validate_logical_label(&source.source_label)?;
        if !logical_keys.insert(crate::ward::portable_surface_key(&source.source_label)) {
            bail!("logical label collision in discovered sources");
        }
    }

    let mut base_names = Vec::with_capacity(sorted.len());
    let mut base_counts = HashMap::with_capacity(sorted.len());
    for source in &sorted {
        let (stem, lossy) = normalized_target_stem(&source.source_label);
        let base_name = format!("{stem}.md");
        *base_counts
            .entry(crate::ward::portable_surface_key(&base_name))
            .or_insert(0_usize) += 1;
        base_names.push((base_name, lossy));
    }

    let mut proposed = Vec::with_capacity(sorted.len());
    for (source, (base_name, lossy)) in sorted.into_iter().zip(base_names) {
        let base_key = crate::ward::portable_surface_key(&base_name);
        let base_stem = base_name
            .strip_suffix(".md")
            .expect("planner always creates Markdown names");
        let needs_suffix = lossy || base_counts[&base_key] > 1 || is_windows_device_stem(base_stem);
        let target_name = if needs_suffix {
            let label_digest = blake3::hash(source.source_label.as_bytes()).to_hex();
            let suffix = &label_digest.as_str()[..12];
            format!("{}-{suffix}.md", truncate_ascii_stem(base_stem, 96))
        } else {
            base_name
        };
        validate_target_name(&target_name)?;
        proposed.push(ProposedEntry {
            source,
            target_name,
            digest: blake3_digest(&source.bytes),
        });
    }
    validate_unique_target_names(
        &proposed
            .iter()
            .map(|entry| entry.target_name.clone())
            .collect::<Vec<_>>(),
    )?;

    let bundle_id = bundle_id(familiar, source_kind, &proposed);
    let target_root = inspect_target_root(coven_home, familiar);
    let statuses = inspect_proposed_targets(&target_root, &proposed);
    let entries = proposed
        .into_iter()
        .zip(statuses)
        .map(|(proposed, status)| PlanEntry {
            logical_label: proposed.source.source_label.clone(),
            target_name: proposed.target_name,
            digest: proposed.digest,
            status,
        })
        .collect::<Vec<_>>();
    let create_count = entries
        .iter()
        .filter(|entry| entry.status == PlanEntryStatus::Create)
        .count();
    let unchanged_count = entries
        .iter()
        .filter(|entry| entry.status == PlanEntryStatus::Unchanged)
        .count();
    let conflict_count = entries
        .iter()
        .filter(|entry| entry.status == PlanEntryStatus::Conflict)
        .count();
    let apply_eligible = conflict_count == 0;

    Ok(ImportPlan {
        familiar_id: familiar.to_owned(),
        source_kind,
        bundle_id,
        status: if apply_eligible {
            ImportPlanStatus::Preview
        } else {
            ImportPlanStatus::Conflict
        },
        apply_eligible,
        file_count: entries.len(),
        create_count,
        unchanged_count,
        restored_count: 0,
        conflict_count,
        entries,
    })
}

fn validate_familiar_component(familiar: &str) -> Result<()> {
    let valid = !familiar.is_empty()
        && familiar.len() <= 64
        && familiar
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        && !is_windows_device_stem(familiar);
    if !valid {
        bail!("familiar ID must be a safe single component");
    }
    Ok(())
}

fn validate_registered_familiar(coven_home: &Path, familiar: &str) -> Result<()> {
    let registered = crate::cockpit_sources::read_familiars(coven_home)
        .map_err(|_| anyhow!("unable to read familiar registry"))?
        .into_iter()
        .any(|candidate| candidate.id == familiar);
    if !registered {
        bail!("unknown familiar `{familiar}`");
    }
    Ok(())
}

fn validate_logical_label(label: &str) -> Result<()> {
    let valid = !label.is_empty()
        && !label.starts_with('/')
        && !label.ends_with('/')
        && !label.contains(['\\', ':'])
        && !label.chars().any(char::is_control)
        && label.ends_with(".md")
        && label
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..");
    if !valid {
        bail!("discovered source has an invalid logical label");
    }
    Ok(())
}

fn normalized_target_stem(label: &str) -> (String, bool) {
    use unicode_normalization::char::is_combining_mark;

    let logical_stem = label
        .strip_suffix(".md")
        .expect("validated logical labels retain .md");
    let mut normalized = String::new();
    let mut separator_pending = false;
    let mut lossy = false;
    for original in logical_stem.chars() {
        let mut original_had_ascii_alphanumeric = false;
        let mut original_was_only_combining = true;
        for character in original.to_string().nfkd() {
            if character.is_ascii_alphanumeric() {
                if separator_pending && !normalized.is_empty() {
                    normalized.push('-');
                }
                separator_pending = false;
                normalized.push(character.to_ascii_lowercase());
                original_had_ascii_alphanumeric = true;
                original_was_only_combining = false;
            } else if is_combining_mark(character) {
                continue;
            } else {
                separator_pending = true;
                original_was_only_combining = false;
            }
        }
        if !original.is_ascii() && !original_had_ascii_alphanumeric && !original_was_only_combining
        {
            lossy = true;
        }
    }
    let normalized = normalized.trim_matches('-');
    if normalized.is_empty() {
        ("memory".to_owned(), true)
    } else {
        (truncate_ascii_stem(normalized, 108).to_owned(), lossy)
    }
}

fn truncate_ascii_stem(stem: &str, max_len: usize) -> &str {
    let end = stem.len().min(max_len);
    stem[..end].trim_end_matches('-')
}

fn validate_target_name(name: &str) -> Result<()> {
    let stem = name.strip_suffix(".md").unwrap_or_default();
    let valid = name.is_ascii()
        && !stem.is_empty()
        && name.len() <= 128
        && Path::new(name).components().count() == 1
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':'])
        && !name.ends_with(['.', ' '])
        && !name.chars().any(char::is_control)
        && !is_windows_device_stem(stem);
    if !valid {
        bail!("planner generated an unsafe target name");
    }
    Ok(())
}

fn validate_unique_target_names(names: &[String]) -> Result<()> {
    let mut keys = HashSet::with_capacity(names.len());
    for name in names {
        if !keys.insert(crate::ward::portable_surface_key(name)) {
            bail!("target-name collision in import plan");
        }
    }
    for name in names {
        validate_target_name(name)?;
    }
    Ok(())
}

fn is_windows_device_stem(stem: &str) -> bool {
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn blake3_digest(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

fn bundle_id(
    familiar: &str,
    source_kind: MemoryImportSourceKind,
    entries: &[ProposedEntry<'_>],
) -> String {
    let mut hasher = blake3::Hasher::new();
    update_framed(&mut hasher, b"coven-memory-import-plan-v1");
    update_framed(&mut hasher, familiar.as_bytes());
    update_framed(
        &mut hasher,
        match source_kind {
            MemoryImportSourceKind::Native => b"native",
            MemoryImportSourceKind::Openclaw => b"openclaw",
        },
    );
    hasher.update(&(entries.len() as u64).to_le_bytes());
    for entry in entries {
        update_framed(&mut hasher, entry.source.source_label.as_bytes());
        update_framed(&mut hasher, entry.target_name.as_bytes());
        update_framed(&mut hasher, entry.digest.as_bytes());
    }
    format!("blake3-{}", hasher.finalize().to_hex())
}

fn update_framed(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn inspect_target_root(coven_home: &Path, familiar: &str) -> TargetRoot {
    let coven_dir = match open_dir_path_nofollow(coven_home) {
        Ok(directory) => directory,
        Err(_) => return TargetRoot::Unsafe,
    };
    let memory_metadata = match coven_dir.symlink_metadata("memory") {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return TargetRoot::Missing,
        Err(_) => return TargetRoot::Unsafe,
    };
    if !memory_metadata.is_dir() || metadata_is_windows_reparse_point(&memory_metadata) {
        return TargetRoot::Unsafe;
    }
    let memory_dir = match coven_dir.open_dir_nofollow("memory") {
        Ok(directory) => directory,
        Err(_) => return TargetRoot::Unsafe,
    };
    let opened_memory_metadata = match memory_dir.dir_metadata() {
        Ok(metadata) => metadata,
        Err(_) => return TargetRoot::Unsafe,
    };
    if !opened_memory_metadata.is_dir()
        || metadata_is_windows_reparse_point(&opened_memory_metadata)
    {
        return TargetRoot::Unsafe;
    }

    let familiar_key = crate::ward::portable_surface_key(familiar);
    let memory_entries = match memory_dir.entries() {
        Ok(entries) => entries,
        Err(_) => return TargetRoot::Unsafe,
    };
    for entry in memory_entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => return TargetRoot::Unsafe,
        };
        let Some(name) = entry.file_name().into_string().ok() else {
            continue;
        };
        if crate::ward::portable_surface_key(&name) == familiar_key && name != familiar {
            return TargetRoot::Unsafe;
        }
    }

    let familiar_metadata = match memory_dir.symlink_metadata(familiar) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return TargetRoot::Missing,
        Err(_) => return TargetRoot::Unsafe,
    };
    if !familiar_metadata.is_dir() || metadata_is_windows_reparse_point(&familiar_metadata) {
        return TargetRoot::Unsafe;
    }
    let familiar_dir = match memory_dir.open_dir_nofollow(familiar) {
        Ok(directory) => directory,
        Err(_) => return TargetRoot::Unsafe,
    };
    let opened_familiar_metadata = match familiar_dir.dir_metadata() {
        Ok(metadata) => metadata,
        Err(_) => return TargetRoot::Unsafe,
    };
    if !opened_familiar_metadata.is_dir()
        || metadata_is_windows_reparse_point(&opened_familiar_metadata)
    {
        return TargetRoot::Unsafe;
    }
    TargetRoot::Ready(familiar_dir)
}

fn inspect_proposed_targets(
    target_root: &TargetRoot,
    proposed: &[ProposedEntry<'_>],
) -> Vec<PlanEntryStatus> {
    match target_root {
        TargetRoot::Missing => vec![PlanEntryStatus::Create; proposed.len()],
        TargetRoot::Unsafe => vec![PlanEntryStatus::Conflict; proposed.len()],
        TargetRoot::Ready(directory) => inspect_ready_target_root(directory, proposed),
    }
}

fn inspect_ready_target_root(
    directory: &Dir,
    proposed: &[ProposedEntry<'_>],
) -> Vec<PlanEntryStatus> {
    let entries = match directory.entries() {
        Ok(entries) => entries,
        Err(_) => return vec![PlanEntryStatus::Conflict; proposed.len()],
    };
    let mut existing_names: HashMap<String, Vec<String>> = HashMap::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => return vec![PlanEntryStatus::Conflict; proposed.len()],
        };
        let Some(name) = entry.file_name().into_string().ok() else {
            continue;
        };
        existing_names
            .entry(crate::ward::portable_surface_key(&name))
            .or_default()
            .push(name);
    }

    proposed
        .iter()
        .map(|entry| {
            let key = crate::ward::portable_surface_key(&entry.target_name);
            let Some(matches) = existing_names.get(&key) else {
                return PlanEntryStatus::Create;
            };
            if matches.len() != 1 || matches[0] != entry.target_name {
                return PlanEntryStatus::Conflict;
            }
            inspect_existing_target(directory, entry)
        })
        .collect()
}

fn inspect_existing_target(directory: &Dir, proposed: &ProposedEntry<'_>) -> PlanEntryStatus {
    let metadata = match directory.symlink_metadata(&proposed.target_name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return PlanEntryStatus::Create,
        Err(_) => return PlanEntryStatus::Conflict,
    };
    if !metadata.is_file()
        || metadata_is_windows_reparse_point(&metadata)
        || metadata.len() > crate::cockpit_sources::MEMORY_CONTENT_MAX_BYTES
    {
        return PlanEntryStatus::Conflict;
    }

    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let mut file = match directory.open_with(&proposed.target_name, &options) {
        Ok(file) => file,
        Err(_) => return PlanEntryStatus::Conflict,
    };
    let Some(digest) = digest_stable_opened_file(&mut file) else {
        return PlanEntryStatus::Conflict;
    };
    if digest == proposed.digest {
        PlanEntryStatus::Unchanged
    } else {
        PlanEntryStatus::Conflict
    }
}

fn digest_stable_opened_file(file: &mut cap_std::fs::File) -> Option<String> {
    let before = file.metadata().ok()?;
    if !before.is_file()
        || metadata_is_windows_reparse_point(&before)
        || before.len() > crate::cockpit_sources::MEMORY_CONTENT_MAX_BYTES
    {
        return None;
    }

    let digest = digest_exact_length_stream(file, before.len())?;
    let after = file.metadata().ok()?;
    if !after.is_file()
        || metadata_is_windows_reparse_point(&after)
        || after.len() != before.len()
        || !opened_metadata_stable(&before, &after)
    {
        return None;
    }
    Some(digest)
}

fn digest_exact_length_stream<R: Read>(reader: &mut R, expected_len: u64) -> Option<String> {
    if expected_len > crate::cockpit_sources::MEMORY_CONTENT_MAX_BYTES {
        return None;
    }
    let mut hasher = blake3::Hasher::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64)?;
        if total > expected_len || total > crate::cockpit_sources::MEMORY_CONTENT_MAX_BYTES {
            return None;
        }
        hasher.update(&buffer[..read]);
    }
    if total != expected_len {
        return None;
    }
    Some(format!("blake3:{}", hasher.finalize().to_hex()))
}

#[cfg(unix)]
fn opened_metadata_stable(before: &cap_std::fs::Metadata, after: &cap_std::fs::Metadata) -> bool {
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.size() == after.size()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}

#[cfg(windows)]
fn opened_metadata_stable(before: &cap_std::fs::Metadata, after: &cap_std::fs::Metadata) -> bool {
    before.creation_time() == after.creation_time()
        && before.last_write_time() == after.last_write_time()
        && before.file_size() == after.file_size()
}

#[cfg(not(any(unix, windows)))]
fn opened_metadata_stable(before: &cap_std::fs::Metadata, after: &cap_std::fs::Metadata) -> bool {
    before.len() == after.len() && before.modified().ok() == after.modified().ok()
}

const IMPORT_PROTOCOL_VERSION: u32 = 1;
const MIGRATIONS_DIRECTORY: &str = "memory-migrations";
const MANIFEST_FILE: &str = "manifest.json";
const JOURNAL_FILE: &str = "journal.jsonl";
const STAGED_DIRECTORY: &str = "staged";
const MIGRATION_LOCK_FILE: &str = ".migration.lock";
const TARGET_WORK_DIRECTORY: &str = ".coven-migration-work";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ImportManifest {
    protocol_version: u32,
    familiar_id: String,
    source_kind: MemoryImportSourceKind,
    bundle_id: String,
    entries: Vec<ManifestEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ManifestEntry {
    source_label: String,
    target_name: String,
    digest: String,
    byte_length: u64,
    initial_status: ManifestInitialStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct RestoredMarker {
    protocol_version: u32,
    familiar_id: String,
    bundle_id: String,
    target_name: String,
    digest: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ManifestInitialStatus {
    Prepared,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum JournalScope {
    Bundle,
    Entry,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum JournalState {
    Prepared,
    Publishing,
    Published,
    Verified,
    Restoring,
    Restored,
    Reactivating,
    Invalidated,
    RollingBack,
    RolledBack,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum JournalOutcome {
    Created,
    Unchanged,
    NotPublished,
    Removed,
    Suppressed,
    ManualRecovery,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct JournalRecord {
    protocol_version: u32,
    sequence: u64,
    scope: JournalScope,
    state: JournalState,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    byte_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<JournalOutcome>,
}

#[derive(Default)]
struct JournalSummary {
    records: Vec<JournalRecord>,
    bundle_state: Option<JournalState>,
    entry_states: HashMap<String, JournalRecord>,
}

impl JournalSummary {
    fn next_sequence(&self) -> Result<u64> {
        u64::try_from(self.records.len()).map_err(|_| anyhow!("import journal is too large"))
    }

    fn apply(&mut self, record: JournalRecord) {
        if record.scope == JournalScope::Bundle {
            self.bundle_state = Some(record.state);
        } else if let Some(target_name) = &record.target_name {
            self.entry_states
                .insert(target_name.clone(), record.clone());
        }
        self.records.push(record);
    }
}

struct Bundle {
    dir: Dir,
    staged: Dir,
}

struct MigrationFamiliar {
    dir: Dir,
    _lock: std::fs::File,
}

struct TargetContext {
    dir: Dir,
    work: Dir,
}

struct CurrentApplyOwnership {
    candidates: HashMap<String, cap_std::fs::File>,
}

#[derive(Clone, Debug)]
enum ApplyStep {
    AfterStage(String),
    Prepared,
    AfterCandidateSyncBeforePrepared(String),
    BeforePublish(String),
    BeforeAtomicPublish(String),
    AfterPublishBeforeJournal(String),
    BeforePublishedJournalWrite(String),
    AfterPublishedJournalWriteBeforeSync(String),
    AfterPublishedJournalSync(String),
    BeforeVerifiedJournalWrite(String),
    DuringRollback(String),
    AfterNotPublishedRollingBack(String),
    BeforeRollbackQuarantine(String),
    AfterRollbackQuarantineBeforeSync(String),
    AfterRollbackRemoveBeforeJournal(String),
}

impl ApplyStep {
    fn target_name(&self) -> Option<&str> {
        match self {
            Self::Prepared => None,
            Self::AfterStage(name)
            | Self::AfterCandidateSyncBeforePrepared(name)
            | Self::BeforePublish(name)
            | Self::BeforeAtomicPublish(name)
            | Self::AfterPublishBeforeJournal(name)
            | Self::BeforePublishedJournalWrite(name)
            | Self::AfterPublishedJournalWriteBeforeSync(name)
            | Self::AfterPublishedJournalSync(name)
            | Self::BeforeVerifiedJournalWrite(name)
            | Self::DuringRollback(name)
            | Self::AfterNotPublishedRollingBack(name)
            | Self::BeforeRollbackQuarantine(name)
            | Self::AfterRollbackQuarantineBeforeSync(name)
            | Self::AfterRollbackRemoveBeforeJournal(name) => Some(name),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApplyHookAction {
    Continue,
    #[cfg(test)]
    Interrupt,
}

trait ApplyHook {
    fn step(&mut self, step: &ApplyStep) -> Result<ApplyHookAction>;
}

#[derive(Clone, Debug)]
enum RestoreStep {
    BeforeMarker(String),
    AfterMarkerBeforeVerify(String),
    AfterVerifyBeforeJournal(String),
}

impl RestoreStep {
    fn target_name(&self) -> &str {
        match self {
            Self::BeforeMarker(name)
            | Self::AfterMarkerBeforeVerify(name)
            | Self::AfterVerifyBeforeJournal(name) => name,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestoreHookAction {
    Continue,
    #[cfg(test)]
    Interrupt,
}

trait RestoreHook {
    fn step(&mut self, step: &RestoreStep) -> Result<RestoreHookAction>;
}

struct NoopRestoreHook;

impl RestoreHook for NoopRestoreHook {
    fn step(&mut self, _step: &RestoreStep) -> Result<RestoreHookAction> {
        Ok(RestoreHookAction::Continue)
    }
}

#[cfg(test)]
struct TestRestoreHook<F> {
    callback: F,
}

#[cfg(test)]
impl<F> TestRestoreHook<F> {
    fn new(callback: F) -> Self {
        Self { callback }
    }
}

#[cfg(test)]
impl<F> RestoreHook for TestRestoreHook<F>
where
    F: FnMut(&RestoreStep) -> Result<RestoreHookAction>,
{
    fn step(&mut self, step: &RestoreStep) -> Result<RestoreHookAction> {
        (self.callback)(step)
    }
}

struct NoopApplyHook;

impl ApplyHook for NoopApplyHook {
    fn step(&mut self, _step: &ApplyStep) -> Result<ApplyHookAction> {
        Ok(ApplyHookAction::Continue)
    }
}

#[cfg(test)]
#[derive(Debug)]
struct ApplyInterrupted;

#[cfg(test)]
impl fmt::Display for ApplyInterrupted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("memory import interrupted")
    }
}

#[cfg(test)]
impl std::error::Error for ApplyInterrupted {}

fn run_apply_step(hook: &mut dyn ApplyHook, step: ApplyStep) -> Result<()> {
    let _ = step.target_name();
    match hook.step(&step)? {
        ApplyHookAction::Continue => Ok(()),
        #[cfg(test)]
        ApplyHookAction::Interrupt => Err(anyhow!(ApplyInterrupted)),
    }
}

fn run_restore_step(hook: &mut dyn RestoreHook, step: RestoreStep) -> Result<()> {
    let _ = step.target_name();
    match hook.step(&step)? {
        RestoreHookAction::Continue => Ok(()),
        #[cfg(test)]
        RestoreHookAction::Interrupt => Err(anyhow!(ApplyInterrupted)),
    }
}

pub(crate) fn apply_import_plan(
    coven_home: &Path,
    plan: &ImportPlan,
    discovered: &[DiscoveredSource],
) -> Result<ImportPlan> {
    apply_import_plan_with_hook(coven_home, plan, discovered, &mut NoopApplyHook)
}

#[cfg(windows)]
fn apply_import_plan_with_hook(
    _coven_home: &Path,
    _plan: &ImportPlan,
    _discovered: &[DiscoveredSource],
    _hook: &mut dyn ApplyHook,
) -> Result<ImportPlan> {
    bail!(
        "memory import apply is unavailable on Windows because durable directory publication cannot be guaranteed"
    )
}

#[cfg(not(windows))]
fn apply_import_plan_with_hook(
    coven_home: &Path,
    plan: &ImportPlan,
    discovered: &[DiscoveredSource],
    hook: &mut dyn ApplyHook,
) -> Result<ImportPlan> {
    validate_familiar_component(&plan.familiar_id)?;
    validate_registered_familiar(coven_home, &plan.familiar_id)?;
    let sources = validate_apply_inputs(plan, discovered)?;
    if !plan.apply_eligible
        && !existing_migration_familiar_is_present(coven_home, &plan.familiar_id)?
    {
        bail!("memory import plan has conflicts");
    }
    let migration = open_locked_migration_familiar(coven_home, &plan.familiar_id)?;
    reconcile_other_incomplete_bundles(
        coven_home,
        &migration,
        &plan.familiar_id,
        &plan.bundle_id,
        hook,
    )?;
    let refreshed_plan =
        build_import_plan(coven_home, &plan.familiar_id, plan.source_kind, discovered)?;
    if refreshed_plan.bundle_id != plan.bundle_id
        || refreshed_plan.entries.len() != plan.entries.len()
    {
        bail!("apply inputs do not match the import plan");
    }
    let expected_manifest = manifest_from_plan(&refreshed_plan, &sources)?;
    if !refreshed_plan.apply_eligible {
        let existing_bundle = migration
            .dir
            .symlink_metadata(bundle_component(&refreshed_plan.bundle_id)?)
            .is_ok();
        if !existing_bundle {
            bail!("memory import plan has conflicts");
        }
    }
    let bundle = open_or_create_bundle(&migration, &refreshed_plan.bundle_id)?;
    let manifest = create_or_validate_manifest(&bundle, &expected_manifest)?;
    let mut journal = open_or_create_journal(&bundle)?;
    let mut current_ownership = CurrentApplyOwnership {
        candidates: HashMap::new(),
    };
    validate_journal_against_manifest(&journal, &manifest)?;

    if journal.bundle_state.is_none() {
        append_journal(
            &bundle,
            &mut journal,
            JournalScope::Bundle,
            JournalState::Prepared,
            None,
            None,
        )?;
    }
    stage_manifest_entries(&bundle, &manifest, &sources, &journal, hook)?;
    run_apply_step(hook, ApplyStep::Prepared)?;

    if journal.bundle_state == Some(JournalState::Verified) {
        return verified_report(&refreshed_plan, &manifest, &bundle, coven_home, &journal);
    }
    if journal.bundle_state == Some(JournalState::RolledBack) {
        return Err(if journal_requires_manual_recovery(&journal) {
            anyhow!("memory import requires manual recovery")
        } else {
            anyhow!("memory import bundle was rolled back")
        });
    }
    if journal.bundle_state == Some(JournalState::Restoring) {
        bail!("memory import bundle has an incomplete restore");
    }
    if journal.bundle_state == Some(JournalState::Restored) {
        let restored_outcome = journal
            .records
            .iter()
            .rev()
            .find(|record| {
                record.scope == JournalScope::Bundle && record.state == JournalState::Restored
            })
            .and_then(|record| record.outcome);
        if restored_outcome == Some(JournalOutcome::ManualRecovery) {
            bail!("memory import bundle restore requires manual recovery");
        }
        if refreshed_plan
            .entries
            .iter()
            .any(|entry| entry.status != PlanEntryStatus::Unchanged)
            || !restored_bundle_has_reactivation_provenance(coven_home, &manifest, &journal)?
        {
            append_journal(
                &bundle,
                &mut journal,
                JournalScope::Bundle,
                JournalState::Invalidated,
                None,
                Some(JournalOutcome::ManualRecovery),
            )?;
            bail!("memory import bundle restore requires manual recovery");
        }
        resume_reactivation(coven_home, &bundle, &manifest, &mut journal)?;
        return verified_report(&refreshed_plan, &manifest, &bundle, coven_home, &journal);
    }
    if journal.bundle_state == Some(JournalState::Reactivating) {
        resume_reactivation(coven_home, &bundle, &manifest, &mut journal)?;
        return verified_report(&refreshed_plan, &manifest, &bundle, coven_home, &journal);
    }
    if journal.bundle_state == Some(JournalState::Invalidated) {
        bail!("memory import bundle restore requires manual recovery");
    }
    if journal.bundle_state == Some(JournalState::RollingBack) {
        let manual = resume_rollback(
            coven_home,
            &manifest,
            &bundle,
            &mut journal,
            hook,
            &current_ownership.candidates,
        )?;
        return Err(if manual {
            anyhow!("memory import requires manual recovery")
        } else {
            anyhow!("memory import bundle was rolled back")
        });
    }
    if journal.bundle_state == Some(JournalState::Prepared) {
        append_journal(
            &bundle,
            &mut journal,
            JournalScope::Bundle,
            JournalState::Publishing,
            None,
            None,
        )?;
    }

    let result = publish_entries(
        coven_home,
        &refreshed_plan,
        &manifest,
        &bundle,
        &mut journal,
        hook,
        &mut current_ownership,
    );
    match result {
        Ok(report) => Ok(report),
        Err(error) if apply_was_interrupted(&error) => Err(error),
        Err(error) => {
            let manual = open_or_create_journal(&bundle)
                .and_then(|mut recovered| {
                    validate_journal_against_manifest(&recovered, &manifest)?;
                    resume_rollback(
                        coven_home,
                        &manifest,
                        &bundle,
                        &mut recovered,
                        hook,
                        &current_ownership.candidates,
                    )
                })
                .unwrap_or(true);
            if manual {
                Err(anyhow!(
                    "memory import failed and requires manual recovery: {error}"
                ))
            } else {
                Err(error)
            }
        }
    }
}

#[cfg(test)]
fn apply_was_interrupted(error: &anyhow::Error) -> bool {
    error.downcast_ref::<ApplyInterrupted>().is_some()
}

#[cfg(not(test))]
fn apply_was_interrupted(_error: &anyhow::Error) -> bool {
    false
}

fn existing_migration_familiar_is_present(coven_home: &Path, familiar: &str) -> Result<bool> {
    let home = open_dir_path_nofollow(coven_home)
        .map_err(|_| anyhow!("COVEN_HOME must be a real directory"))?;
    let migrations = match home.symlink_metadata(MIGRATIONS_DIRECTORY) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(_) => bail!("unable to inspect memory migration directory"),
        Ok(metadata) if metadata.is_dir() && !metadata_is_windows_reparse_point(&metadata) => home
            .open_dir_nofollow(MIGRATIONS_DIRECTORY)
            .map_err(|_| anyhow!("memory migration path is not a real directory"))?,
        Ok(_) => bail!("memory migration path is not a real directory"),
    };
    match migrations.symlink_metadata(familiar) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => bail!("unable to inspect familiar migration directory"),
        Ok(metadata) if metadata.is_dir() && !metadata_is_windows_reparse_point(&metadata) => {
            Ok(true)
        }
        Ok(_) => bail!("familiar migration path is not a real directory"),
    }
}

fn validate_apply_inputs<'a>(
    plan: &ImportPlan,
    discovered: &'a [DiscoveredSource],
) -> Result<HashMap<&'a str, &'a DiscoveredSource>> {
    let mut sources = HashMap::with_capacity(discovered.len());
    for source in discovered {
        validate_logical_label(&source.source_label)?;
        if sources
            .insert(source.source_label.as_str(), source)
            .is_some()
        {
            bail!("duplicate source label in apply inputs");
        }
    }
    if sources.len() != plan.entries.len() {
        bail!("apply inputs do not match the import plan");
    }
    for entry in &plan.entries {
        let source = sources
            .get(entry.logical_label.as_str())
            .ok_or_else(|| anyhow!("apply inputs do not match the import plan"))?;
        if blake3_digest(&source.bytes) != entry.digest {
            bail!("apply inputs do not match the import plan");
        }
    }
    Ok(sources)
}

fn manifest_from_plan(
    plan: &ImportPlan,
    sources: &HashMap<&str, &DiscoveredSource>,
) -> Result<ImportManifest> {
    let mut entries = Vec::with_capacity(plan.entries.len());
    for entry in &plan.entries {
        let source = sources
            .get(entry.logical_label.as_str())
            .ok_or_else(|| anyhow!("apply inputs do not match the import plan"))?;
        entries.push(ManifestEntry {
            source_label: entry.logical_label.clone(),
            target_name: entry.target_name.clone(),
            digest: entry.digest.clone(),
            byte_length: u64::try_from(source.bytes.len())
                .map_err(|_| anyhow!("source is too large"))?,
            initial_status: ManifestInitialStatus::Prepared,
        });
    }
    entries.sort_by(|left, right| left.source_label.cmp(&right.source_label));
    Ok(ImportManifest {
        protocol_version: IMPORT_PROTOCOL_VERSION,
        familiar_id: plan.familiar_id.clone(),
        source_kind: plan.source_kind,
        bundle_id: plan.bundle_id.clone(),
        entries,
    })
}

#[cfg(test)]
fn bundle_path(coven_home: &Path, familiar: &str, bundle_id: &str) -> Result<PathBuf> {
    validate_familiar_component(familiar)?;
    Ok(coven_home
        .join(MIGRATIONS_DIRECTORY)
        .join(familiar)
        .join(bundle_component(bundle_id)?))
}

fn bundle_component(bundle_id: &str) -> Result<String> {
    let digest = bundle_id
        .strip_prefix("blake3-")
        .ok_or_else(|| anyhow!("bundle ID has an unsupported format"))?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("bundle ID has an unsupported format");
    }
    Ok(bundle_id.to_owned())
}

fn open_locked_migration_familiar(coven_home: &Path, familiar: &str) -> Result<MigrationFamiliar> {
    let home = open_dir_path_nofollow(coven_home)
        .map_err(|_| anyhow!("COVEN_HOME must be a real directory"))?;
    let migrations = ensure_child_directory(&home, MIGRATIONS_DIRECTORY, true)?;
    let familiar_dir = ensure_child_directory(&migrations, familiar, true)?;
    match familiar_dir.symlink_metadata(MIGRATION_LOCK_FILE) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_private_file(&familiar_dir, MIGRATION_LOCK_FILE, &[])?;
            sync_dir_handle(&familiar_dir)?;
        }
        Ok(_) => {}
        Err(_) => bail!("unable to inspect memory migration lock"),
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    let lock = familiar_dir
        .open_with(MIGRATION_LOCK_FILE, &options)
        .map_err(|_| anyhow!("unable to open memory migration lock"))?;
    validate_private_file_handle(&lock)?;
    let lock = lock.into_std();
    lock.try_lock_exclusive()
        .map_err(|_| anyhow!("another memory migration is active for this familiar"))?;
    Ok(MigrationFamiliar {
        dir: familiar_dir,
        _lock: lock,
    })
}

fn open_or_create_bundle(migration: &MigrationFamiliar, bundle_id: &str) -> Result<Bundle> {
    let bundle_name = bundle_component(bundle_id)?;
    let (bundle_dir, _) = ensure_child_directory_with_status(&migration.dir, &bundle_name, true)?;
    let staged = ensure_child_directory(&bundle_dir, STAGED_DIRECTORY, true)?;
    Ok(Bundle {
        dir: bundle_dir,
        staged,
    })
}

fn open_existing_bundle(migration: &MigrationFamiliar, bundle_name: &str) -> Result<Bundle> {
    open_existing_bundle_in(&migration.dir, bundle_name)
}

fn open_existing_bundle_in(migration: &Dir, bundle_name: &str) -> Result<Bundle> {
    if bundle_component(bundle_name)? != bundle_name {
        bail!("memory migration bundle name is invalid");
    }
    let metadata = migration
        .symlink_metadata(bundle_name)
        .map_err(|_| anyhow!("unable to inspect memory migration bundle"))?;
    if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
        bail!("memory migration bundle is not a real directory");
    }
    let dir = migration
        .open_dir_nofollow(bundle_name)
        .map_err(|_| anyhow!("memory migration bundle is not a real directory"))?;
    validate_private_directory_handle(&dir)?;
    let staged_metadata = dir
        .symlink_metadata(STAGED_DIRECTORY)
        .map_err(|_| anyhow!("memory migration staged directory is unavailable"))?;
    if !staged_metadata.is_dir() || metadata_is_windows_reparse_point(&staged_metadata) {
        bail!("memory migration staged path is not a real directory");
    }
    let staged = dir
        .open_dir_nofollow(STAGED_DIRECTORY)
        .map_err(|_| anyhow!("memory migration staged path is not a real directory"))?;
    validate_private_directory_handle(&staged)?;
    Ok(Bundle { dir, staged })
}

fn open_existing_private_directory(parent: &Dir, name: &str) -> Result<Dir> {
    let metadata = parent
        .symlink_metadata(name)
        .map_err(|_| anyhow!("private import directory is unavailable"))?;
    if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
        bail!("private import directory is unsafe");
    }
    let directory = parent
        .open_dir_nofollow(name)
        .map_err(|_| anyhow!("private import directory is unavailable"))?;
    validate_private_directory_handle(&directory)?;
    Ok(directory)
}

fn open_existing_migration_familiar(coven_home: &Path, familiar: &str) -> Result<Dir> {
    let home = open_dir_path_nofollow(coven_home)
        .map_err(|_| anyhow!("COVEN_HOME must be a real directory"))?;
    let migrations = open_existing_private_directory(&home, MIGRATIONS_DIRECTORY)?;
    open_existing_private_directory(&migrations, familiar)
}

fn open_existing_target_context(
    coven_home: &Path,
    familiar: &str,
    bundle_id: &str,
) -> Result<TargetContext> {
    let home = open_dir_path_nofollow(coven_home)
        .map_err(|_| anyhow!("COVEN_HOME must be a real directory"))?;
    let memory = home
        .open_dir_nofollow("memory")
        .map_err(|_| anyhow!("memory root is unavailable"))?;
    let dir = memory
        .open_dir_nofollow(familiar)
        .map_err(|_| anyhow!("familiar memory is unavailable"))?;
    let work_root = open_existing_private_directory(&dir, TARGET_WORK_DIRECTORY)?;
    let work = open_existing_private_directory(&work_root, &bundle_component(bundle_id)?)?;
    Ok(TargetContext { dir, work })
}

fn reconcile_other_incomplete_bundles(
    coven_home: &Path,
    migration: &MigrationFamiliar,
    familiar: &str,
    current_bundle_id: &str,
    hook: &mut dyn ApplyHook,
) -> Result<()> {
    let mut names = Vec::new();
    for entry in migration
        .dir
        .entries()
        .map_err(|_| anyhow!("unable to enumerate memory migration bundles"))?
    {
        let entry = entry.map_err(|_| anyhow!("unable to enumerate memory migration bundles"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow!("memory migration bundle name is invalid"))?;
        if name == MIGRATION_LOCK_FILE || name == current_bundle_id {
            continue;
        }
        bundle_component(&name)?;
        names.push(name);
    }
    names.sort();
    for name in names {
        let bundle = match open_existing_bundle(migration, &name) {
            Ok(bundle) => bundle,
            Err(error) => {
                if discard_empty_bundle_without_staged(migration, &name)? {
                    continue;
                }
                return Err(error);
            }
        };
        let manifest = match read_existing_manifest(&bundle) {
            Ok(manifest) => manifest,
            Err(error) => {
                if discard_prepublication_bundle(migration, &name, bundle)? {
                    continue;
                }
                return Err(error);
            }
        };
        validate_stored_manifest(&manifest, familiar, &name)?;
        let mut journal = open_or_create_journal(&bundle)?;
        validate_journal_against_manifest(&journal, &manifest)?;
        match journal.bundle_state {
            Some(JournalState::Verified) => {
                stage_manifest_is_complete(&bundle, &manifest)?;
                continue;
            }
            Some(JournalState::Restored) => continue,
            Some(JournalState::Reactivating) => {
                resume_reactivation(coven_home, &bundle, &manifest, &mut journal)?;
                stage_manifest_is_complete(&bundle, &manifest)?;
                continue;
            }
            Some(JournalState::Invalidated) => {
                bail!("an older memory import requires manual recovery")
            }
            Some(JournalState::Restoring) => {
                bail!("an older memory import has an incomplete restore")
            }
            Some(JournalState::RolledBack) => {
                if journal_requires_manual_recovery(&journal) {
                    bail!("an older memory import requires manual recovery");
                }
                continue;
            }
            None => {
                append_journal(
                    &bundle,
                    &mut journal,
                    JournalScope::Bundle,
                    JournalState::Prepared,
                    None,
                    None,
                )?;
            }
            Some(_) => {}
        }
        if resume_rollback(
            coven_home,
            &manifest,
            &bundle,
            &mut journal,
            hook,
            &HashMap::new(),
        )? {
            bail!("an older memory import requires manual recovery");
        }
    }
    Ok(())
}

fn discard_empty_bundle_without_staged(
    migration: &MigrationFamiliar,
    bundle_name: &str,
) -> Result<bool> {
    let metadata = migration
        .dir
        .symlink_metadata(bundle_name)
        .map_err(|_| anyhow!("unable to inspect memory migration bundle"))?;
    if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
        return Ok(false);
    }
    let dir = migration
        .dir
        .open_dir_nofollow(bundle_name)
        .map_err(|_| anyhow!("memory migration bundle is not a real directory"))?;
    validate_private_directory_handle(&dir)?;
    if dir.symlink_metadata(STAGED_DIRECTORY).is_ok()
        || dir
            .entries()
            .map_err(|_| anyhow!("unable to enumerate memory migration bundle"))?
            .next()
            .is_some()
    {
        return Ok(false);
    }
    drop(dir);
    migration
        .dir
        .remove_dir(bundle_name)
        .map_err(|_| anyhow!("unable to remove abandoned memory migration bundle"))?;
    sync_dir_handle(&migration.dir)?;
    Ok(true)
}

fn bundle_is_prepublication(bundle: &Bundle) -> Result<bool> {
    let journal = open_or_create_journal(bundle)?;
    Ok(
        matches!(journal.bundle_state, None | Some(JournalState::Prepared))
            && journal.entry_states.is_empty(),
    )
}

fn reset_prepublication_bundle_files(bundle: &Bundle) -> Result<()> {
    if !bundle_is_prepublication(bundle)? {
        bail!("memory migration bundle has already entered publication");
    }
    for entry in bundle
        .staged
        .entries()
        .map_err(|_| anyhow!("unable to enumerate staged import files"))?
    {
        let entry = entry.map_err(|_| anyhow!("unable to enumerate staged import files"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow!("staged import file name is invalid"))?;
        remove_private_file_if_present(&bundle.staged, &name)?;
    }
    remove_private_file_if_present(&bundle.dir, MANIFEST_FILE)?;
    remove_private_file_if_present(&bundle.dir, &partial_file_name(MANIFEST_FILE))?;
    Ok(())
}

fn discard_prepublication_bundle(
    migration: &MigrationFamiliar,
    bundle_name: &str,
    bundle: Bundle,
) -> Result<bool> {
    if !bundle_is_prepublication(&bundle)? {
        return Ok(false);
    }
    let partial_manifest = partial_file_name(MANIFEST_FILE);
    for entry in bundle
        .dir
        .entries()
        .map_err(|_| anyhow!("unable to enumerate memory migration bundle"))?
    {
        let entry = entry.map_err(|_| anyhow!("unable to enumerate memory migration bundle"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow!("memory migration bundle entry is invalid"))?;
        if name != MANIFEST_FILE
            && name != JOURNAL_FILE
            && name != STAGED_DIRECTORY
            && name != partial_manifest
        {
            return Ok(false);
        }
    }
    reset_prepublication_bundle_files(&bundle)?;
    remove_private_file_if_present(&bundle.dir, JOURNAL_FILE)?;
    let Bundle { dir, staged } = bundle;
    drop(staged);
    dir.remove_dir(STAGED_DIRECTORY)
        .map_err(|_| anyhow!("unable to remove abandoned staged directory"))?;
    sync_dir_handle(&dir)?;
    drop(dir);
    migration
        .dir
        .remove_dir(bundle_name)
        .map_err(|_| anyhow!("unable to remove abandoned memory migration bundle"))?;
    sync_dir_handle(&migration.dir)?;
    Ok(true)
}

fn read_existing_manifest(bundle: &Bundle) -> Result<ImportManifest> {
    let bytes = read_private_regular_file(&bundle.dir, MANIFEST_FILE, 1024 * 1024)?;
    let manifest: ImportManifest =
        serde_json::from_slice(&bytes).map_err(|_| anyhow!("import manifest is invalid"))?;
    if canonical_json_line(&manifest)? != bytes {
        bail!("import manifest is not in its immutable canonical form");
    }
    Ok(manifest)
}

fn validate_stored_manifest(
    manifest: &ImportManifest,
    familiar: &str,
    bundle_name: &str,
) -> Result<()> {
    if manifest.protocol_version != IMPORT_PROTOCOL_VERSION
        || manifest.familiar_id != familiar
        || manifest.bundle_id != bundle_name
    {
        bail!("stored import manifest identity is invalid");
    }
    let mut previous_label: Option<&str> = None;
    let mut targets = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        validate_logical_label(&entry.source_label)?;
        validate_target_name(&entry.target_name)?;
        if entry.byte_length > MAX_AGGREGATE_SOURCE_BYTES
            || entry.digest.len() != 71
            || !entry
                .digest
                .strip_prefix("blake3:")
                .is_some_and(|digest| digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
            || previous_label.is_some_and(|previous| previous >= entry.source_label.as_str())
        {
            bail!("stored import manifest entry is invalid");
        }
        previous_label = Some(&entry.source_label);
        targets.push(entry.target_name.clone());
    }
    validate_unique_target_names(&targets)?;
    if stored_manifest_bundle_id(manifest) != manifest.bundle_id {
        bail!("stored import manifest bundle ID is invalid");
    }
    Ok(())
}

fn stored_manifest_bundle_id(manifest: &ImportManifest) -> String {
    let mut hasher = blake3::Hasher::new();
    update_framed(&mut hasher, b"coven-memory-import-plan-v1");
    update_framed(&mut hasher, manifest.familiar_id.as_bytes());
    update_framed(
        &mut hasher,
        match manifest.source_kind {
            MemoryImportSourceKind::Native => b"native",
            MemoryImportSourceKind::Openclaw => b"openclaw",
        },
    );
    hasher.update(&(manifest.entries.len() as u64).to_le_bytes());
    for entry in &manifest.entries {
        update_framed(&mut hasher, entry.source_label.as_bytes());
        update_framed(&mut hasher, entry.target_name.as_bytes());
        update_framed(&mut hasher, entry.digest.as_bytes());
    }
    format!("blake3-{}", hasher.finalize().to_hex())
}

fn stage_manifest_is_complete(bundle: &Bundle, manifest: &ImportManifest) -> Result<()> {
    for entry in &manifest.entries {
        verify_entry_file(&bundle.staged, &entry.target_name, entry)?;
    }
    Ok(())
}

fn ensure_child_directory(parent: &Dir, name: &str, private: bool) -> Result<Dir> {
    ensure_child_directory_with_status(parent, name, private).map(|(directory, _)| directory)
}

fn ensure_child_directory_with_status(
    parent: &Dir,
    name: &str,
    private: bool,
) -> Result<(Dir, bool)> {
    validate_directory_component(name)?;
    reject_portable_sibling_collision(parent, name)?;
    let created = match parent.symlink_metadata(name) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
                bail!("import path component is not a real directory");
            }
            false
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            parent
                .create_dir(name)
                .map_err(|_| anyhow!("unable to create private import directory"))?;
            true
        }
        Err(_) => bail!("unable to inspect private import directory"),
    };
    let directory = parent
        .open_dir_nofollow(name)
        .map_err(|_| anyhow!("import path component is not a real directory"))?;
    let metadata = directory
        .dir_metadata()
        .map_err(|_| anyhow!("unable to inspect private import directory"))?;
    if !metadata.is_dir() || metadata_is_windows_reparse_point(&metadata) {
        bail!("import path component is not a real directory");
    }
    if private {
        if created {
            secure_private_directory_handle(&directory)?;
        }
        validate_private_directory_handle(&directory)?;
    }
    if created {
        sync_dir_handle(&directory)?;
        sync_dir_handle(parent)?;
    }
    Ok((directory, created))
}

fn validate_directory_component(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':'])
        || name.chars().any(char::is_control)
        || Path::new(name).components().count() != 1
        || is_windows_device_stem(name)
    {
        bail!("import path contains an unsafe component");
    }
    Ok(())
}

fn reject_portable_sibling_collision(parent: &Dir, requested: &str) -> Result<()> {
    let requested_key = crate::ward::portable_surface_key(requested);
    for entry in parent
        .entries()
        .map_err(|_| anyhow!("unable to enumerate import directory"))?
    {
        let entry = entry.map_err(|_| anyhow!("unable to enumerate import directory"))?;
        let Some(name) = entry.file_name().into_string().ok() else {
            continue;
        };
        if crate::ward::portable_surface_key(&name) == requested_key && name != requested {
            bail!("import path has a portable-name collision");
        }
    }
    Ok(())
}

#[cfg(unix)]
fn secure_private_directory_handle(directory: &Dir) -> Result<()> {
    use cap_std::fs::PermissionsExt;

    directory
        .set_permissions(".", cap_std::fs::Permissions::from_mode(0o700))
        .map_err(|_| anyhow!("unable to secure private import directory"))
}

#[cfg(unix)]
fn validate_private_directory_handle(directory: &Dir) -> Result<()> {
    let metadata = directory
        .dir_metadata()
        .map_err(|_| anyhow!("unable to inspect private import directory"))?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o777 != 0o700 {
        bail!("private import directory has unsafe ownership or permissions");
    }
    Ok(())
}

#[cfg(unix)]
fn sync_dir_handle(directory: &Dir) -> Result<()> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    directory
        .open_with(".", &options)
        .and_then(|file| file.sync_all())
        .map_err(|_| anyhow!("unable to sync import directory"))
}

#[cfg(not(unix))]
fn secure_private_directory_handle(_directory: &Dir) -> Result<()> {
    bail!("private memory migration directories are unsupported on this platform")
}

#[cfg(not(unix))]
fn validate_private_directory_handle(_directory: &Dir) -> Result<()> {
    bail!("private memory migration directories are unsupported on this platform")
}

#[cfg(not(unix))]
fn sync_dir_handle(_directory: &Dir) -> Result<()> {
    bail!("durable memory migration directories are unsupported on this platform")
}

#[cfg(not(unix))]
fn secure_private_file_handle(_file: &cap_std::fs::File) -> Result<()> {
    bail!("private memory migration files are unsupported on this platform")
}

#[cfg(not(unix))]
fn validate_private_file_handle(_file: &cap_std::fs::File) -> Result<()> {
    bail!("private memory migration files are unsupported on this platform")
}

fn canonical_json_line<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn create_or_validate_manifest(
    bundle: &Bundle,
    expected: &ImportManifest,
) -> Result<ImportManifest> {
    let expected_bytes = canonical_json_line(expected)?;
    match bundle.dir.symlink_metadata(MANIFEST_FILE) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_private_file_atomic(&bundle.dir, MANIFEST_FILE, &expected_bytes)?;
            sync_dir_handle(&bundle.dir)?;
            Ok(expected.clone())
        }
        Ok(_) => {
            let bytes = read_private_regular_file(&bundle.dir, MANIFEST_FILE, 1024 * 1024);
            let actual = bytes
                .as_ref()
                .ok()
                .and_then(|bytes| serde_json::from_slice::<ImportManifest>(bytes).ok());
            if let (Ok(bytes), Some(actual)) = (&bytes, actual) {
                validate_existing_manifest(&actual, expected)?;
                if canonical_json_line(&actual)? != *bytes {
                    bail!("import manifest is not in its immutable canonical form");
                }
                remove_private_file_if_present(&bundle.dir, &partial_file_name(MANIFEST_FILE))?;
                return Ok(actual);
            }
            if !bundle_is_prepublication(bundle)? {
                bail!("import manifest is invalid");
            }
            reset_prepublication_bundle_files(bundle)?;
            create_private_file_atomic(&bundle.dir, MANIFEST_FILE, &expected_bytes)?;
            Ok(expected.clone())
        }
        Err(_) => bail!("unable to inspect import manifest"),
    }
}

fn validate_existing_manifest(actual: &ImportManifest, expected: &ImportManifest) -> Result<()> {
    if actual != expected {
        bail!("existing import manifest does not match the deterministic plan");
    }
    Ok(())
}

fn create_private_file(directory: &Dir, name: &str, bytes: &[u8]) -> Result<cap_std::fs::File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = directory
        .open_with(name, &options)
        .map_err(|_| anyhow!("unable to create private import file"))?;
    secure_private_file_handle(&file)?;
    file.write_all(bytes)
        .map_err(|_| anyhow!("unable to write private import file"))?;
    file.sync_all()
        .map_err(|_| anyhow!("unable to sync private import file"))?;
    validate_private_file_handle(&file)?;
    Ok(file)
}

fn create_private_file_atomic(directory: &Dir, name: &str, bytes: &[u8]) -> Result<()> {
    let partial = partial_file_name(name);
    remove_private_file_if_present(directory, &partial)?;
    create_private_file(directory, &partial, bytes)?;
    directory
        .hard_link(&partial, directory, name)
        .map_err(|_| anyhow!("unable to publish private import file"))?;
    sync_dir_handle(directory)?;
    directory
        .remove_file(&partial)
        .map_err(|_| anyhow!("unable to clean private import file"))?;
    sync_dir_handle(directory)
}

fn partial_file_name(name: &str) -> String {
    format!(".{name}.partial")
}

fn remove_private_file_if_present(directory: &Dir, name: &str) -> Result<()> {
    match directory.symlink_metadata(name) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata_is_windows_reparse_point(&metadata) {
                bail!("private import file is unsafe");
            }
            let mut options = OpenOptions::new();
            options
                .read(true)
                .follow(FollowSymlinks::No)
                .maybe_dir(true);
            let file = directory
                .open_with(name, &options)
                .map_err(|_| anyhow!("unable to open private import file"))?;
            validate_private_file_handle(&file)?;
            drop(file);
            directory
                .remove_file(name)
                .map_err(|_| anyhow!("unable to remove private import file"))?;
            sync_dir_handle(directory)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => bail!("unable to inspect private import file"),
    }
    Ok(())
}

fn read_private_regular_file(directory: &Dir, name: &str, max_len: u64) -> Result<Vec<u8>> {
    let metadata = directory
        .symlink_metadata(name)
        .map_err(|_| anyhow!("unable to inspect private import file"))?;
    if !metadata.is_file()
        || metadata_is_windows_reparse_point(&metadata)
        || metadata.len() > max_len
    {
        bail!("private import file is unsafe");
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    let mut file = directory
        .open_with(name, &options)
        .map_err(|_| anyhow!("unable to open private import file"))?;
    validate_private_file_handle(&file)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|_| anyhow!("unable to read private import file"))?;
    if bytes.len() as u64 != metadata.len() {
        bail!("private import file changed while reading");
    }
    Ok(bytes)
}

#[cfg(unix)]
fn secure_private_file_handle(file: &cap_std::fs::File) -> Result<()> {
    use cap_std::fs::PermissionsExt;

    file.set_permissions(cap_std::fs::Permissions::from_mode(0o600))
        .map_err(|_| anyhow!("unable to secure private import file"))
}

#[cfg(unix)]
fn validate_private_file_handle(file: &cap_std::fs::File) -> Result<()> {
    let metadata = file
        .metadata()
        .map_err(|_| anyhow!("unable to inspect private import file"))?;
    if !metadata.is_file()
        || metadata_is_windows_reparse_point(&metadata)
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o600
    {
        bail!("private import file has unsafe ownership or permissions");
    }
    Ok(())
}

#[cfg(unix)]
fn validate_private_mutable_file_handle(file: &cap_std::fs::File) -> Result<()> {
    validate_private_file_handle(file)?;
    let metadata = file
        .metadata()
        .map_err(|_| anyhow!("unable to inspect private import file"))?;
    if metadata.nlink() != 1 {
        bail!("private mutable import file is unsafe");
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_mutable_file_handle(file: &cap_std::fs::File) -> Result<()> {
    validate_private_file_handle(file)
}

fn open_or_create_journal(bundle: &Bundle) -> Result<JournalSummary> {
    match bundle.dir.symlink_metadata(JOURNAL_FILE) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_private_file(&bundle.dir, JOURNAL_FILE, &[])?;
            sync_dir_handle(&bundle.dir)?;
        }
        Ok(_) => {}
        Err(_) => bail!("unable to inspect import journal"),
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    let mut file = bundle
        .dir
        .open_with(JOURNAL_FILE, &options)
        .map_err(|_| anyhow!("unable to open import journal"))?;
    validate_private_mutable_file_handle(&file)?;
    let metadata = file
        .metadata()
        .map_err(|_| anyhow!("unable to inspect import journal"))?;
    if metadata.len() > 16 * 1024 * 1024 {
        bail!("import journal is too large");
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|_| anyhow!("unable to read import journal"))?;
    if bytes.len() as u64 != metadata.len() {
        bail!("import journal changed while reading");
    }
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        let durable_len = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        file.set_len(
            u64::try_from(durable_len).map_err(|_| anyhow!("import journal is too large"))?,
        )
        .map_err(|_| anyhow!("unable to repair import journal"))?;
        file.sync_all()
            .map_err(|_| anyhow!("unable to sync repaired import journal"))?;
        sync_dir_handle(&bundle.dir)?;
        bytes.truncate(durable_len);
    }
    parse_journal(&bytes)
}

fn parse_journal(bytes: &[u8]) -> Result<JournalSummary> {
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bail!("import journal has a torn final record");
    }
    let mut summary = JournalSummary::default();
    for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let record: JournalRecord =
            serde_json::from_slice(line).map_err(|_| anyhow!("import journal is invalid"))?;
        if record.protocol_version != IMPORT_PROTOCOL_VERSION || record.sequence != index as u64 {
            bail!("import journal sequence is invalid");
        }
        validate_journal_transition(&summary, &record)?;
        summary.apply(record);
    }
    Ok(summary)
}

fn read_existing_journal(bundle: &Bundle) -> Result<JournalSummary> {
    let bytes = read_private_regular_file(&bundle.dir, JOURNAL_FILE, 16 * 1024 * 1024)?;
    parse_journal(&bytes)
}

fn validate_journal_against_manifest(
    journal: &JournalSummary,
    manifest: &ImportManifest,
) -> Result<()> {
    let entries = manifest
        .entries
        .iter()
        .map(|entry| (entry.target_name.as_str(), entry))
        .collect::<HashMap<_, _>>();
    for record in &journal.records {
        match record.scope {
            JournalScope::Bundle => {
                let valid_outcome = match record.state {
                    JournalState::RolledBack | JournalState::Restored => {
                        matches!(record.outcome, None | Some(JournalOutcome::ManualRecovery))
                    }
                    JournalState::Invalidated => {
                        record.outcome == Some(JournalOutcome::ManualRecovery)
                    }
                    _ => record.outcome.is_none(),
                };
                if !valid_outcome {
                    bail!("import journal is not bound to the manifest protocol");
                }
            }
            JournalScope::Entry => {
                let target_name = record
                    .target_name
                    .as_deref()
                    .ok_or_else(|| anyhow!("import journal is not bound to the manifest"))?;
                let entry = entries
                    .get(target_name)
                    .ok_or_else(|| anyhow!("import journal is not bound to the manifest"))?;
                if record.digest.as_deref() != Some(entry.digest.as_str())
                    || record.byte_length != Some(entry.byte_length)
                {
                    bail!("import journal is not bound to the manifest");
                }
                let valid_outcome = match record.state {
                    JournalState::Prepared => record.outcome.is_none(),
                    JournalState::Published => record.outcome == Some(JournalOutcome::Created),
                    JournalState::RollingBack => matches!(
                        record.outcome,
                        Some(
                            JournalOutcome::Created
                                | JournalOutcome::NotPublished
                                | JournalOutcome::ManualRecovery
                        )
                    ),
                    JournalState::Verified => matches!(
                        record.outcome,
                        Some(JournalOutcome::Created | JournalOutcome::Unchanged)
                    ),
                    JournalState::Restoring => matches!(
                        record.outcome,
                        Some(
                            JournalOutcome::Created
                                | JournalOutcome::Unchanged
                                | JournalOutcome::ManualRecovery
                        )
                    ),
                    JournalState::Restored => matches!(
                        record.outcome,
                        Some(
                            JournalOutcome::Suppressed
                                | JournalOutcome::Unchanged
                                | JournalOutcome::ManualRecovery
                        )
                    ),
                    JournalState::Reactivating => false,
                    JournalState::Invalidated => false,
                    JournalState::RolledBack => matches!(
                        record.outcome,
                        Some(
                            JournalOutcome::NotPublished
                                | JournalOutcome::Removed
                                | JournalOutcome::ManualRecovery
                        )
                    ),
                    JournalState::Publishing => false,
                };
                if !valid_outcome {
                    bail!("import journal is not bound to the manifest protocol");
                }
            }
        }
    }
    if journal.bundle_state == Some(JournalState::Verified)
        && manifest.entries.iter().any(|entry| {
            journal
                .entry_states
                .get(&entry.target_name)
                .is_none_or(|record| record.state != JournalState::Verified)
        })
    {
        bail!("verified import journal is incomplete for its manifest");
    }
    if journal.bundle_state == Some(JournalState::RolledBack)
        && journal.entry_states.values().any(|record| {
            matches!(
                record.outcome,
                Some(JournalOutcome::Created | JournalOutcome::Removed)
            ) && record.state != JournalState::RolledBack
        })
    {
        bail!("rolled-back import journal is incomplete for its manifest");
    }
    if journal.bundle_state == Some(JournalState::Restored)
        && manifest.entries.iter().any(|entry| {
            journal
                .entry_states
                .get(&entry.target_name)
                .is_none_or(|record| record.state != JournalState::Restored)
        })
    {
        bail!("restored import journal is incomplete for its manifest");
    }
    Ok(())
}

fn journal_requires_manual_recovery(journal: &JournalSummary) -> bool {
    journal
        .records
        .iter()
        .any(|record| matches!(record.outcome, Some(JournalOutcome::ManualRecovery)))
}

fn validate_journal_transition(summary: &JournalSummary, record: &JournalRecord) -> Result<()> {
    match record.scope {
        JournalScope::Bundle => {
            if record.target_name.is_some()
                || record.digest.is_some()
                || record.byte_length.is_some()
            {
                bail!("import journal bundle record is invalid");
            }
            let valid = matches!(
                (summary.bundle_state, record.state),
                (None, JournalState::Prepared)
                    | (Some(JournalState::Prepared), JournalState::Publishing)
                    | (
                        Some(JournalState::Prepared | JournalState::Publishing),
                        JournalState::RollingBack
                    )
                    | (Some(JournalState::Publishing), JournalState::Verified)
                    | (Some(JournalState::Verified), JournalState::Restoring)
                    | (Some(JournalState::Restoring), JournalState::Restored)
                    | (Some(JournalState::Restored), JournalState::Reactivating)
                    | (Some(JournalState::Restored), JournalState::Invalidated)
                    | (Some(JournalState::Reactivating), JournalState::Verified)
                    | (Some(JournalState::Reactivating), JournalState::Invalidated)
                    | (Some(JournalState::RollingBack), JournalState::RolledBack)
            );
            if !valid {
                bail!("import journal bundle transition is invalid");
            }
        }
        JournalScope::Entry => {
            let target = record
                .target_name
                .as_deref()
                .ok_or_else(|| anyhow!("import journal entry record is invalid"))?;
            validate_target_name(target)?;
            if record.digest.is_none() || record.byte_length.is_none() {
                bail!("import journal entry record is invalid");
            }
            let bundle_allows_entry = match record.state {
                JournalState::Prepared | JournalState::Published => {
                    summary.bundle_state == Some(JournalState::Publishing)
                }
                JournalState::Verified => matches!(
                    summary.bundle_state,
                    Some(JournalState::Publishing | JournalState::Reactivating)
                ),
                JournalState::Restoring | JournalState::Restored => {
                    summary.bundle_state == Some(JournalState::Restoring)
                }
                JournalState::RollingBack | JournalState::RolledBack => {
                    summary.bundle_state == Some(JournalState::RollingBack)
                }
                JournalState::Publishing
                | JournalState::Reactivating
                | JournalState::Invalidated => false,
            };
            if !bundle_allows_entry {
                bail!("import journal entry appears outside its bundle phase");
            }
            let previous = summary.entry_states.get(target);
            let valid = if record.state == JournalState::Verified
                && record.outcome == Some(JournalOutcome::Unchanged)
                && previous.is_some_and(|event| {
                    event.state == JournalState::Verified
                        && event.outcome == Some(JournalOutcome::Created)
                }) {
                true
            } else if record.outcome == Some(JournalOutcome::NotPublished) {
                matches!(
                    (
                        previous.map(|event| (event.state, event.outcome)),
                        record.state
                    ),
                    (
                        Some((JournalState::Prepared, None)),
                        JournalState::RollingBack
                    ) | (
                        Some((
                            JournalState::RollingBack,
                            Some(JournalOutcome::NotPublished)
                        )),
                        JournalState::RolledBack
                    )
                )
            } else if previous
                .is_some_and(|event| event.outcome == Some(JournalOutcome::NotPublished))
            {
                false
            } else {
                matches!(
                    (previous.map(|event| event.state), record.state),
                    (None, JournalState::Prepared)
                        | (Some(JournalState::Prepared), JournalState::Published)
                        | (
                            Some(JournalState::Prepared | JournalState::Published),
                            JournalState::Verified
                        )
                        | (Some(JournalState::Verified), JournalState::Restoring)
                        | (Some(JournalState::Restoring), JournalState::Restored)
                        | (Some(JournalState::Restored), JournalState::Verified)
                        | (
                            Some(
                                JournalState::Prepared
                                    | JournalState::Published
                                    | JournalState::Verified
                            ),
                            JournalState::RollingBack
                        )
                        | (
                            Some(
                                JournalState::Prepared
                                    | JournalState::Published
                                    | JournalState::Verified
                                    | JournalState::RollingBack
                            ),
                            JournalState::RolledBack
                        )
                )
            };
            if !valid {
                bail!("import journal entry transition is invalid");
            }
        }
    }
    Ok(())
}

fn append_journal(
    bundle: &Bundle,
    summary: &mut JournalSummary,
    scope: JournalScope,
    state: JournalState,
    entry: Option<&ManifestEntry>,
    outcome: Option<JournalOutcome>,
) -> Result<()> {
    let record = JournalRecord {
        protocol_version: IMPORT_PROTOCOL_VERSION,
        sequence: summary.next_sequence()?,
        scope,
        state,
        target_name: entry.map(|entry| entry.target_name.clone()),
        digest: entry.map(|entry| entry.digest.clone()),
        byte_length: entry.map(|entry| entry.byte_length),
        outcome,
    };
    write_journal_record(bundle, summary, record, None)
}

fn write_journal_record(
    bundle: &Bundle,
    summary: &mut JournalSummary,
    record: JournalRecord,
    hook: Option<&mut dyn ApplyHook>,
) -> Result<()> {
    validate_journal_transition(summary, &record)?;
    let bytes = canonical_json_line(&record)?;
    let target_name = record.target_name.clone();
    let mut hook = hook;
    if let Some(target_name) = target_name.as_ref() {
        if let Some(hook) = hook.as_mut() {
            run_apply_step(
                &mut **hook,
                ApplyStep::BeforePublishedJournalWrite(target_name.clone()),
            )?;
        }
    }
    let mut options = OpenOptions::new();
    options
        .append(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    let mut file = bundle
        .dir
        .open_with(JOURNAL_FILE, &options)
        .map_err(|_| anyhow!("unable to append import journal"))?;
    validate_private_mutable_file_handle(&file)?;
    file.write_all(&bytes)
        .map_err(|_| anyhow!("unable to append import journal"))?;
    if let Some(target_name) = target_name.as_ref() {
        if let Some(hook) = hook.as_mut() {
            run_apply_step(
                &mut **hook,
                ApplyStep::AfterPublishedJournalWriteBeforeSync(target_name.clone()),
            )?;
        }
    }
    file.sync_all()
        .map_err(|_| anyhow!("unable to sync import journal"))?;
    if record.state == JournalState::Published {
        if let (Some(hook), Some(target_name)) = (hook.as_mut(), target_name.as_ref()) {
            run_apply_step(
                &mut **hook,
                ApplyStep::AfterPublishedJournalSync(target_name.clone()),
            )?;
        }
    }
    summary.apply(record);
    Ok(())
}

fn stage_manifest_entries(
    bundle: &Bundle,
    manifest: &ImportManifest,
    sources: &HashMap<&str, &DiscoveredSource>,
    journal: &JournalSummary,
    hook: &mut dyn ApplyHook,
) -> Result<()> {
    for entry in &manifest.entries {
        let source = sources
            .get(entry.source_label.as_str())
            .ok_or_else(|| anyhow!("apply inputs do not match the import manifest"))?;
        match bundle.staged.symlink_metadata(&entry.target_name) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_private_file_atomic(&bundle.staged, &entry.target_name, &source.bytes)?;
                sync_dir_handle(&bundle.staged)?;
            }
            Ok(_) if verify_entry_file(&bundle.staged, &entry.target_name, entry).is_ok() => {
                remove_private_file_if_present(
                    &bundle.staged,
                    &partial_file_name(&entry.target_name),
                )?;
            }
            Ok(metadata)
                if journal.bundle_state == Some(JournalState::Prepared)
                    && !journal.entry_states.contains_key(&entry.target_name)
                    && metadata.is_file()
                    && !metadata_is_windows_reparse_point(&metadata)
                    && metadata.len() < entry.byte_length =>
            {
                remove_private_file_if_present(&bundle.staged, &entry.target_name)?;
                remove_private_file_if_present(
                    &bundle.staged,
                    &partial_file_name(&entry.target_name),
                )?;
                create_private_file_atomic(&bundle.staged, &entry.target_name, &source.bytes)?;
            }
            Ok(_) => bail!("import file failed digest verification"),
            Err(_) => bail!("unable to inspect staged import file"),
        }
        verify_entry_file(&bundle.staged, &entry.target_name, entry)?;
        run_apply_step(hook, ApplyStep::AfterStage(entry.target_name.clone()))?;
    }
    Ok(())
}

fn verify_entry_file(directory: &Dir, name: &str, entry: &ManifestEntry) -> Result<()> {
    let metadata = directory
        .symlink_metadata(name)
        .map_err(|_| anyhow!("unable to inspect import file"))?;
    if !metadata.is_file()
        || metadata_is_windows_reparse_point(&metadata)
        || metadata.len() != entry.byte_length
    {
        bail!("import file failed length or type verification");
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    let mut file = directory
        .open_with(name, &options)
        .map_err(|_| anyhow!("unable to open import file for verification"))?;
    let digest = digest_stable_opened_file(&mut file)
        .ok_or_else(|| anyhow!("import file changed during verification"))?;
    if digest != entry.digest {
        bail!("import file failed digest verification");
    }
    Ok(())
}

fn open_target_directory(coven_home: &Path, familiar: &str) -> Result<Dir> {
    let home = open_dir_path_nofollow(coven_home)
        .map_err(|_| anyhow!("COVEN_HOME must be a real directory"))?;
    let memory = ensure_child_directory(&home, "memory", false)?;
    reject_portable_sibling_collision(&memory, familiar)?;
    ensure_child_directory(&memory, familiar, false)
}

fn open_target_context(
    coven_home: &Path,
    familiar: &str,
    bundle_id: &str,
) -> Result<TargetContext> {
    let dir = open_target_directory(coven_home, familiar)?;
    let work_root = ensure_child_directory(&dir, TARGET_WORK_DIRECTORY, true)?;
    let work = ensure_child_directory(&work_root, &bundle_component(bundle_id)?, true)?;
    Ok(TargetContext { dir, work })
}

fn candidate_name(entry: &ManifestEntry) -> String {
    format!("{}.candidate", entry.target_name)
}

fn quarantine_name(entry: &ManifestEntry) -> String {
    format!("{}.rollback", entry.target_name)
}

fn ensure_publish_candidate(
    target: &TargetContext,
    bundle: &Bundle,
    entry: &ManifestEntry,
) -> Result<Option<cap_std::fs::File>> {
    let name = candidate_name(entry);
    match target.work.symlink_metadata(&name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let bytes =
                read_private_regular_file(&bundle.staged, &entry.target_name, entry.byte_length)?;
            let file = create_private_file(&target.work, &name, &bytes)?;
            sync_dir_handle(&target.work)?;
            verify_open_private_entry_file(file, entry).map(Some)
        }
        Ok(_) => {
            verify_private_entry_file(&target.work, &name, entry)?;
            Ok(None)
        }
        Err(_) => bail!("unable to inspect import publication candidate"),
    }
}

fn verify_open_private_entry_file(
    mut file: cap_std::fs::File,
    entry: &ManifestEntry,
) -> Result<cap_std::fs::File> {
    validate_private_file_handle(&file)?;
    let metadata = file
        .metadata()
        .map_err(|_| anyhow!("unable to inspect private import file"))?;
    if metadata.len() != entry.byte_length {
        bail!("import file failed length or type verification");
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| anyhow!("unable to seek private import file"))?;
    let digest = digest_stable_opened_file(&mut file)
        .ok_or_else(|| anyhow!("import file changed during verification"))?;
    if digest != entry.digest {
        bail!("import file failed digest verification");
    }
    Ok(file)
}

#[cfg(unix)]
fn same_file_identity_with_handle(
    directory: &Dir,
    name: &str,
    trusted: &cap_std::fs::File,
) -> Result<bool> {
    let named = directory
        .symlink_metadata(name)
        .map_err(|_| anyhow!("unable to inspect import file identity"))?;
    let trusted = trusted
        .metadata()
        .map_err(|_| anyhow!("unable to inspect trusted import file identity"))?;
    Ok(named.dev() == trusted.dev() && named.ino() == trusted.ino())
}

#[cfg(not(unix))]
fn same_file_identity_with_handle(
    _directory: &Dir,
    _name: &str,
    _trusted: &cap_std::fs::File,
) -> Result<bool> {
    bail!("trusted file identity is unsupported on this platform")
}

fn verify_private_entry_file(directory: &Dir, name: &str, entry: &ManifestEntry) -> Result<()> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    let file = directory
        .open_with(name, &options)
        .map_err(|_| anyhow!("unable to open private import file"))?;
    validate_private_file_handle(&file)?;
    drop(file);
    verify_entry_file(directory, name, entry)
}

fn remove_candidate_if_present(target: &TargetContext, entry: &ManifestEntry) -> Result<()> {
    let name = candidate_name(entry);
    match target.work.symlink_metadata(&name) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata_is_windows_reparse_point(&metadata) {
                bail!("import publication candidate is unsafe");
            }
            target
                .work
                .remove_file(&name)
                .map_err(|_| anyhow!("unable to remove import publication candidate"))?;
            sync_dir_handle(&target.work)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => bail!("unable to inspect import publication candidate"),
    }
    Ok(())
}

fn publish_entries(
    coven_home: &Path,
    plan: &ImportPlan,
    manifest: &ImportManifest,
    bundle: &Bundle,
    journal: &mut JournalSummary,
    hook: &mut dyn ApplyHook,
    current_ownership: &mut CurrentApplyOwnership,
) -> Result<ImportPlan> {
    let target = open_target_context(coven_home, &manifest.familiar_id, &manifest.bundle_id)?;
    let mut created = 0_usize;
    let mut unchanged = 0_usize;
    let mut statuses = HashMap::new();

    for entry in &manifest.entries {
        verify_entry_file(&bundle.staged, &entry.target_name, entry)?;
        let existing = journal.entry_states.get(&entry.target_name).cloned();
        match existing.as_ref() {
            None => {
                remove_candidate_if_present(&target, entry)?;
                let candidate = ensure_publish_candidate(&target, bundle, entry)?
                    .ok_or_else(|| anyhow!("unable to create fresh publication candidate"))?;
                current_ownership
                    .candidates
                    .insert(entry.target_name.clone(), candidate);
                run_apply_step(
                    hook,
                    ApplyStep::AfterCandidateSyncBeforePrepared(entry.target_name.clone()),
                )?;
                append_journal(
                    bundle,
                    journal,
                    JournalScope::Entry,
                    JournalState::Prepared,
                    Some(entry),
                    None,
                )?;
            }
            Some(record)
                if record.state == JournalState::Verified
                    && record.outcome == Some(JournalOutcome::Unchanged) => {}
            Some(record) if record.state == JournalState::Prepared && record.outcome.is_none() => {
                remove_candidate_if_present(&target, entry)?;
                let candidate = ensure_publish_candidate(&target, bundle, entry)?
                    .ok_or_else(|| anyhow!("unable to refresh import publication candidate"))?;
                current_ownership
                    .candidates
                    .insert(entry.target_name.clone(), candidate);
            }
            Some(_) => {
                if !verify_optional_private_entry(&target.work, &candidate_name(entry), entry)? {
                    bail!("import publication candidate is missing");
                }
            }
        }

        let trusted_candidate = current_ownership.candidates.get(&entry.target_name);
        let ownership = inspect_target_ownership(
            &target,
            entry,
            journal.entry_states.get(&entry.target_name),
            trusted_candidate,
        )?;
        let outcome = match ownership {
            TargetOwnership::Ours => {
                append_published_if_needed(bundle, journal, entry, hook)?;
                JournalOutcome::Created
            }
            TargetOwnership::MatchingOther => JournalOutcome::Unchanged,
            TargetOwnership::Missing => {
                if trusted_candidate.is_none()
                    && existing.as_ref().is_some_and(|record| {
                        matches!(
                            (record.state, record.outcome),
                            (
                                JournalState::Published
                                    | JournalState::Verified
                                    | JournalState::RollingBack,
                                Some(JournalOutcome::Created)
                            )
                        )
                    })
                {
                    bail!("durably published import target is missing");
                }
                run_apply_step(hook, ApplyStep::BeforePublish(entry.target_name.clone()))?;
                let ownership = inspect_target_ownership(
                    &target,
                    entry,
                    journal.entry_states.get(&entry.target_name),
                    trusted_candidate,
                )?;
                match ownership {
                    TargetOwnership::Missing => {
                        run_apply_step(
                            hook,
                            ApplyStep::BeforeAtomicPublish(entry.target_name.clone()),
                        )?;
                        match target.work.hard_link(
                            candidate_name(entry),
                            &target.dir,
                            &entry.target_name,
                        ) {
                            Ok(()) => {}
                            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                                mark_entry_not_published(bundle, journal, &target, entry, hook)?;
                                bail!("import target changed during atomic publication");
                            }
                            Err(_) => bail!("atomic no-replace publication failed"),
                        }
                        sync_dir_handle(&target.dir)?;
                        run_apply_step(
                            hook,
                            ApplyStep::AfterPublishBeforeJournal(entry.target_name.clone()),
                        )?;
                        append_published_if_needed(bundle, journal, entry, hook)?;
                        JournalOutcome::Created
                    }
                    TargetOwnership::Ours => {
                        append_published_if_needed(bundle, journal, entry, hook)?;
                        JournalOutcome::Created
                    }
                    TargetOwnership::MatchingOther => JournalOutcome::Unchanged,
                    TargetOwnership::Conflict => {
                        mark_entry_not_published(bundle, journal, &target, entry, hook)?;
                        bail!("import target changed immediately before publication")
                    }
                }
            }
            TargetOwnership::Conflict => {
                mark_entry_not_published(bundle, journal, &target, entry, hook)?;
                bail!("import target conflicts with the import plan")
            }
        };

        verify_entry_file(&target.dir, &entry.target_name, entry)?;
        if outcome == JournalOutcome::Created {
            let trusted = trusted_candidate
                .ok_or_else(|| anyhow!("published import target has no current-run identity"))?;
            if !same_file_identity_with_handle(&target.dir, &entry.target_name, trusted)?
                || !same_file_identity_with_handle(&target.work, &candidate_name(entry), trusted)?
            {
                bail!("published import target identity changed during verification");
            }
        }
        run_apply_step(
            hook,
            ApplyStep::BeforeVerifiedJournalWrite(entry.target_name.clone()),
        )?;
        append_verified_if_needed(bundle, journal, entry, outcome)?;
        match outcome {
            JournalOutcome::Created => {
                created += 1;
                statuses.insert(entry.target_name.as_str(), PlanEntryStatus::Create);
            }
            JournalOutcome::Unchanged => {
                remove_candidate_if_present(&target, entry)?;
                unchanged += 1;
                statuses.insert(entry.target_name.as_str(), PlanEntryStatus::Unchanged);
            }
            _ => unreachable!("publication produces only created or unchanged outcomes"),
        }
    }

    for entry in &manifest.entries {
        verify_entry_file(&target.dir, &entry.target_name, entry)?;
    }
    append_journal(
        bundle,
        journal,
        JournalScope::Bundle,
        JournalState::Verified,
        None,
        None,
    )?;
    let mut report = plan.clone();
    report.status = ImportPlanStatus::Verified;
    report.apply_eligible = true;
    report.create_count = created;
    report.unchanged_count = unchanged;
    report.conflict_count = 0;
    for entry in &mut report.entries {
        if let Some(status) = statuses.get(entry.target_name.as_str()) {
            entry.status = *status;
        }
    }
    Ok(report)
}

fn mark_entry_not_published(
    bundle: &Bundle,
    journal: &mut JournalSummary,
    target: &TargetContext,
    entry: &ManifestEntry,
    hook: &mut dyn ApplyHook,
) -> Result<()> {
    if journal
        .entry_states
        .get(&entry.target_name)
        .is_none_or(|record| record.state != JournalState::Prepared || record.outcome.is_some())
    {
        bail!("published import target no longer matches its durable ownership record");
    }
    if journal.bundle_state != Some(JournalState::RollingBack) {
        append_journal(
            bundle,
            journal,
            JournalScope::Bundle,
            JournalState::RollingBack,
            None,
            None,
        )?;
    }
    append_journal(
        bundle,
        journal,
        JournalScope::Entry,
        JournalState::RollingBack,
        Some(entry),
        Some(JournalOutcome::NotPublished),
    )?;
    run_apply_step(
        hook,
        ApplyStep::AfterNotPublishedRollingBack(entry.target_name.clone()),
    )?;
    append_journal(
        bundle,
        journal,
        JournalScope::Entry,
        JournalState::RolledBack,
        Some(entry),
        Some(JournalOutcome::NotPublished),
    )?;
    remove_candidate_if_present(target, entry)
}

fn append_published_if_needed(
    bundle: &Bundle,
    journal: &mut JournalSummary,
    entry: &ManifestEntry,
    hook: &mut dyn ApplyHook,
) -> Result<()> {
    if journal
        .entry_states
        .get(&entry.target_name)
        .is_some_and(|record| {
            matches!(
                record.state,
                JournalState::Published | JournalState::Verified
            )
        })
    {
        return Ok(());
    }
    let record = JournalRecord {
        protocol_version: IMPORT_PROTOCOL_VERSION,
        sequence: journal.next_sequence()?,
        scope: JournalScope::Entry,
        state: JournalState::Published,
        target_name: Some(entry.target_name.clone()),
        digest: Some(entry.digest.clone()),
        byte_length: Some(entry.byte_length),
        outcome: Some(JournalOutcome::Created),
    };
    write_journal_record(bundle, journal, record, Some(hook))
}

fn append_verified_if_needed(
    bundle: &Bundle,
    journal: &mut JournalSummary,
    entry: &ManifestEntry,
    outcome: JournalOutcome,
) -> Result<()> {
    if let Some(record) = journal.entry_states.get(&entry.target_name) {
        if record.state == JournalState::Verified && record.outcome == Some(outcome) {
            return Ok(());
        }
        if record.state == JournalState::Verified
            && record.outcome == Some(JournalOutcome::Created)
            && outcome == JournalOutcome::Unchanged
        {
            // Persist the safer ownership classification below.
        } else if record.state == JournalState::Verified {
            bail!("verified import ownership correction is invalid");
        }
    }
    append_journal(
        bundle,
        journal,
        JournalScope::Entry,
        JournalState::Verified,
        Some(entry),
        Some(outcome),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetOwnership {
    Missing,
    Ours,
    MatchingOther,
    Conflict,
}

fn inspect_target_ownership(
    target: &TargetContext,
    entry: &ManifestEntry,
    record: Option<&JournalRecord>,
    trusted_candidate: Option<&cap_std::fs::File>,
) -> Result<TargetOwnership> {
    let metadata = match target.dir.symlink_metadata(&entry.target_name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(TargetOwnership::Missing)
        }
        Err(_) => return Ok(TargetOwnership::Conflict),
    };
    if !metadata.is_file()
        || metadata_is_windows_reparse_point(&metadata)
        || metadata.len() != entry.byte_length
    {
        return Ok(TargetOwnership::Conflict);
    }
    if verify_entry_file(&target.dir, &entry.target_name, entry).is_err() {
        return Ok(TargetOwnership::Conflict);
    }
    let candidate = candidate_name(entry);
    let candidate_exists = match target.work.symlink_metadata(&candidate) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata_is_windows_reparse_point(&metadata) {
                return Ok(TargetOwnership::Conflict);
            }
            verify_private_entry_file(&target.work, &candidate, entry)?;
            true
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(_) => return Ok(TargetOwnership::Conflict),
    };
    let target_aliases_candidate = candidate_exists
        && same_file_identity_named(&target.dir, &entry.target_name, &target.work, &candidate)?;
    let durable_created = record.is_some_and(|record| {
        matches!(
            (record.state, record.outcome),
            (
                JournalState::Published | JournalState::Verified | JournalState::RollingBack,
                Some(JournalOutcome::Created)
            )
        )
    });
    if let Some(trusted) = trusted_candidate {
        if target_aliases_candidate
            && durable_created
            && same_file_identity_with_handle(&target.dir, &entry.target_name, trusted)?
            && same_file_identity_with_handle(&target.work, &candidate, trusted)?
        {
            return Ok(TargetOwnership::Ours);
        }
        if target_aliases_candidate && !durable_created {
            return Ok(TargetOwnership::MatchingOther);
        }
        return Ok(if durable_created {
            TargetOwnership::Conflict
        } else {
            TargetOwnership::MatchingOther
        });
    }
    if target_aliases_candidate {
        Ok(TargetOwnership::MatchingOther)
    } else if durable_created {
        Ok(TargetOwnership::Conflict)
    } else {
        Ok(TargetOwnership::MatchingOther)
    }
}

#[cfg(unix)]
fn same_file_identity_named(
    left: &Dir,
    left_name: &str,
    right: &Dir,
    right_name: &str,
) -> Result<bool> {
    let left = left
        .symlink_metadata(left_name)
        .map_err(|_| anyhow!("unable to inspect published target identity"))?;
    let right = right
        .symlink_metadata(right_name)
        .map_err(|_| anyhow!("unable to inspect staged target identity"))?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(windows)]
fn same_file_identity_named(
    left: &Dir,
    left_name: &str,
    right: &Dir,
    right_name: &str,
) -> Result<bool> {
    Ok(windows_opened_file_identity(left, left_name)?
        == windows_opened_file_identity(right, right_name)?)
}

#[cfg(windows)]
fn windows_opened_file_identity(directory: &Dir, name: &str) -> Result<(u64, [u8; 16])> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
    };

    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    let file = directory
        .open_with(name, &options)
        .map_err(|_| anyhow!("unable to open import file identity"))?;
    let metadata = file
        .metadata()
        .map_err(|_| anyhow!("unable to inspect import file identity"))?;
    if !metadata.is_file() || metadata_is_windows_reparse_point(&metadata) {
        bail!("import file identity is unsafe");
    }
    let mut info = FILE_ID_INFO::default();
    let result = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as _,
            FileIdInfo,
            std::ptr::addr_of_mut!(info).cast(),
            u32::try_from(std::mem::size_of::<FILE_ID_INFO>())
                .expect("FILE_ID_INFO size fits in u32"),
        )
    };
    if result == 0 {
        bail!("unable to inspect import file identity");
    }
    if info.FileId.Identifier == [0; 16] || info.FileId.Identifier == [u8::MAX; 16] {
        bail!("import file identity is unusable");
    }
    Ok((info.VolumeSerialNumber, info.FileId.Identifier))
}

#[cfg(not(any(unix, windows)))]
fn same_file_identity_named(
    _left: &Dir,
    _left_name: &str,
    _right: &Dir,
    _right_name: &str,
) -> Result<bool> {
    bail!("file identity is unsupported on this platform")
}

fn verified_report(
    plan: &ImportPlan,
    manifest: &ImportManifest,
    bundle: &Bundle,
    coven_home: &Path,
    journal: &JournalSummary,
) -> Result<ImportPlan> {
    let target = open_target_directory(coven_home, &manifest.familiar_id)?;
    let mut report = plan.clone();
    report.status = ImportPlanStatus::Verified;
    report.apply_eligible = true;
    report.create_count = 0;
    report.unchanged_count = 0;
    report.conflict_count = 0;
    for entry in &manifest.entries {
        verify_entry_file(&bundle.staged, &entry.target_name, entry)?;
        verify_entry_file(&target, &entry.target_name, entry)?;
        let durable_outcome = journal
            .entry_states
            .get(&entry.target_name)
            .and_then(|record| record.outcome)
            .ok_or_else(|| anyhow!("verified import journal is incomplete"))?;
        if !matches!(
            durable_outcome,
            JournalOutcome::Created | JournalOutcome::Unchanged
        ) {
            bail!("verified import journal has an invalid outcome");
        }
        let current_status = plan
            .entries
            .iter()
            .find(|candidate| candidate.target_name == entry.target_name)
            .map(|candidate| candidate.status)
            .ok_or_else(|| anyhow!("verified import plan is incomplete"))?;
        let status = match current_status {
            PlanEntryStatus::Create => {
                report.create_count += 1;
                PlanEntryStatus::Create
            }
            PlanEntryStatus::Unchanged => {
                report.unchanged_count += 1;
                PlanEntryStatus::Unchanged
            }
            PlanEntryStatus::Restored | PlanEntryStatus::Conflict => {
                bail!("verified import target is unsafe or divergent")
            }
        };
        if let Some(report_entry) = report
            .entries
            .iter_mut()
            .find(|candidate| candidate.target_name == entry.target_name)
        {
            report_entry.status = status;
        }
    }
    Ok(report)
}

fn verified_outcome_for_entry(
    journal: &JournalSummary,
    entry: &ManifestEntry,
) -> Result<JournalOutcome> {
    journal
        .records
        .iter()
        .rev()
        .find(|record| {
            record.scope == JournalScope::Entry
                && record.target_name.as_deref() == Some(entry.target_name.as_str())
                && record.state == JournalState::Verified
        })
        .and_then(|record| record.outcome)
        .filter(|outcome| matches!(outcome, JournalOutcome::Created | JournalOutcome::Unchanged))
        .ok_or_else(|| anyhow!("verified import journal is incomplete"))
}

fn restored_bundle_has_reactivation_provenance(
    coven_home: &Path,
    manifest: &ImportManifest,
    journal: &JournalSummary,
) -> Result<bool> {
    let target = match open_existing_target_context(
        coven_home,
        &manifest.familiar_id,
        &manifest.bundle_id,
    ) {
        Ok(target) => target,
        Err(_) => return Ok(false),
    };
    for entry in &manifest.entries {
        match verified_outcome_for_entry(journal, entry)? {
            JournalOutcome::Created
                if !verify_retained_restore_evidence(
                    &target,
                    &manifest.familiar_id,
                    &manifest.bundle_id,
                    entry,
                ) =>
            {
                return Ok(false);
            }
            JournalOutcome::Created | JournalOutcome::Unchanged => {}
            _ => unreachable!("verified outcomes are filtered by helper"),
        }
    }
    Ok(true)
}

fn resume_reactivation(
    coven_home: &Path,
    bundle: &Bundle,
    manifest: &ImportManifest,
    journal: &mut JournalSummary,
) -> Result<()> {
    if journal.bundle_state == Some(JournalState::Restored) {
        append_journal(
            bundle,
            journal,
            JournalScope::Bundle,
            JournalState::Reactivating,
            None,
            None,
        )?;
    }
    if journal.bundle_state != Some(JournalState::Reactivating) {
        bail!("memory import bundle is not reactivating");
    }
    if !restored_bundle_has_reactivation_provenance(coven_home, manifest, journal)? {
        append_journal(
            bundle,
            journal,
            JournalScope::Bundle,
            JournalState::Invalidated,
            None,
            Some(JournalOutcome::ManualRecovery),
        )?;
        bail!("memory import bundle restore requires manual recovery");
    }
    for entry in &manifest.entries {
        if journal
            .entry_states
            .get(&entry.target_name)
            .is_some_and(|record| record.state == JournalState::Verified)
        {
            continue;
        }
        if journal
            .entry_states
            .get(&entry.target_name)
            .is_none_or(|record| record.state != JournalState::Restored)
        {
            bail!("reactivating import journal is incomplete");
        }
        append_journal(
            bundle,
            journal,
            JournalScope::Entry,
            JournalState::Verified,
            Some(entry),
            Some(verified_outcome_for_entry(journal, entry)?),
        )?;
    }
    append_journal(
        bundle,
        journal,
        JournalScope::Bundle,
        JournalState::Verified,
        None,
        None,
    )
}

fn resume_rollback(
    coven_home: &Path,
    manifest: &ImportManifest,
    bundle: &Bundle,
    journal: &mut JournalSummary,
    hook: &mut dyn ApplyHook,
    trusted_candidates: &HashMap<String, cap_std::fs::File>,
) -> Result<bool> {
    if journal.bundle_state == Some(JournalState::RolledBack) {
        return Ok(journal_requires_manual_recovery(journal));
    }
    if journal.bundle_state.is_none() {
        append_journal(
            bundle,
            journal,
            JournalScope::Bundle,
            JournalState::Prepared,
            None,
            None,
        )?;
    }
    if journal.bundle_state != Some(JournalState::RollingBack) {
        append_journal(
            bundle,
            journal,
            JournalScope::Bundle,
            JournalState::RollingBack,
            None,
            None,
        )?;
    }
    let target = open_target_context(coven_home, &manifest.familiar_id, &manifest.bundle_id)?;
    let mut manual = false;
    for entry in manifest.entries.iter().rev() {
        let Some(record) = journal.entry_states.get(&entry.target_name).cloned() else {
            continue;
        };
        if record.state == JournalState::RolledBack {
            if record.outcome == Some(JournalOutcome::NotPublished) {
                remove_candidate_if_present(&target, entry)?;
            }
            manual |= record.outcome == Some(JournalOutcome::ManualRecovery);
            continue;
        }
        if record.state == JournalState::RollingBack
            && record.outcome == Some(JournalOutcome::NotPublished)
        {
            remove_candidate_if_present(&target, entry)?;
            append_journal(
                bundle,
                journal,
                JournalScope::Entry,
                JournalState::RolledBack,
                Some(entry),
                Some(JournalOutcome::NotPublished),
            )?;
            continue;
        }
        if record.outcome == Some(JournalOutcome::Unchanged) {
            remove_candidate_if_present(&target, entry)?;
            continue;
        }
        let Some(trusted_candidate) = trusted_candidates.get(&entry.target_name) else {
            finish_untrusted_rollback_entry(&target, bundle, journal, entry, &record)?;
            manual = true;
            continue;
        };
        let durably_published = journal.records.iter().any(|candidate| {
            candidate.scope == JournalScope::Entry
                && candidate.target_name.as_deref() == Some(entry.target_name.as_str())
                && matches!(
                    (candidate.state, candidate.outcome),
                    (
                        JournalState::Published | JournalState::Verified,
                        Some(JournalOutcome::Created)
                    )
                )
        });
        if record.state != JournalState::RollingBack {
            append_journal(
                bundle,
                journal,
                JournalScope::Entry,
                JournalState::RollingBack,
                Some(entry),
                Some(JournalOutcome::Created),
            )?;
        }
        run_apply_step(hook, ApplyStep::DuringRollback(entry.target_name.clone()))?;
        let (outcome, removed_target) = rollback_entry(
            &target,
            entry,
            durably_published,
            hook,
            Some(trusted_candidate),
        )?;
        if outcome == JournalOutcome::ManualRecovery {
            manual = true;
        } else if removed_target {
            run_apply_step(
                hook,
                ApplyStep::AfterRollbackRemoveBeforeJournal(entry.target_name.clone()),
            )?;
        };
        append_journal(
            bundle,
            journal,
            JournalScope::Entry,
            JournalState::RolledBack,
            Some(entry),
            Some(outcome),
        )?;
    }
    if journal.bundle_state != Some(JournalState::RolledBack) {
        append_journal(
            bundle,
            journal,
            JournalScope::Bundle,
            JournalState::RolledBack,
            None,
            if manual {
                Some(JournalOutcome::ManualRecovery)
            } else {
                None
            },
        )?;
    }
    Ok(manual)
}

fn finish_untrusted_rollback_entry(
    target: &TargetContext,
    bundle: &Bundle,
    journal: &mut JournalSummary,
    entry: &ManifestEntry,
    record: &JournalRecord,
) -> Result<()> {
    restore_untrusted_quarantine(target, entry)?;
    if record.state != JournalState::RollingBack {
        append_journal(
            bundle,
            journal,
            JournalScope::Entry,
            JournalState::RollingBack,
            Some(entry),
            Some(JournalOutcome::ManualRecovery),
        )?;
    }
    append_journal(
        bundle,
        journal,
        JournalScope::Entry,
        JournalState::RolledBack,
        Some(entry),
        Some(JournalOutcome::ManualRecovery),
    )
}

fn restore_untrusted_quarantine(target: &TargetContext, entry: &ManifestEntry) -> Result<()> {
    let quarantine = quarantine_name(entry);
    if !optional_path(&target.work, &quarantine)? {
        return Ok(());
    }
    sync_dir_handle(&target.work)?;
    sync_dir_handle(&target.dir)?;
    match target.dir.symlink_metadata(&entry.target_name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            rename_noreplace(&target.work, &quarantine, &target.dir, &entry.target_name)
                .map_err(|_| anyhow!("unable to restore quarantined target"))?;
            sync_dir_handle(&target.dir)?;
            sync_dir_handle(&target.work)?;
        }
        Ok(_) | Err(_) => {}
    }
    Ok(())
}

fn rollback_entry(
    target: &TargetContext,
    entry: &ManifestEntry,
    durably_published: bool,
    hook: &mut dyn ApplyHook,
    trusted_candidate: Option<&cap_std::fs::File>,
) -> Result<(JournalOutcome, bool)> {
    let candidate = candidate_name(entry);
    let quarantine = quarantine_name(entry);
    let candidate_exists = optional_private_regular_file(&target.work, &candidate)?;
    let quarantine_exists = optional_path(&target.work, &quarantine)?;

    if quarantine_exists {
        return finish_quarantined_rollback(target, entry, candidate_exists, trusted_candidate);
    }

    match target.dir.symlink_metadata(&entry.target_name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if candidate_exists {
                if durably_published {
                    return Ok((JournalOutcome::ManualRecovery, false));
                }
                remove_candidate_if_present(target, entry)?;
            }
            Ok((JournalOutcome::Removed, false))
        }
        Err(_) => Ok((JournalOutcome::ManualRecovery, false)),
        Ok(_) => {
            let Some(trusted_candidate) = trusted_candidate else {
                return Ok((JournalOutcome::ManualRecovery, false));
            };
            if !candidate_exists
                || !same_file_identity_with_handle(
                    &target.dir,
                    &entry.target_name,
                    trusted_candidate,
                )?
                || !same_file_identity_with_handle(&target.work, &candidate, trusted_candidate)?
                || verify_entry_file(&target.dir, &entry.target_name, entry).is_err()
                || verify_entry_file(&target.work, &candidate, entry).is_err()
            {
                return Ok((JournalOutcome::ManualRecovery, false));
            }
            run_apply_step(
                hook,
                ApplyStep::BeforeRollbackQuarantine(entry.target_name.clone()),
            )?;
            rename_noreplace(&target.dir, &entry.target_name, &target.work, &quarantine)
                .map_err(|_| anyhow!("unable to quarantine published import target"))?;
            run_apply_step(
                hook,
                ApplyStep::AfterRollbackQuarantineBeforeSync(entry.target_name.clone()),
            )?;
            sync_dir_handle(&target.work)?;
            sync_dir_handle(&target.dir)?;
            finish_quarantined_rollback(target, entry, true, Some(trusted_candidate))
        }
    }
}

fn finish_quarantined_rollback(
    target: &TargetContext,
    entry: &ManifestEntry,
    candidate_exists: bool,
    trusted_candidate: Option<&cap_std::fs::File>,
) -> Result<(JournalOutcome, bool)> {
    let candidate = candidate_name(entry);
    let quarantine = quarantine_name(entry);
    sync_dir_handle(&target.work)?;
    sync_dir_handle(&target.dir)?;
    let owned = if let Some(trusted) = trusted_candidate {
        candidate_exists
            && verify_entry_file(&target.work, &candidate, entry).is_ok()
            && verify_entry_file(&target.work, &quarantine, entry).is_ok()
            && same_file_identity_with_handle(&target.work, &candidate, trusted)?
            && same_file_identity_with_handle(&target.work, &quarantine, trusted)?
    } else {
        false
    };
    if owned {
        match target.dir.symlink_metadata(&entry.target_name) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) | Err(_) => return Ok((JournalOutcome::ManualRecovery, false)),
        }
        target
            .work
            .remove_file(&quarantine)
            .map_err(|_| anyhow!("unable to remove quarantined import target"))?;
        target
            .work
            .remove_file(&candidate)
            .map_err(|_| anyhow!("unable to remove import publication candidate"))?;
        sync_dir_handle(&target.work)?;
        return Ok((JournalOutcome::Removed, true));
    }
    match target.dir.symlink_metadata(&entry.target_name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            rename_noreplace(&target.work, &quarantine, &target.dir, &entry.target_name)
                .map_err(|_| anyhow!("unable to restore quarantined user target"))?;
            sync_dir_handle(&target.dir)?;
            sync_dir_handle(&target.work)?;
        }
        Ok(_) | Err(_) => {}
    }
    Ok((JournalOutcome::ManualRecovery, false))
}

fn optional_private_regular_file(directory: &Dir, name: &str) -> Result<bool> {
    match directory.symlink_metadata(name) {
        Ok(_) => {
            let mut options = OpenOptions::new();
            options
                .read(true)
                .follow(FollowSymlinks::No)
                .maybe_dir(true);
            let file = directory
                .open_with(name, &options)
                .map_err(|_| anyhow!("unable to open private import work file"))?;
            validate_private_file_handle(&file)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => bail!("unable to inspect private import work file"),
    }
}

fn verify_optional_private_entry(
    directory: &Dir,
    name: &str,
    entry: &ManifestEntry,
) -> Result<bool> {
    match directory.symlink_metadata(name) {
        Ok(_) => {
            verify_private_entry_file(directory, name, entry)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => bail!("unable to inspect private import work file"),
    }
}

fn optional_path(directory: &Dir, name: &str) -> Result<bool> {
    match directory.symlink_metadata(name) {
        Ok(metadata) if !metadata_is_windows_reparse_point(&metadata) => Ok(true),
        Ok(_) => bail!("import rollback path is unsafe"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => bail!("unable to inspect import rollback path"),
    }
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "redox"
))]
fn rename_noreplace(
    source: &Dir,
    source_name: &str,
    destination: &Dir,
    destination_name: &str,
) -> io::Result<()> {
    Ok(rustix::fs::renameat_with(
        source,
        source_name,
        destination,
        destination_name,
        rustix::fs::RenameFlags::NOREPLACE,
    )?)
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "redox"
)))]
fn rename_noreplace(
    _source: &Dir,
    _source_name: &str,
    _destination: &Dir,
    _destination_name: &str,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace rename is unsupported",
    ))
}

pub(crate) fn run_import(
    familiar: &str,
    source: MemoryImportSourceKind,
    openclaw_root: Option<&Path>,
    apply: bool,
    json: bool,
) -> Result<()> {
    let coven_home = crate::paths::coven_home_dir()?;
    let discovered = discover_sources(
        &coven_home,
        DiscoverSourcesRequest {
            familiar,
            source,
            openclaw_root,
        },
    )?;
    let preview = build_import_plan(&coven_home, familiar, source, &discovered)?;
    let plan = if apply {
        apply_import_plan(&coven_home, &preview, &discovered)?
    } else {
        preview
    };

    if json {
        println!("{}", serde_json::to_string(&plan)?);
    } else {
        let mode = match plan.status {
            ImportPlanStatus::Preview => "Preview",
            ImportPlanStatus::Conflict => "Conflict",
            ImportPlanStatus::Verified => "Verified import",
            ImportPlanStatus::Restored => "Restored import",
            ImportPlanStatus::RolledBack => "Rolled-back import",
            ImportPlanStatus::ManualRecovery => "Import requiring manual recovery",
        };
        println!(
            "{mode} for familiar `{familiar}`: {} file(s), {} create, {} unchanged, {} conflict.",
            plan.file_count, plan.create_count, plan.unchanged_count, plan.conflict_count
        );
        println!("Bundle: {}", plan.bundle_id);
        for entry in &plan.entries {
            println!(
                "- {} -> {} [{}]",
                entry.logical_label,
                entry.target_name,
                match entry.status {
                    PlanEntryStatus::Create => "create",
                    PlanEntryStatus::Unchanged => "unchanged",
                    PlanEntryStatus::Restored => "restored",
                    PlanEntryStatus::Conflict => "conflict",
                }
            );
        }
        if plan.status == ImportPlanStatus::Verified {
            println!("Import verified; source files were left unchanged.");
        } else if plan.apply_eligible {
            println!("Plan is apply-eligible; no files or directories were created.");
        } else {
            println!("Plan is not apply-eligible; no files or directories were created.");
        }
    }
    Ok(())
}

pub(crate) fn run_restore(familiar: &str, bundle: &str, json: bool) -> Result<()> {
    let coven_home = crate::paths::coven_home_dir()?;
    let report = restore_import_bundle(&coven_home, familiar, bundle)?;
    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        println!(
            "Logical restore for familiar `{familiar}`: {} suppressed, {} unchanged, {} retained.",
            report.restored_count, report.unchanged_count, report.conflict_count
        );
        println!("Bundle: {}", report.bundle_id);
        for entry in &report.entries {
            println!(
                "- {} -> {} [{}]",
                entry.logical_label,
                entry.target_name,
                match entry.status {
                    PlanEntryStatus::Create => "create",
                    PlanEntryStatus::Unchanged => "unchanged",
                    PlanEntryStatus::Restored => "suppressed",
                    PlanEntryStatus::Conflict => "retained",
                }
            );
        }
        if report.status == ImportPlanStatus::Restored {
            println!(
                "Imported bytes remain private and recoverable but are hidden from Coven readers."
            );
        }
    }
    if report.status == ImportPlanStatus::ManualRecovery {
        bail!("memory restore requires manual recovery");
    }
    Ok(())
}

#[cfg(windows)]
fn restore_import_bundle(
    _coven_home: &Path,
    _familiar: &str,
    _bundle_id: &str,
) -> Result<ImportPlan> {
    bail!(
        "memory import restore is unavailable on Windows because durable directory mutation cannot be guaranteed"
    )
}

#[cfg(not(windows))]
fn restore_import_bundle(coven_home: &Path, familiar: &str, bundle_id: &str) -> Result<ImportPlan> {
    restore_import_bundle_with_hook(coven_home, familiar, bundle_id, &mut NoopRestoreHook)
}

#[cfg(all(test, not(windows)))]
pub(crate) fn restore_import_bundle_for_test(
    coven_home: &Path,
    familiar: &str,
    bundle_id: &str,
) -> Result<ImportPlan> {
    restore_import_bundle(coven_home, familiar, bundle_id)
}

#[cfg(not(windows))]
fn restore_import_bundle_with_hook(
    coven_home: &Path,
    familiar: &str,
    bundle_id: &str,
    hook: &mut dyn RestoreHook,
) -> Result<ImportPlan> {
    validate_familiar_component(familiar)?;
    validate_registered_familiar(coven_home, familiar)?;
    let bundle_name = bundle_component(bundle_id)?;
    let migration = open_locked_migration_familiar(coven_home, familiar)?;
    let bundle = open_existing_bundle(&migration, &bundle_name)?;
    let manifest = read_existing_manifest(&bundle)?;
    validate_stored_manifest(&manifest, familiar, &bundle_name)?;
    let mut journal = open_or_create_journal(&bundle)?;
    validate_journal_against_manifest(&journal, &manifest)?;
    if !matches!(
        journal.bundle_state,
        Some(
            JournalState::Verified
                | JournalState::Restoring
                | JournalState::Restored
                | JournalState::Invalidated
        )
    ) {
        bail!("memory import bundle is not verified for restore");
    }
    if journal.bundle_state == Some(JournalState::Verified) {
        append_journal(
            &bundle,
            &mut journal,
            JournalScope::Bundle,
            JournalState::Restoring,
            None,
            None,
        )?;
    }

    let target = open_target_context(coven_home, familiar, bundle_id)?;
    let restore_already_invalidated = journal.bundle_state == Some(JournalState::Invalidated);
    let mut restored_count = 0_usize;
    let mut unchanged_count = 0_usize;
    let mut conflict_count = 0_usize;
    let mut restore_evidence_invalidated = false;
    let mut statuses = HashMap::new();

    for entry in &manifest.entries {
        let latest = journal.entry_states.get(&entry.target_name).cloned();
        if let Some(record) = latest
            .as_ref()
            .filter(|record| record.state == JournalState::Restored)
        {
            match record.outcome {
                Some(JournalOutcome::Suppressed) => {
                    if !restore_already_invalidated
                        && verify_retained_restore_evidence(&target, familiar, bundle_id, entry)
                    {
                        restored_count += 1;
                        statuses.insert(entry.target_name.as_str(), PlanEntryStatus::Restored);
                    } else {
                        restore_evidence_invalidated = true;
                        conflict_count += 1;
                        statuses.insert(entry.target_name.as_str(), PlanEntryStatus::Conflict);
                    }
                }
                Some(JournalOutcome::Unchanged) => {
                    unchanged_count += 1;
                    statuses.insert(entry.target_name.as_str(), PlanEntryStatus::Unchanged);
                }
                Some(JournalOutcome::ManualRecovery) => {
                    conflict_count += 1;
                    statuses.insert(entry.target_name.as_str(), PlanEntryStatus::Conflict);
                }
                _ => bail!("restored import journal has an invalid outcome"),
            }
            continue;
        }

        let verified_outcome = verified_outcome_for_entry(&journal, entry)?;

        if verified_outcome == JournalOutcome::Unchanged {
            append_restore_entry(
                &bundle,
                &mut journal,
                entry,
                JournalOutcome::Unchanged,
                JournalOutcome::Unchanged,
            )?;
            unchanged_count += 1;
            statuses.insert(entry.target_name.as_str(), PlanEntryStatus::Unchanged);
            continue;
        }
        if verified_outcome != JournalOutcome::Created {
            bail!("verified import journal has an invalid restore outcome");
        }

        let restoring_created = latest.as_ref().is_some_and(|record| {
            record.state == JournalState::Restoring
                && record.outcome == Some(JournalOutcome::Created)
        });
        match classify_restore_target(&target, entry)? {
            RestoreTargetStatus::Exact(mut trusted) => {
                if !restoring_created {
                    append_journal(
                        &bundle,
                        &mut journal,
                        JournalScope::Entry,
                        JournalState::Restoring,
                        Some(entry),
                        Some(JournalOutcome::Created),
                    )?;
                }
                match mark_exact_restore_target(
                    &target,
                    familiar,
                    bundle_id,
                    entry,
                    &mut trusted,
                    hook,
                )? {
                    JournalOutcome::Suppressed => {
                        append_journal(
                            &bundle,
                            &mut journal,
                            JournalScope::Entry,
                            JournalState::Restored,
                            Some(entry),
                            Some(JournalOutcome::Suppressed),
                        )?;
                        restored_count += 1;
                        statuses.insert(entry.target_name.as_str(), PlanEntryStatus::Restored);
                    }
                    JournalOutcome::ManualRecovery => {
                        append_journal(
                            &bundle,
                            &mut journal,
                            JournalScope::Entry,
                            JournalState::Restored,
                            Some(entry),
                            Some(JournalOutcome::ManualRecovery),
                        )?;
                        conflict_count += 1;
                        statuses.insert(entry.target_name.as_str(), PlanEntryStatus::Conflict);
                    }
                    _ => unreachable!("restore removal has only terminal outcomes"),
                }
            }
            RestoreTargetStatus::Missing | RestoreTargetStatus::Conflict => {
                append_restore_entry(
                    &bundle,
                    &mut journal,
                    entry,
                    JournalOutcome::ManualRecovery,
                    JournalOutcome::ManualRecovery,
                )?;
                conflict_count += 1;
                statuses.insert(entry.target_name.as_str(), PlanEntryStatus::Conflict);
            }
        }
    }

    if journal.bundle_state == Some(JournalState::Restored) && restore_evidence_invalidated {
        append_journal(
            &bundle,
            &mut journal,
            JournalScope::Bundle,
            JournalState::Invalidated,
            None,
            Some(JournalOutcome::ManualRecovery),
        )?;
    } else if journal.bundle_state != Some(JournalState::Restored)
        && journal.bundle_state != Some(JournalState::Invalidated)
    {
        append_journal(
            &bundle,
            &mut journal,
            JournalScope::Bundle,
            JournalState::Restored,
            None,
            if conflict_count > 0 {
                Some(JournalOutcome::ManualRecovery)
            } else {
                None
            },
        )?;
    }

    let mut entries = manifest
        .entries
        .iter()
        .map(|entry| PlanEntry {
            logical_label: entry.source_label.clone(),
            target_name: entry.target_name.clone(),
            digest: entry.digest.clone(),
            status: statuses
                .get(entry.target_name.as_str())
                .copied()
                .unwrap_or(PlanEntryStatus::Conflict),
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.logical_label.cmp(&right.logical_label));
    Ok(ImportPlan {
        familiar_id: familiar.to_owned(),
        source_kind: manifest.source_kind,
        bundle_id: bundle_id.to_owned(),
        status: if conflict_count > 0 {
            ImportPlanStatus::ManualRecovery
        } else {
            ImportPlanStatus::Restored
        },
        apply_eligible: false,
        file_count: entries.len(),
        create_count: 0,
        unchanged_count,
        restored_count,
        conflict_count,
        entries,
    })
}

fn append_restore_entry(
    bundle: &Bundle,
    journal: &mut JournalSummary,
    entry: &ManifestEntry,
    restoring_outcome: JournalOutcome,
    restored_outcome: JournalOutcome,
) -> Result<()> {
    if journal
        .entry_states
        .get(&entry.target_name)
        .is_none_or(|record| record.state != JournalState::Restoring)
    {
        append_journal(
            bundle,
            journal,
            JournalScope::Entry,
            JournalState::Restoring,
            Some(entry),
            Some(restoring_outcome),
        )?;
    }
    append_journal(
        bundle,
        journal,
        JournalScope::Entry,
        JournalState::Restored,
        Some(entry),
        Some(restored_outcome),
    )
}

enum RestoreTargetStatus {
    Exact(cap_std::fs::File),
    Missing,
    Conflict,
}

fn classify_restore_target(
    target: &TargetContext,
    entry: &ManifestEntry,
) -> Result<RestoreTargetStatus> {
    match target.dir.symlink_metadata(&entry.target_name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(RestoreTargetStatus::Missing),
        Err(_) => Ok(RestoreTargetStatus::Conflict),
        Ok(metadata)
            if metadata.is_file()
                && !metadata_is_windows_reparse_point(&metadata)
                && metadata.len() == entry.byte_length =>
        {
            let mut options = OpenOptions::new();
            options
                .read(true)
                .follow(FollowSymlinks::No)
                .maybe_dir(true);
            let mut file = match target.dir.open_with(&entry.target_name, &options) {
                Ok(file) => file,
                Err(_) => return Ok(RestoreTargetStatus::Conflict),
            };
            let digest = digest_stable_opened_file(&mut file);
            if digest.as_deref() != Some(entry.digest.as_str())
                || !verify_optional_private_entry(&target.work, &candidate_name(entry), entry)?
                || !same_file_identity_with_handle(&target.work, &candidate_name(entry), &file)?
            {
                return Ok(RestoreTargetStatus::Conflict);
            }
            Ok(RestoreTargetStatus::Exact(file))
        }
        Ok(_) => Ok(RestoreTargetStatus::Conflict),
    }
}

fn verify_retained_restore_evidence(
    target: &TargetContext,
    familiar: &str,
    bundle_id: &str,
    entry: &ManifestEntry,
) -> bool {
    let candidate = candidate_name(entry);
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    let canonical = match target.dir.open_with(&entry.target_name, &options) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut canonical = match verify_open_private_entry_file(canonical, entry) {
        Ok(file) => file,
        Err(_) => return false,
    };
    if verify_entry_file(&target.work, &candidate, entry).is_err()
        || !matches!(
            same_file_identity_with_handle(&target.work, &candidate, &canonical),
            Ok(true)
        )
        || !opened_file_has_restore_marker(&mut canonical, familiar, bundle_id, entry)
    {
        return false;
    }
    true
}

#[cfg(unix)]
fn read_restored_marker(file: &cap_std::fs::File) -> Option<RestoredMarker> {
    let mut value = vec![0_u8; 1024];
    let length = rustix::fs::fgetxattr(file, RESTORED_XATTR, &mut value).ok()?;
    value.truncate(length);
    serde_json::from_slice(&value).ok()
}

#[cfg(not(unix))]
fn read_restored_marker(_file: &cap_std::fs::File) -> Option<RestoredMarker> {
    None
}

fn opened_file_matches_restored_marker(
    file: &mut cap_std::fs::File,
    marker: &RestoredMarker,
) -> bool {
    if marker.protocol_version != IMPORT_PROTOCOL_VERSION {
        return false;
    }
    if file.seek(SeekFrom::Start(0)).is_err() {
        return false;
    }
    digest_stable_opened_file(file).as_deref() == Some(marker.digest.as_str())
}

fn opened_file_has_restore_marker(
    file: &mut cap_std::fs::File,
    familiar: &str,
    bundle_id: &str,
    entry: &ManifestEntry,
) -> bool {
    let Some(marker) = read_restored_marker(file) else {
        return false;
    };
    marker.familiar_id == familiar
        && marker.bundle_id == bundle_id
        && marker.target_name == entry.target_name
        && marker.digest == entry.digest
        && opened_file_matches_restored_marker(file, &marker)
}

pub(crate) fn opened_file_is_logically_restored(
    coven_home: &Path,
    familiar: &str,
    target_name: &str,
    file: &mut cap_std::fs::File,
) -> bool {
    let Some(marker) = read_restored_marker(file) else {
        return false;
    };
    if marker.protocol_version != IMPORT_PROTOCOL_VERSION
        || marker.familiar_id != familiar
        || marker.target_name != target_name
        || validate_familiar_component(familiar).is_err()
        || validate_target_name(target_name).is_err()
        || bundle_component(&marker.bundle_id).is_err()
        || !opened_file_matches_restored_marker(file, &marker)
    {
        return false;
    }

    let migration = match open_existing_migration_familiar(coven_home, familiar) {
        Ok(migration) => migration,
        Err(_) => return false,
    };
    let bundle = match open_existing_bundle_in(&migration, &marker.bundle_id) {
        Ok(bundle) => bundle,
        Err(_) => return false,
    };
    let manifest = match read_existing_manifest(&bundle) {
        Ok(manifest) => manifest,
        Err(_) => return false,
    };
    if validate_stored_manifest(&manifest, familiar, &marker.bundle_id).is_err() {
        return false;
    }
    let Some(entry) = manifest
        .entries
        .iter()
        .find(|entry| entry.target_name == target_name && entry.digest == marker.digest)
    else {
        return false;
    };
    let journal = match read_existing_journal(&bundle) {
        Ok(journal) => journal,
        Err(_) => return false,
    };
    if validate_journal_against_manifest(&journal, &manifest).is_err()
        || journal.bundle_state != Some(JournalState::Restored)
        || !journal.entry_states.get(target_name).is_some_and(|record| {
            record.state == JournalState::Restored
                && record.outcome == Some(JournalOutcome::Suppressed)
        })
    {
        return false;
    }
    let target = match open_existing_target_context(coven_home, familiar, &marker.bundle_id) {
        Ok(target) => target,
        Err(_) => return false,
    };
    verify_entry_file(&target.work, &candidate_name(entry), entry).is_ok()
        && matches!(
            same_file_identity_with_handle(&target.work, &candidate_name(entry), file),
            Ok(true)
        )
}

#[cfg(unix)]
fn mark_opened_file_restored(
    file: &mut cap_std::fs::File,
    familiar: &str,
    bundle_id: &str,
    entry: &ManifestEntry,
) -> Result<bool> {
    let marker = RestoredMarker {
        protocol_version: IMPORT_PROTOCOL_VERSION,
        familiar_id: familiar.to_owned(),
        bundle_id: bundle_id.to_owned(),
        target_name: entry.target_name.clone(),
        digest: entry.digest.clone(),
    };
    let marker_bytes = serde_json::to_vec(&marker)?;
    match rustix::fs::fsetxattr(
        &*file,
        RESTORED_XATTR,
        &marker_bytes,
        rustix::fs::XattrFlags::CREATE,
    ) {
        Ok(()) => {}
        Err(_) if read_restored_marker(file).as_ref() == Some(&marker) => {}
        Err(_) => return Ok(false),
    }
    file.sync_all()
        .map_err(|_| anyhow!("unable to sync logical restore marker"))?;
    Ok(opened_file_has_restore_marker(
        file, familiar, bundle_id, entry,
    ))
}

#[cfg(not(unix))]
fn mark_opened_file_restored(
    _file: &mut cap_std::fs::File,
    _familiar: &str,
    _bundle_id: &str,
    _entry: &ManifestEntry,
) -> Result<bool> {
    Ok(false)
}

fn mark_exact_restore_target(
    target: &TargetContext,
    familiar: &str,
    bundle_id: &str,
    entry: &ManifestEntry,
    trusted: &mut cap_std::fs::File,
    hook: &mut dyn RestoreHook,
) -> Result<JournalOutcome> {
    let candidate = candidate_name(entry);
    if !matches!(
        same_file_identity_with_handle(&target.dir, &entry.target_name, trusted),
        Ok(true)
    ) || !matches!(
        same_file_identity_with_handle(&target.work, &candidate, trusted),
        Ok(true)
    ) {
        return Ok(JournalOutcome::ManualRecovery);
    }
    run_restore_step(hook, RestoreStep::BeforeMarker(entry.target_name.clone()))?;
    if !mark_opened_file_restored(trusted, familiar, bundle_id, entry)? {
        return Ok(JournalOutcome::ManualRecovery);
    }
    run_restore_step(
        hook,
        RestoreStep::AfterMarkerBeforeVerify(entry.target_name.clone()),
    )?;
    let owned = matches!(
        same_file_identity_with_handle(&target.dir, &entry.target_name, trusted),
        Ok(true)
    ) && matches!(
        same_file_identity_with_handle(&target.work, &candidate, trusted),
        Ok(true)
    ) && verify_entry_file(&target.work, &candidate, entry).is_ok()
        && opened_file_has_restore_marker(trusted, familiar, bundle_id, entry);
    if !owned {
        return Ok(JournalOutcome::ManualRecovery);
    }
    run_restore_step(
        hook,
        RestoreStep::AfterVerifyBeforeJournal(entry.target_name.clone()),
    )?;
    if !matches!(
        same_file_identity_with_handle(&target.dir, &entry.target_name, trusted),
        Ok(true)
    ) || !matches!(
        same_file_identity_with_handle(&target.work, &candidate, trusted),
        Ok(true)
    ) || verify_entry_file(&target.work, &candidate, entry).is_err()
        || !opened_file_has_restore_marker(trusted, familiar, bundle_id, entry)
    {
        return Ok(JournalOutcome::ManualRecovery);
    }
    Ok(JournalOutcome::Suppressed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    const EXPECTED_MAX_SOURCE_FILES: usize = 256;
    const EXPECTED_MAX_AGGREGATE_BYTES: u64 = 16 * 1024 * 1024;
    const EXPECTED_MAX_TRAVERSAL_DEPTH: usize = 32;
    const EXPECTED_MAX_VISITED_ENTRIES: usize = 1024;
    const EXPECTED_MAX_VISITED_DIRECTORIES: usize = 256;

    #[test]
    fn import_plan_json_is_stable_and_redacted() {
        let report = ImportPlan {
            familiar_id: "sage".to_owned(),
            source_kind: MemoryImportSourceKind::Openclaw,
            bundle_id: "blake3:bundle-1".to_owned(),
            status: ImportPlanStatus::Preview,
            apply_eligible: true,
            file_count: 1,
            create_count: 1,
            unchanged_count: 0,
            restored_count: 0,
            conflict_count: 0,
            entries: vec![PlanEntry {
                logical_label: "memory/notes.md".to_owned(),
                target_name: "openclaw-notes.md".to_owned(),
                digest: "blake3:abc123".to_owned(),
                status: PlanEntryStatus::Create,
            }],
        };

        let value = serde_json::to_value(&report).expect("report must serialize");
        assert_exact_object_keys(
            &value,
            &[
                "familiar_id",
                "source_kind",
                "bundle_id",
                "status",
                "file_count",
                "created_count",
                "unchanged_count",
                "restored_count",
                "conflict_count",
                "entries",
            ],
        );
        assert_exact_object_keys(
            &value["entries"][0],
            &["source_label", "target_name", "digest", "status"],
        );
        assert_eq!(value["familiar_id"], "sage");
        assert_eq!(value["source_kind"], "openclaw");
        assert_eq!(value["status"], "preview");
        assert_eq!(value["created_count"], 1);
        assert_eq!(value["restored_count"], 0);
        assert_eq!(value["entries"][0]["status"], "create");
        assert_eq!(value["entries"][0]["source_label"], "memory/notes.md");

        let json = serde_json::to_string(&report).expect("report must serialize");
        for forbidden in ["content", "source_path", "absolute_path", "bytes"] {
            assert!(
                !json.contains(forbidden),
                "serialized report leaked forbidden value {forbidden:?}: {json}"
            );
        }
    }

    fn assert_exact_object_keys(value: &serde_json::Value, expected: &[&str]) {
        let object = value
            .as_object()
            .expect("value must serialize as an object");
        let mut actual = object.keys().map(String::as_str).collect::<Vec<_>>();
        actual.sort_unstable();
        let mut expected = expected.to_vec();
        expected.sort_unstable();
        assert_eq!(actual, expected);
    }

    #[test]
    fn empty_import_plan_has_a_bound_bundle_without_path_fields() {
        let report = ImportPlan {
            familiar_id: "sage".to_owned(),
            source_kind: MemoryImportSourceKind::Native,
            bundle_id: "blake3:empty".to_owned(),
            status: ImportPlanStatus::Preview,
            apply_eligible: true,
            file_count: 0,
            create_count: 0,
            unchanged_count: 0,
            restored_count: 0,
            conflict_count: 0,
            entries: Vec::new(),
        };

        let value = serde_json::to_value(report).expect("report must serialize");
        let object = value
            .as_object()
            .expect("report must serialize as an object");
        assert_eq!(object["bundle_id"], "blake3:empty");
        assert!(!object.contains_key("content"));
        assert!(!object.contains_key("source_path"));
        assert!(!object.contains_key("openclaw_root"));
        assert_no_absolute_path_values(&value);
    }

    fn assert_no_absolute_path_values(value: &serde_json::Value) {
        match value {
            serde_json::Value::String(value) => {
                assert!(
                    !value.starts_with('/'),
                    "serialized report contains an absolute path: {value}"
                );
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    assert_no_absolute_path_values(value);
                }
            }
            serde_json::Value::Object(values) => {
                for value in values.values() {
                    assert_no_absolute_path_values(value);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn discovers_native_allowlist_for_exact_registered_familiar_in_stable_order() -> Result<()> {
        let temp = trusted_tempdir()?;
        let sage = temp.path().join("sage-workspace");
        let cody = temp.path().join("cody-workspace");
        write_registered_familiars(temp.path(), &[("sage", &sage), ("cody", &cody)])?;

        write_file(&sage.join("notes/z-last.md"), b"z")?;
        write_file(&sage.join("MEMORY.md"), b"root")?;
        write_file(&sage.join("memory/nested/a-first.md"), b"a")?;
        write_file(&cody.join("MEMORY.md"), b"wrong familiar")?;
        add_excluded_traps(&sage, true)?;

        let first = discover_sources(
            temp.path(),
            DiscoverSourcesRequest {
                familiar: "sage",
                source: MemoryImportSourceKind::Native,
                openclaw_root: None,
            },
        )?;
        let second = discover_sources(
            temp.path(),
            DiscoverSourcesRequest {
                familiar: "sage",
                source: MemoryImportSourceKind::Native,
                openclaw_root: None,
            },
        )?;

        assert_eq!(
            labels(&first),
            vec!["MEMORY.md", "memory/nested/a-first.md", "notes/z-last.md"]
        );
        assert_eq!(labels(&second), labels(&first));
        assert_eq!(
            first
                .iter()
                .map(|source| source.bytes.as_slice())
                .collect::<Vec<_>>(),
            vec![b"root".as_slice(), b"a".as_slice(), b"z".as_slice()]
        );
        assert!(first
            .iter()
            .all(|source| !Path::new(&source.source_label).is_absolute()));

        let debug = format!("{first:?}");
        assert!(!debug.contains("root"));
        assert!(!debug.contains(&sage.to_string_lossy().into_owned()));
        assert!(debug.contains("byte_len"));
        Ok(())
    }

    #[test]
    fn discovers_openclaw_allowlist_into_explicit_registered_familiar() -> Result<()> {
        let temp = trusted_tempdir()?;
        let sage = temp.path().join("native-sage");
        let openclaw = temp.path().join("explicit-openclaw");
        write_registered_familiars(temp.path(), &[("sage", &sage)])?;

        write_file(&openclaw.join("memory/z.md"), b"z")?;
        write_file(&openclaw.join("DREAMS.md"), b"dreams")?;
        write_file(&openclaw.join("MEMORY.md"), b"memory")?;
        write_file(&openclaw.join("memory/nested/a.md"), b"a")?;
        add_excluded_traps(&openclaw, false)?;

        let discovered = discover_sources(
            temp.path(),
            DiscoverSourcesRequest {
                familiar: "sage",
                source: MemoryImportSourceKind::Openclaw,
                openclaw_root: Some(&openclaw),
            },
        )?;

        assert_eq!(
            labels(&discovered),
            vec![
                "DREAMS.md",
                "MEMORY.md",
                "memory/nested/a.md",
                "memory/z.md"
            ]
        );
        assert!(
            discovered
                .iter()
                .all(|source| source.source_label != "notes/native.md"),
            "OpenClaw must never discover a notes tree"
        );
        Ok(())
    }

    #[test]
    fn discovers_unknown_familiar_before_touching_source_root() -> Result<()> {
        let temp = trusted_tempdir()?;
        let sage = temp.path().join("sage");
        write_registered_familiars(temp.path(), &[("sage", &sage)])?;
        let must_not_touch = temp.path().join("missing-source-root");

        let error = discover_sources(
            temp.path(),
            DiscoverSourcesRequest {
                familiar: "unknown",
                source: MemoryImportSourceKind::Openclaw,
                openclaw_root: Some(&must_not_touch),
            },
        )
        .expect_err("an unregistered familiar must fail");

        let message = format!("{error:#}");
        assert!(message.contains("unknown familiar `unknown`"), "{message}");
        assert!(!message.contains("source root"), "{message}");
        assert!(!message.contains(&must_not_touch.to_string_lossy().into_owned()));
        Ok(())
    }

    #[test]
    fn discovers_registry_errors_without_revealing_absolute_paths() -> Result<()> {
        let temp = trusted_tempdir()?;
        fs::write(temp.path().join("familiars.toml"), "not valid toml = [")?;

        let error = discover_sources(
            temp.path(),
            DiscoverSourcesRequest {
                familiar: "sage",
                source: MemoryImportSourceKind::Native,
                openclaw_root: None,
            },
        )
        .expect_err("an invalid registry must fail");

        let message = format!("{error:#}");
        assert!(message.contains("familiar registry"), "{message}");
        assert!(
            !message.contains(&temp.path().to_string_lossy().into_owned()),
            "{message}"
        );
        Ok(())
    }

    #[test]
    fn discovers_openclaw_requires_a_real_explicit_root() -> Result<()> {
        let temp = trusted_tempdir()?;
        let sage = temp.path().join("sage");
        write_registered_familiars(temp.path(), &[("sage", &sage)])?;

        let missing = discover_sources(
            temp.path(),
            DiscoverSourcesRequest {
                familiar: "sage",
                source: MemoryImportSourceKind::Openclaw,
                openclaw_root: None,
            },
        )
        .expect_err("OpenClaw discovery must require a root");
        assert!(
            missing.to_string().contains("explicit OpenClaw root"),
            "{missing:#}"
        );

        let file_root = temp.path().join("not-a-directory");
        fs::write(&file_root, "sentinel")?;
        let wrong_type = discover_sources(
            temp.path(),
            DiscoverSourcesRequest {
                familiar: "sage",
                source: MemoryImportSourceKind::Openclaw,
                openclaw_root: Some(&file_root),
            },
        )
        .expect_err("an OpenClaw file root must fail closed");
        assert!(
            wrong_type.to_string().contains("real directory"),
            "{wrong_type:#}"
        );
        assert!(
            !format!("{wrong_type:#}").contains(&file_root.to_string_lossy().into_owned()),
            "errors must not reveal absolute roots"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn discovers_without_traversing_source_or_allowed_directory_symlinks() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = trusted_tempdir()?;
        let sage = temp.path().join("sage");
        let openclaw = temp.path().join("openclaw");
        let outside = temp.path().join("outside");
        write_registered_familiars(temp.path(), &[("sage", &sage)])?;
        write_file(&outside.join("secret.md"), b"sentinel outside content")?;
        write_file(&openclaw.join("MEMORY.md"), b"root")?;
        fs::create_dir_all(openclaw.join("memory"))?;
        symlink(
            outside.join("secret.md"),
            openclaw.join("memory/file-link.md"),
        )?;
        symlink(&outside, openclaw.join("memory/directory-link"))?;

        let discovered = discover_sources(
            temp.path(),
            DiscoverSourcesRequest {
                familiar: "sage",
                source: MemoryImportSourceKind::Openclaw,
                openclaw_root: Some(&openclaw),
            },
        )?;
        assert_eq!(labels(&discovered), vec!["MEMORY.md"]);

        let linked_root = temp.path().join("linked-openclaw");
        symlink(&openclaw, &linked_root)?;
        let error = discover_sources(
            temp.path(),
            DiscoverSourcesRequest {
                familiar: "sage",
                source: MemoryImportSourceKind::Openclaw,
                openclaw_root: Some(&linked_root),
            },
        )
        .expect_err("a symlinked source root must fail closed");
        assert!(error.to_string().contains("real directory"), "{error:#}");

        let alternate = temp.path().join("alternate-openclaw");
        write_file(&alternate.join("MEMORY.md"), b"root only")?;
        symlink(&outside, alternate.join("memory"))?;
        let discovered = discover_sources(
            temp.path(),
            DiscoverSourcesRequest {
                familiar: "sage",
                source: MemoryImportSourceKind::Openclaw,
                openclaw_root: Some(&alternate),
            },
        )?;
        assert_eq!(labels(&discovered), vec!["MEMORY.md"]);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn discovers_excludes_non_utf8_names_before_path_inspection() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        assert_eq!(
            visible_utf8_name(OsString::from_vec(b"non-utf8-\xff.md".to_vec())),
            None
        );
        assert_eq!(visible_utf8_name(OsString::from("escape\\name.md")), None);
        assert_eq!(visible_utf8_name(OsString::from("C:drive.md")), None);
        assert_eq!(visible_utf8_name(OsString::from("line\nbreak.md")), None);
    }

    #[test]
    fn windows_reparse_attribute_classification_is_platform_independent() {
        assert!(!windows_attributes_are_reparse_point(0));
        assert!(!windows_attributes_are_reparse_point(0x20));
        assert!(windows_attributes_are_reparse_point(0x400));
        assert!(windows_attributes_are_reparse_point(0x420));
    }

    #[test]
    fn source_root_rejects_an_empty_relative_path() {
        let error = SourceRoot::open(Path::new(""))
            .err()
            .expect("an empty source root must not resolve to the current directory");
        assert_eq!(error.to_string(), "source root is unavailable");
    }

    #[test]
    fn aggregate_limit_is_checked_before_read_bytes_are_retained() {
        let mut budget = DiscoveryBudget {
            source_files: 0,
            aggregate_bytes: MAX_AGGREGATE_SOURCE_BYTES - 1,
            ..DiscoveryBudget::default()
        };
        let mut reader = io::Cursor::new(b"ab");

        let error = read_source_bytes_with_budget(&mut reader, &mut budget, "overflow-secret.md")
            .expect_err("a chunk larger than the remaining aggregate budget must fail");

        assert_eq!(error.to_string(), SOURCE_BYTE_LIMIT_ERROR);
        assert!(!format!("{error:#}").contains("overflow-secret"));
        assert_eq!(budget.aggregate_bytes, MAX_AGGREGATE_SOURCE_BYTES - 1);
    }

    #[test]
    fn discovery_accepts_exact_file_limit_and_rejects_one_more_without_leaking() -> Result<()> {
        let temp = trusted_tempdir()?;
        let memory = temp.path().join("memory");
        for index in 0..EXPECTED_MAX_SOURCE_FILES {
            write_file(&memory.join(format!("{index:04}.md")), b"x")?;
        }
        let root = SourceRoot::open(temp.path())?;
        let exact = root.discover(&[], &["memory"])?;
        assert_eq!(exact.len(), EXPECTED_MAX_SOURCE_FILES);

        write_file(&memory.join("overflow-secret.md"), b"count overflow secret")?;
        let error = root
            .discover(&[], &["memory"])
            .expect_err("one source beyond the file limit must fail");
        assert_eq!(
            error.to_string(),
            "source discovery exceeds maximum file count"
        );
        assert!(!format!("{error:#}").contains("overflow-secret"));
        assert!(!format!("{error:#}").contains("count overflow secret"));
        assert!(!format!("{error:#}").contains(&temp.path().to_string_lossy().into_owned()));
        Ok(())
    }

    #[test]
    fn discovery_accepts_exact_entry_limit_and_rejects_one_more_in_mixed_zero_markdown_tree(
    ) -> Result<()> {
        let temp = trusted_tempdir()?;
        let memory = temp.path().join("memory");
        for index in 0..EXPECTED_MAX_VISITED_DIRECTORIES {
            fs::create_dir_all(memory.join(format!("dir-{index:04}")))?;
        }
        for index in EXPECTED_MAX_VISITED_DIRECTORIES..EXPECTED_MAX_VISITED_ENTRIES {
            write_file(
                &memory.join(format!("ignored-{index:04}.txt")),
                b"ignored secret",
            )?;
        }

        let root = SourceRoot::open(temp.path())?;
        let exact = root.discover(&[], &["memory"])?;
        assert!(exact.is_empty());

        write_file(
            &memory.join("overflow-secret.txt"),
            b"entry overflow secret",
        )?;
        let error = root
            .discover(&[], &["memory"])
            .expect_err("one directory entry beyond the traversal limit must fail");
        assert_eq!(
            error.to_string(),
            "source discovery exceeds maximum visited entry count"
        );
        assert!(!format!("{error:#}").contains("overflow-secret"));
        assert!(!format!("{error:#}").contains("entry overflow secret"));
        assert!(!format!("{error:#}").contains(&temp.path().to_string_lossy().into_owned()));
        Ok(())
    }

    #[test]
    fn discovery_accepts_exact_directory_limit_and_rejects_one_more_without_leaking() -> Result<()>
    {
        let temp = trusted_tempdir()?;
        let memory = temp.path().join("memory");
        for index in 0..EXPECTED_MAX_VISITED_DIRECTORIES {
            fs::create_dir_all(memory.join(format!("dir-{index:04}")))?;
        }

        let root = SourceRoot::open(temp.path())?;
        let exact = root.discover(&[], &["memory"])?;
        assert!(exact.is_empty());

        fs::create_dir_all(memory.join("overflow-secret-directory"))?;
        let error = root
            .discover(&[], &["memory"])
            .expect_err("one directory beyond the traversal limit must fail");
        assert_eq!(
            error.to_string(),
            "source discovery exceeds maximum directory count"
        );
        assert!(!format!("{error:#}").contains("overflow-secret"));
        assert!(!format!("{error:#}").contains(&temp.path().to_string_lossy().into_owned()));
        Ok(())
    }

    #[test]
    fn discovery_accepts_exact_byte_limit_and_rejects_one_more_without_leaking() -> Result<()> {
        let temp = trusted_tempdir()?;
        let memory = temp.path().join("memory");
        let mut remaining = EXPECTED_MAX_AGGREGATE_BYTES;
        let mut index = 0;
        while remaining > 0 {
            let chunk = remaining.min(crate::cockpit_sources::MEMORY_CONTENT_MAX_BYTES);
            write_file(
                &memory.join(format!("{index:04}.md")),
                &vec![b'x'; chunk as usize],
            )?;
            remaining -= chunk;
            index += 1;
        }
        let root = SourceRoot::open(temp.path())?;
        let exact = root.discover(&[], &["memory"])?;
        assert_eq!(
            exact
                .iter()
                .map(|source| source.bytes.len() as u64)
                .sum::<u64>(),
            EXPECTED_MAX_AGGREGATE_BYTES
        );

        write_file(&memory.join("overflow-secret.md"), b"b")?;
        let error = root
            .discover(&[], &["memory"])
            .expect_err("one byte beyond the aggregate limit must fail");
        assert_eq!(
            error.to_string(),
            "source discovery exceeds maximum aggregate bytes"
        );
        assert!(!format!("{error:#}").contains("overflow-secret"));
        assert!(!format!("{error:#}").contains(&temp.path().to_string_lossy().into_owned()));
        Ok(())
    }

    #[test]
    fn discovery_accepts_exact_depth_limit_and_rejects_one_more_without_leaking() -> Result<()> {
        let temp = trusted_tempdir()?;
        let mut deepest = temp.path().join("memory");
        for index in 0..EXPECTED_MAX_TRAVERSAL_DEPTH {
            deepest.push(format!("level-{index:02}"));
        }
        write_file(&deepest.join("exact.md"), b"exact")?;
        let root = SourceRoot::open(temp.path())?;
        let exact = root.discover(&[], &["memory"])?;
        assert_eq!(
            labels(&exact),
            vec![format!(
                "memory/{}/exact.md",
                (0..EXPECTED_MAX_TRAVERSAL_DEPTH)
                    .map(|index| format!("level-{index:02}"))
                    .collect::<Vec<_>>()
                    .join("/")
            )]
        );

        let overflow = deepest.join("overflow-secret");
        write_file(&overflow.join("hidden.md"), b"depth overflow secret")?;
        let error = root
            .discover(&[], &["memory"])
            .expect_err("one directory beyond the depth limit must fail");
        assert_eq!(
            error.to_string(),
            "source discovery exceeds maximum traversal depth"
        );
        assert!(!format!("{error:#}").contains("overflow-secret"));
        assert!(!format!("{error:#}").contains("depth overflow secret"));
        assert!(!format!("{error:#}").contains(&temp.path().to_string_lossy().into_owned()));
        Ok(())
    }

    #[test]
    fn plans_use_canonical_flat_targets_and_exact_blake3_digests_without_mutation() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        write_file(&temp.path().join("memory.md"), b"wrong location")?;
        write_file(
            &temp.path().join("memory/other/memory-notes.md"),
            b"wrong familiar",
        )?;
        let sources = vec![
            DiscoveredSource {
                source_label: "memory/notes.md".to_owned(),
                bytes: b"nested".to_vec(),
            },
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"abc".to_vec(),
            },
        ];

        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;

        assert_eq!(plan.status, ImportPlanStatus::Preview);
        assert!(plan.apply_eligible);
        assert_eq!(plan.create_count, 2);
        assert_eq!(plan.unchanged_count, 0);
        assert_eq!(plan.conflict_count, 0);
        assert_eq!(
            plan.entries
                .iter()
                .map(|entry| (
                    entry.logical_label.as_str(),
                    entry.target_name.as_str(),
                    entry.status
                ))
                .collect::<Vec<_>>(),
            vec![
                ("MEMORY.md", "memory.md", PlanEntryStatus::Create),
                (
                    "memory/notes.md",
                    "memory-notes.md",
                    PlanEntryStatus::Create
                )
            ]
        );
        assert_eq!(
            plan.entries[0].digest,
            "blake3:6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
        assert!(!temp.path().join("memory/sage").exists());
        assert!(!temp.path().join("memory-import").exists());
        assert!(!temp.path().join("journal").exists());
        Ok(())
    }

    #[test]
    fn plans_flatten_collisions_and_reserved_names_with_stable_digest_suffixes() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "memory/a-b.md".to_owned(),
                bytes: b"one".to_vec(),
            },
            DiscoveredSource {
                source_label: "memory/a/b.md".to_owned(),
                bytes: b"two".to_vec(),
            },
            DiscoveredSource {
                source_label: "CON.md".to_owned(),
                bytes: b"three".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/Caf\u{e9}.md".to_owned(),
                bytes: b"four".to_vec(),
            },
        ];

        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let names = plan
            .entries
            .iter()
            .map(|entry| entry.target_name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names.len(), 4);
        assert!(names[0].starts_with("con-") && names[0].ends_with(".md"));
        assert!(names[1].starts_with("memory-a-b-") && names[1].ends_with(".md"));
        assert!(names[2].starts_with("memory-a-b-") && names[2].ends_with(".md"));
        assert_ne!(names[1], names[2]);
        assert_eq!(names[3], "notes-cafe.md");
        for name in names {
            assert_portable_target_name(name);
        }
        Ok(())
    }

    #[test]
    fn plans_are_independent_of_input_and_filesystem_enumeration_order() -> Result<()> {
        let first = trusted_tempdir()?;
        let second = trusted_tempdir()?;
        for home in [first.path(), second.path()] {
            let workspace = home.join("workspace");
            write_registered_familiars(home, &[("sage", &workspace)])?;
            fs::create_dir_all(home.join("memory/sage"))?;
        }
        write_file(&first.path().join("memory/sage/z.md"), b"unrelated")?;
        write_file(&first.path().join("memory/sage/memory.md"), b"root")?;
        write_file(&second.path().join("memory/sage/memory.md"), b"root")?;
        write_file(&second.path().join("memory/sage/z.md"), b"unrelated")?;

        let sources = vec![
            DiscoveredSource {
                source_label: "notes/z.md".to_owned(),
                bytes: b"z".to_vec(),
            },
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"root".to_vec(),
            },
            DiscoveredSource {
                source_label: "memory/a.md".to_owned(),
                bytes: b"a".to_vec(),
            },
        ];
        let reversed = sources.iter().cloned().rev().collect::<Vec<_>>();

        let first_plan = build_import_plan(
            first.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let second_plan = build_import_plan(
            second.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &reversed,
        )?;

        assert_eq!(first_plan, second_plan);
        assert_eq!(
            first_plan
                .entries
                .iter()
                .map(|entry| entry.status)
                .collect::<Vec<_>>(),
            vec![
                PlanEntryStatus::Unchanged,
                PlanEntryStatus::Create,
                PlanEntryStatus::Create
            ]
        );
        Ok(())
    }

    #[test]
    fn plans_bundle_id_binds_familiar_source_labels_targets_and_exact_bytes() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace), ("cody", &workspace)])?;
        let base = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"same".to_vec(),
        }];
        let same = build_import_plan(temp.path(), "sage", MemoryImportSourceKind::Native, &base)?;
        let same_again =
            build_import_plan(temp.path(), "sage", MemoryImportSourceKind::Native, &base)?;
        assert_eq!(same.bundle_id, same_again.bundle_id);
        assert!(same.bundle_id.starts_with("blake3-"));
        let alternate_target = bundle_id(
            "sage",
            MemoryImportSourceKind::Native,
            &[ProposedEntry {
                source: &base[0],
                target_name: "alternate.md".to_owned(),
                digest: blake3_digest(&base[0].bytes),
            }],
        );
        assert_ne!(alternate_target, same.bundle_id);

        let changed_bytes = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"changed".to_vec(),
        }];
        let changed_label = vec![DiscoveredSource {
            source_label: "notes/MEMORY.md".to_owned(),
            bytes: b"same".to_vec(),
        }];
        let variants = [
            build_import_plan(
                temp.path(),
                "sage",
                MemoryImportSourceKind::Native,
                &changed_bytes,
            )?,
            build_import_plan(
                temp.path(),
                "sage",
                MemoryImportSourceKind::Native,
                &changed_label,
            )?,
            build_import_plan(temp.path(), "cody", MemoryImportSourceKind::Native, &base)?,
            build_import_plan(temp.path(), "sage", MemoryImportSourceKind::Openclaw, &base)?,
        ];
        assert!(variants
            .iter()
            .all(|variant| variant.bundle_id != same.bundle_id));
        Ok(())
    }

    #[test]
    fn plans_classify_exact_files_and_make_any_conflict_whole_plan_conflict() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        write_file(&temp.path().join("memory/sage/memory.md"), b"same")?;
        write_file(&temp.path().join("memory/sage/memory-divergent.md"), b"old")?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"same".to_vec(),
            },
            DiscoveredSource {
                source_label: "memory/divergent.md".to_owned(),
                bytes: b"new".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/new.md".to_owned(),
                bytes: b"create".to_vec(),
            },
        ];

        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;

        assert_eq!(plan.status, ImportPlanStatus::Conflict);
        assert!(!plan.apply_eligible);
        assert_eq!(plan.create_count, 1);
        assert_eq!(plan.unchanged_count, 1);
        assert_eq!(plan.conflict_count, 1);
        assert_eq!(
            plan.entries
                .iter()
                .map(|entry| entry.status)
                .collect::<Vec<_>>(),
            vec![
                PlanEntryStatus::Unchanged,
                PlanEntryStatus::Conflict,
                PlanEntryStatus::Create
            ]
        );
        assert!(!temp.path().join("memory/sage/notes-new.md").exists());
        assert!(!temp.path().join("memory-import").exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn plans_treat_symlink_nonregular_and_case_colliding_targets_as_conflicts() -> Result<()> {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        fs::create_dir_all(temp.path().join("memory/sage"))?;
        write_file(&temp.path().join("outside.md"), b"same")?;
        symlink(
            temp.path().join("outside.md"),
            temp.path().join("memory/sage/memory-link.md"),
        )?;
        let socket = UnixListener::bind(temp.path().join("memory/sage/memory-socket.md"))?;
        write_file(&temp.path().join("memory/sage/Notes-Case.md"), b"same")?;
        let sources = vec![
            DiscoveredSource {
                source_label: "memory/link.md".to_owned(),
                bytes: b"same".to_vec(),
            },
            DiscoveredSource {
                source_label: "memory/socket.md".to_owned(),
                bytes: b"same".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/case.md".to_owned(),
                bytes: b"same".to_vec(),
            },
        ];

        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        drop(socket);

        assert_eq!(plan.status, ImportPlanStatus::Conflict);
        assert_eq!(plan.conflict_count, 3);
        assert!(plan
            .entries
            .iter()
            .all(|entry| entry.status == PlanEntryStatus::Conflict));
        Ok(())
    }

    #[test]
    fn plans_reject_duplicate_normalized_labels_and_unsafe_familiars_before_inspection(
    ) -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(
            temp.path(),
            &[("sage", &workspace), ("../sage", &workspace)],
        )?;
        let duplicate = vec![
            DiscoveredSource {
                source_label: "notes/Caf\u{e9}.md".to_owned(),
                bytes: b"one".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/CAFE\u{301}.md".to_owned(),
                bytes: b"two".to_vec(),
            },
        ];

        let duplicate_error = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &duplicate,
        )
        .expect_err("Unicode normalization and case-fold collisions must fail");
        assert!(
            duplicate_error
                .to_string()
                .contains("logical label collision"),
            "{duplicate_error:#}"
        );

        let exact_duplicate = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"one".to_vec(),
            },
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"two".to_vec(),
            },
        ];
        assert!(build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &exact_duplicate,
        )
        .expect_err("duplicate labels must fail")
        .to_string()
        .contains("logical label collision"));

        let unsafe_error =
            build_import_plan(temp.path(), "../sage", MemoryImportSourceKind::Native, &[])
                .expect_err("unsafe registered familiar IDs must be rejected");
        assert!(unsafe_error.to_string().contains("safe single component"));
        assert!(!temp.path().join("memory").exists());
        Ok(())
    }

    #[test]
    fn plans_reject_unregistered_internal_familiar_inputs() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;

        let error = build_import_plan(temp.path(), "unknown", MemoryImportSourceKind::Native, &[])
            .expect_err("planner must independently verify registration");

        assert_eq!(error.to_string(), "unknown familiar `unknown`");
        assert!(!temp.path().join("memory").exists());
        Ok(())
    }

    #[test]
    fn plans_existing_digest_rejects_streams_that_grow_or_shrink_from_opened_length() {
        let mut exact = io::Cursor::new(b"abc");
        assert_eq!(
            digest_exact_length_stream(&mut exact, 3),
            Some(
                "blake3:6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
                    .to_owned()
            )
        );

        let mut grew = io::Cursor::new(b"abcd");
        assert_eq!(digest_exact_length_stream(&mut grew, 3), None);

        let mut shrank = io::Cursor::new(b"ab");
        assert_eq!(digest_exact_length_stream(&mut shrank, 3), None);
    }

    #[test]
    fn plans_target_name_validation_rejects_portable_collisions_and_unsafe_names() {
        for names in [
            vec!["notes.md".to_owned(), "NOTES.md".to_owned()],
            vec!["caf\u{e9}.md".to_owned(), "CAFE\u{301}.md".to_owned()],
        ] {
            let error = validate_unique_target_names(&names)
                .expect_err("case-folded target names must collide");
            assert!(error.to_string().contains("target-name collision"));
        }

        for unsafe_name in [
            ".",
            "..",
            "nested/name.md",
            "nested\\name.md",
            "CON.md",
            "name.md ",
            "name.\n.md",
            "name.txt",
        ] {
            assert!(
                validate_target_name(unsafe_name).is_err(),
                "unsafe target name was accepted: {unsafe_name:?}"
            );
        }
    }

    #[test]
    fn plans_normalize_unusual_labels_to_ascii_safe_flat_names_or_fail_closed() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "...md".to_owned(),
                bytes: b"dots".to_vec(),
            },
            DiscoveredSource {
                source_label: "NUL.md".to_owned(),
                bytes: b"device".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/\u{4e2d}\u{56fd}.md".to_owned(),
                bytes: b"unicode".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/trailing. .md".to_owned(),
                bytes: b"portable".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        assert_eq!(plan.entries.len(), 4);
        for entry in &plan.entries {
            assert_portable_target_name(&entry.target_name);
        }
        assert!(plan
            .entries
            .iter()
            .any(|entry| entry.target_name.starts_with("nul-")));
        assert!(plan
            .entries
            .iter()
            .any(|entry| entry.target_name.starts_with("notes-") && entry.target_name.len() > 20));

        for invalid_label in [
            "../escape.md",
            "notes/../escape.md",
            "notes\\escape.md",
            "notes//empty.md",
            "notes/control\n.md",
            "/absolute.md",
        ] {
            let invalid = vec![DiscoveredSource {
                source_label: invalid_label.to_owned(),
                bytes: b"secret".to_vec(),
            }];
            assert!(
                build_import_plan(
                    temp.path(),
                    "sage",
                    MemoryImportSourceKind::Native,
                    &invalid,
                )
                .is_err(),
                "invalid logical label was accepted: {invalid_label:?}"
            );
        }
        assert!(!temp.path().join("memory").exists());
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_creates_private_redacted_bundle_and_verifies_targets() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"root secret".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/topic.md".to_owned(),
                bytes: b"topic secret".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;

        let report = apply_import_plan(temp.path(), &plan, &sources)?;

        assert_eq!(report.status, ImportPlanStatus::Verified);
        assert_eq!(report.create_count, 2);
        assert_eq!(report.unchanged_count, 0);
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"root secret"
        );
        assert_eq!(
            fs::read(temp.path().join("memory/sage/notes-topic.md"))?,
            b"topic secret"
        );

        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        assert_eq!(
            bundle.file_name().and_then(|name| name.to_str()),
            Some(plan.bundle_id.as_str())
        );
        let manifest = fs::read(bundle.join("manifest.json"))?;
        let journal = fs::read(bundle.join("journal.jsonl"))?;
        let manifest_value: serde_json::Value = serde_json::from_slice(&manifest)?;
        assert_eq!(manifest_value["protocol_version"], IMPORT_PROTOCOL_VERSION);
        assert_eq!(manifest_value["familiar_id"], "sage");
        assert_eq!(manifest_value["source_kind"], "native");
        assert_eq!(manifest_value["bundle_id"], plan.bundle_id);
        assert_eq!(manifest_value["entries"][0]["source_label"], "MEMORY.md");
        assert_eq!(manifest_value["entries"][0]["target_name"], "memory.md");
        assert_eq!(manifest_value["entries"][0]["byte_length"], 11);
        assert_eq!(manifest_value["entries"][0]["initial_status"], "prepared");
        let records = journal
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(serde_json::from_slice::<JournalRecord>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(
            records
                .iter()
                .enumerate()
                .map(|(index, record)| (index as u64, record.sequence))
                .collect::<Vec<_>>(),
            records
                .iter()
                .map(|record| (record.sequence, record.sequence))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            records
                .iter()
                .filter(|record| record.scope == JournalScope::Bundle)
                .map(|record| record.state)
                .collect::<Vec<_>>(),
            vec![
                JournalState::Prepared,
                JournalState::Publishing,
                JournalState::Verified
            ]
        );
        let rendered = format!(
            "{}\n{}",
            String::from_utf8_lossy(&manifest),
            String::from_utf8_lossy(&journal)
        );
        assert!(!rendered.contains("root secret"));
        assert!(!rendered.contains("topic secret"));
        assert!(!rendered.contains(&workspace.to_string_lossy().into_owned()));
        assert!(bundle.join("staged/memory.md").is_file());
        assert!(bundle.join("staged/notes-topic.md").is_file());
        assert!(!temp.path().join("memory/sage/manifest.json").exists());
        assert!(!temp.path().join("memory/sage/journal.jsonl").exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            for directory in [
                temp.path().join("memory-migrations"),
                temp.path().join("memory-migrations/sage"),
                bundle.clone(),
                bundle.join("staged"),
            ] {
                assert_eq!(
                    fs::symlink_metadata(directory)?.permissions().mode() & 0o777,
                    0o700
                );
            }
            for file in [
                bundle.join("manifest.json"),
                bundle.join("journal.jsonl"),
                bundle.join("staged/memory.md"),
                bundle.join("staged/notes-topic.md"),
            ] {
                assert_eq!(
                    fs::symlink_metadata(file)?.permissions().mode() & 0o777,
                    0o600
                );
            }
        }
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_exact_target_race_is_unchanged_and_never_overwritten() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"same bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let target = temp.path().join("memory/sage/memory.md");
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if matches!(step, ApplyStep::BeforePublish(name) if name == "memory.md") {
                fs::write(&target, b"same bytes")?;
            }
            Ok(ApplyHookAction::Continue)
        });

        let report = apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)?;

        assert_eq!(report.create_count, 0);
        assert_eq!(report.unchanged_count, 1);
        assert_eq!(report.entries[0].status, PlanEntryStatus::Unchanged);
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        let target_metadata = fs::metadata(&target)?;
        let staged_metadata = fs::metadata(bundle.join("staged/memory.md"))?;
        assert!(!opened_metadata_stable_std(
            &target_metadata,
            &staged_metadata
        ));
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_created_target_does_not_alias_private_staging() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"original bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;

        apply_import_plan(temp.path(), &plan, &sources)?;
        let target = temp.path().join("memory/sage/memory.md");
        let staged = bundle_path(temp.path(), "sage", &plan.bundle_id)?.join("staged/memory.md");
        fs::write(&target, b"changed!")?;

        assert_eq!(fs::read(target)?, b"changed!");
        assert_eq!(fs::read(staged)?, b"original bytes");
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn apply_replaces_preexisting_candidates_with_current_run_candidates() -> Result<()> {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"existing user bytes".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let first_entry = plan
            .entries
            .iter()
            .find(|entry| entry.logical_label == "MEMORY.md")
            .expect("first entry must exist");
        let second_entry = plan
            .entries
            .iter()
            .find(|entry| entry.logical_label == "notes/second.md")
            .expect("second entry must exist");
        let target_dir = temp.path().join("memory/sage");
        let work_root = target_dir.join(TARGET_WORK_DIRECTORY);
        let work = work_root.join(&plan.bundle_id);
        fs::create_dir_all(&work)?;
        fs::set_permissions(&work_root, fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(&work, fs::Permissions::from_mode(0o700))?;
        let canonical = target_dir.join(&first_entry.target_name);
        fs::write(&canonical, b"existing user bytes")?;
        fs::set_permissions(&canonical, fs::Permissions::from_mode(0o600))?;
        fs::hard_link(
            &canonical,
            work.join(format!("{}.candidate", first_entry.target_name)),
        )?;
        let malformed = work.join(format!("{}.candidate", second_entry.target_name));
        fs::write(&malformed, b"attacker candidate")?;
        fs::set_permissions(&malformed, fs::Permissions::from_mode(0o600))?;

        let report = apply_import_plan(temp.path(), &plan, &sources)?;

        assert_eq!(fs::read(canonical)?, b"existing user bytes");
        assert_eq!(report.status, ImportPlanStatus::Verified);
        assert_eq!(report.create_count, 1);
        assert_eq!(report.unchanged_count, 1);
        assert_eq!(
            fs::read(target_dir.join(&second_entry.target_name))?,
            b"second"
        );
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_does_not_claim_a_candidate_linked_by_an_external_publisher() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let first = temp.path().join("memory/sage/memory.md");
        let second = temp.path().join("memory/sage/notes-second.md");
        let candidate = temp
            .path()
            .join("memory/sage")
            .join(TARGET_WORK_DIRECTORY)
            .join(&plan.bundle_id)
            .join("memory.md.candidate");
        let mut externally_published = false;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if !externally_published
                && matches!(step, ApplyStep::BeforePublish(name) if name == "memory.md")
            {
                externally_published = true;
                fs::hard_link(&candidate, &first)?;
            }
            if matches!(step, ApplyStep::BeforePublish(name) if name == "notes-second.md") {
                fs::write(&second, b"conflict")?;
            }
            Ok(ApplyHookAction::Continue)
        });

        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the later conflict must abort the batch");

        assert!(externally_published);
        assert_eq!(fs::read(first)?, b"first");
        assert_eq!(fs::read(second)?, b"conflict");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_resume_keeps_a_preverified_external_candidate_link_as_unchanged() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"external bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let target = temp.path().join("memory/sage/memory.md");
        let candidate = temp
            .path()
            .join("memory/sage")
            .join(TARGET_WORK_DIRECTORY)
            .join(&plan.bundle_id)
            .join("memory.md.candidate");
        let mut linked = false;
        let mut interrupted = false;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if !linked && matches!(step, ApplyStep::BeforePublish(name) if name == "memory.md") {
                linked = true;
                fs::hard_link(&candidate, &target)?;
            }
            if !interrupted
                && matches!(
                    step,
                    ApplyStep::BeforeVerifiedJournalWrite(name) if name == "memory.md"
                )
            {
                interrupted = true;
                return Ok(ApplyHookAction::Interrupt);
            }
            Ok(ApplyHookAction::Continue)
        });

        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("verification journaling must be interrupted");
        assert!(linked && interrupted);

        let resumed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let report = apply_import_plan(temp.path(), &resumed_plan, &sources)?;

        assert_eq!(report.unchanged_count, 1);
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        let journal = parse_journal(&fs::read(bundle.join(JOURNAL_FILE))?)?;
        assert!(!journal.records.iter().any(|record| {
            record.target_name.as_deref() == Some("memory.md")
                && record.state == JournalState::Published
                && record.outcome == Some(JournalOutcome::Created)
        }));
        assert!(journal.records.iter().any(|record| {
            record.target_name.as_deref() == Some("memory.md")
                && record.state == JournalState::Verified
                && record.outcome == Some(JournalOutcome::Unchanged)
        }));
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_resume_persists_verified_created_as_unchanged_without_a_pinned_handle() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(
                if matches!(step, ApplyStep::BeforePublish(name) if name == "notes-second.md") {
                    ApplyHookAction::Interrupt
                } else {
                    ApplyHookAction::Continue
                },
            )
        });

        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the second entry must be interrupted after the first is verified");

        let resumed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let report = apply_import_plan(temp.path(), &resumed_plan, &sources)?;

        assert_eq!(report.create_count, 1);
        assert_eq!(report.unchanged_count, 1);
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        let journal = parse_journal(&fs::read(bundle.join(JOURNAL_FILE))?)?;
        let first = journal
            .entry_states
            .get("memory.md")
            .expect("first entry must remain journaled");
        assert_eq!(first.state, JournalState::Verified);
        assert_eq!(first.outcome, Some(JournalOutcome::Unchanged));
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn apply_rollback_preserves_a_same_digest_candidate_identity_replacement() -> Result<()> {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let first = temp.path().join("memory/sage/memory.md");
        let second = temp.path().join("memory/sage/notes-second.md");
        let candidate = temp
            .path()
            .join("memory/sage")
            .join(TARGET_WORK_DIRECTORY)
            .join(&plan.bundle_id)
            .join("memory.md.candidate");
        let replacement = temp.path().join("same-digest-replacement");
        let mut replaced = false;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if !replaced
                && matches!(step, ApplyStep::BeforePublish(name) if name == "notes-second.md")
            {
                replaced = true;
                fs::write(&replacement, b"first")?;
                fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;
                fs::remove_file(&first)?;
                fs::remove_file(&candidate)?;
                fs::hard_link(&replacement, &first)?;
                fs::hard_link(&replacement, &candidate)?;
                fs::write(&second, b"conflict")?;
            }
            Ok(ApplyHookAction::Continue)
        });

        let error = apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the later conflict must roll the current batch back");

        assert!(replaced);
        assert_eq!(fs::read(first)?, b"first");
        assert_eq!(fs::read(second)?, b"conflict");
        assert!(error.to_string().contains("manual recovery"), "{error:#}");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_rolls_back_a_publish_when_the_published_journal_append_fails() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"journal failure bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if matches!(step, ApplyStep::AfterPublishBeforeJournal(name) if name == "memory.md") {
                bail!("injected published-journal failure");
            }
            Ok(ApplyHookAction::Continue)
        });

        let error = apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the injected journal failure must abort the apply");

        assert!(
            error.to_string().contains("published-journal failure"),
            "{error:#}"
        );
        assert!(!temp.path().join("memory/sage/memory.md").exists());
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        assert!(
            fs::read_to_string(bundle.join(JOURNAL_FILE))?.contains("\"state\":\"rolled_back\"")
        );
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_recovers_when_the_published_record_is_written_but_not_synced() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"unsynced journal bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if matches!(
                step,
                ApplyStep::AfterPublishedJournalWriteBeforeSync(name) if name == "memory.md"
            ) {
                bail!("injected journal sync failure");
            }
            Ok(ApplyHookAction::Continue)
        });

        let error = apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the injected sync failure must abort the apply");

        assert!(
            error.to_string().contains("journal sync failure"),
            "{error:#}"
        );
        assert!(!temp.path().join("memory/sage/memory.md").exists());
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        let journal = fs::read_to_string(bundle.join(JOURNAL_FILE))?;
        assert_eq!(
            journal
                .lines()
                .enumerate()
                .filter_map(|(index, line)| {
                    let record: JournalRecord = serde_json::from_str(line).ok()?;
                    Some((index as u64, record.sequence))
                })
                .collect::<Vec<_>>(),
            journal
                .lines()
                .filter_map(|line| serde_json::from_str::<JournalRecord>(line).ok())
                .map(|record| (record.sequence, record.sequence))
                .collect::<Vec<_>>()
        );
        assert!(journal.contains("\"state\":\"rolled_back\""));
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_repairs_a_torn_final_journal_record_before_resuming() -> Result<()> {
        use std::io::Write as _;

        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"repair bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(if matches!(step, ApplyStep::Prepared) {
                ApplyHookAction::Interrupt
            } else {
                ApplyHookAction::Continue
            })
        });
        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("preparation must be interrupted");
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        let mut journal = fs::OpenOptions::new()
            .append(true)
            .open(bundle.join(JOURNAL_FILE))?;
        journal.write_all(br#"{"protocol_version":1,"sequence":"#)?;
        journal.sync_all()?;

        let report = apply_import_plan(temp.path(), &plan, &sources)?;

        assert_eq!(report.status, ImportPlanStatus::Verified);
        let journal = fs::read(bundle.join(JOURNAL_FILE))?;
        assert!(journal.ends_with(b"\n"));
        assert!(parse_journal(&journal).is_ok());
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn apply_rejects_a_hardlinked_journal_without_mutating_the_other_link() -> Result<()> {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"journal bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(if matches!(step, ApplyStep::Prepared) {
                ApplyHookAction::Interrupt
            } else {
                ApplyHookAction::Continue
            })
        });
        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("preparation must be interrupted");
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        let external = temp.path().join("external-journal");
        let external_bytes = br#"{"external":"must remain intact"}"#;
        fs::write(&external, external_bytes)?;
        fs::set_permissions(&external, fs::Permissions::from_mode(0o600))?;
        fs::remove_file(bundle.join(JOURNAL_FILE))?;
        fs::hard_link(&external, bundle.join(JOURNAL_FILE))?;

        let error = apply_import_plan(temp.path(), &plan, &sources)
            .expect_err("a multiply linked mutable journal must be rejected");

        assert!(error.to_string().contains("unsafe"), "{error:#}");
        assert_eq!(fs::read(external)?, external_bytes);
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_rollback_keeps_a_replacement_raced_at_the_quarantine_boundary() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let first = temp.path().join("memory/sage/memory.md");
        let second = temp.path().join("memory/sage/notes-second.md");
        let mut raced = false;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if matches!(step, ApplyStep::BeforePublish(name) if name == "notes-second.md") {
                fs::write(&second, b"conflict")?;
            }
            if !raced
                && matches!(step, ApplyStep::BeforeRollbackQuarantine(name) if name == "memory.md")
            {
                raced = true;
                fs::remove_file(&first)?;
                fs::write(&first, b"user replacement")?;
            }
            Ok(ApplyHookAction::Continue)
        });

        let error = apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the conflicting second target must roll the batch back");

        assert!(error.to_string().contains("manual recovery"), "{error:#}");
        assert!(raced, "the rollback race hook did not run");
        assert_eq!(fs::read(first)?, b"user replacement");
        assert_eq!(fs::read(second)?, b"conflict");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_rollback_keeps_a_directory_raced_at_the_quarantine_boundary() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let first = temp.path().join("memory/sage/memory.md");
        let second = temp.path().join("memory/sage/notes-second.md");
        let mut raced = false;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if matches!(step, ApplyStep::BeforePublish(name) if name == "notes-second.md") {
                fs::write(&second, b"conflict")?;
            }
            if !raced
                && matches!(step, ApplyStep::BeforeRollbackQuarantine(name) if name == "memory.md")
            {
                raced = true;
                fs::remove_file(&first)?;
                fs::create_dir(&first)?;
                fs::write(first.join("sentinel"), b"user directory")?;
            }
            Ok(ApplyHookAction::Continue)
        });

        let error = apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the conflicting second target must roll the batch back");

        assert!(error.to_string().contains("manual recovery"), "{error:#}");
        assert!(raced, "the rollback race hook did not run");
        assert_eq!(fs::read(first.join("sentinel"))?, b"user directory");
        assert_eq!(fs::read(second)?, b"conflict");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_rejects_a_concurrent_transaction_for_the_same_familiar() -> Result<()> {
        use std::sync::{mpsc, Arc, Barrier};
        use std::thread;

        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"locked bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let home = temp.path().to_path_buf();
        let thread_plan = plan.clone();
        let thread_sources = sources.clone();
        let release = Arc::new(Barrier::new(2));
        let thread_release = Arc::clone(&release);
        let (prepared_tx, prepared_rx) = mpsc::channel();

        let first = thread::spawn(move || {
            let mut prepared_tx = Some(prepared_tx);
            let mut hook = TestApplyHook::new(|step: &ApplyStep| {
                if matches!(step, ApplyStep::Prepared) {
                    if let Some(tx) = prepared_tx.take() {
                        tx.send(()).expect("test receiver remains available");
                        thread_release.wait();
                    }
                }
                Ok(ApplyHookAction::Continue)
            });
            apply_import_plan_with_hook(&home, &thread_plan, &thread_sources, &mut hook)
        });
        prepared_rx.recv()?;

        let concurrent = apply_import_plan(temp.path(), &plan, &sources)
            .expect_err("a concurrent transaction must fail instead of sharing the journal");
        assert!(
            concurrent
                .to_string()
                .contains("another memory migration is active"),
            "{concurrent:#}"
        );
        release.wait();
        let first_report = first.join().expect("first apply thread must not panic")?;
        assert_eq!(first_report.status, ImportPlanStatus::Verified);
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_blocks_changed_sources_after_an_older_bundle_may_have_published() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let original_sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"old bytes".to_vec(),
        }];
        let original_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &original_sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(
                if matches!(step, ApplyStep::AfterPublishBeforeJournal(name) if name == "memory.md")
                {
                    ApplyHookAction::Interrupt
                } else {
                    ApplyHookAction::Continue
                },
            )
        });
        apply_import_plan_with_hook(temp.path(), &original_plan, &original_sources, &mut hook)
            .expect_err("the original bundle must be interrupted after publication");
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"old bytes"
        );

        let changed_sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"new bytes".to_vec(),
        }];
        let changed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &changed_sources,
        )?;
        let error = apply_import_plan(temp.path(), &changed_plan, &changed_sources)
            .expect_err("an older potentially published bundle must require manual recovery");

        assert!(error.to_string().contains("manual recovery"), "{error:#}");
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"old bytes"
        );
        let original_bundle = bundle_path(temp.path(), "sage", &original_plan.bundle_id)?;
        assert!(fs::read_to_string(original_bundle.join(JOURNAL_FILE))?
            .contains("\"state\":\"rolled_back\""));
        assert!(fs::read_to_string(original_bundle.join(JOURNAL_FILE))?
            .contains("\"outcome\":\"manual_recovery\""));
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_rolls_back_an_older_bundle_interrupted_during_partial_staging() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let original_sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"old first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"old second".to_vec(),
            },
        ];
        let original_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &original_sources,
        )?;
        let mut interrupted = false;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if !interrupted && matches!(step, ApplyStep::AfterStage(_)) {
                interrupted = true;
                return Ok(ApplyHookAction::Interrupt);
            }
            Ok(ApplyHookAction::Continue)
        });
        apply_import_plan_with_hook(temp.path(), &original_plan, &original_sources, &mut hook)
            .expect_err("the original bundle must be interrupted during staging");
        let original_bundle = bundle_path(temp.path(), "sage", &original_plan.bundle_id)?;
        assert_eq!(
            fs::read_dir(original_bundle.join(STAGED_DIRECTORY))?.count(),
            1
        );
        assert!(!temp.path().join("memory/sage/memory.md").exists());

        let changed_sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"new first".to_vec(),
        }];
        let changed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &changed_sources,
        )?;
        let report = apply_import_plan(temp.path(), &changed_plan, &changed_sources)?;

        assert_eq!(report.status, ImportPlanStatus::Verified);
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"new first"
        );
        assert!(fs::read_to_string(original_bundle.join(JOURNAL_FILE))?
            .contains("\"state\":\"rolled_back\""));
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_rolls_back_an_empty_journal_bundle_before_a_changed_source_plan() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let original_sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"old bytes".to_vec(),
        }];
        let original_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &original_sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(if matches!(step, ApplyStep::Prepared) {
                ApplyHookAction::Interrupt
            } else {
                ApplyHookAction::Continue
            })
        });
        apply_import_plan_with_hook(temp.path(), &original_plan, &original_sources, &mut hook)
            .expect_err("the original bundle must be interrupted");
        let original_bundle = bundle_path(temp.path(), "sage", &original_plan.bundle_id)?;
        fs::write(original_bundle.join(JOURNAL_FILE), [])?;
        fs::remove_file(original_bundle.join("staged/memory.md"))?;

        let changed_sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"new bytes".to_vec(),
        }];
        let changed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &changed_sources,
        )?;
        let report = apply_import_plan(temp.path(), &changed_plan, &changed_sources)?;

        assert_eq!(report.status, ImportPlanStatus::Verified);
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"new bytes"
        );
        assert!(fs::read_to_string(original_bundle.join(JOURNAL_FILE))?
            .contains("\"state\":\"rolled_back\""));
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_discards_a_torn_prepublication_manifest_before_a_changed_source_plan() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let original_sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"old bytes".to_vec(),
        }];
        let original_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &original_sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(if matches!(step, ApplyStep::Prepared) {
                ApplyHookAction::Interrupt
            } else {
                ApplyHookAction::Continue
            })
        });
        apply_import_plan_with_hook(temp.path(), &original_plan, &original_sources, &mut hook)
            .expect_err("the original bundle must be interrupted before publication");
        let original_bundle = bundle_path(temp.path(), "sage", &original_plan.bundle_id)?;
        fs::write(
            original_bundle.join(MANIFEST_FILE),
            br#"{"protocol_version":"#,
        )?;

        let changed_sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"new bytes".to_vec(),
        }];
        let changed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &changed_sources,
        )?;
        let report = apply_import_plan(temp.path(), &changed_plan, &changed_sources)?;

        assert_eq!(report.status, ImportPlanStatus::Verified);
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"new bytes"
        );
        assert!(!original_bundle.exists());
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_discards_an_empty_bundle_interrupted_before_staged_creation() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let abandoned_sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"abandoned bytes".to_vec(),
        }];
        let abandoned_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &abandoned_sources,
        )?;
        let abandoned_bundle = bundle_path(temp.path(), "sage", &abandoned_plan.bundle_id)?;
        fs::create_dir_all(&abandoned_bundle)?;
        for directory in [
            temp.path().join(MIGRATIONS_DIRECTORY),
            temp.path().join(MIGRATIONS_DIRECTORY).join("sage"),
            abandoned_bundle.clone(),
        ] {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }

        let changed_sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"new bytes".to_vec(),
        }];
        let changed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &changed_sources,
        )?;
        let report = apply_import_plan(temp.path(), &changed_plan, &changed_sources)?;

        assert_eq!(report.status, ImportPlanStatus::Verified);
        assert!(!abandoned_bundle.exists());
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"new bytes"
        );
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_rebuilds_a_partial_staged_file_before_publication() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first complete bytes".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second complete bytes".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(
                if matches!(step, ApplyStep::AfterStage(name) if name == "memory.md") {
                    ApplyHookAction::Interrupt
                } else {
                    ApplyHookAction::Continue
                },
            )
        });
        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the bundle must be interrupted after its first staged file");
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        fs::write(bundle.join("staged/memory.md"), b"partial")?;

        let report = apply_import_plan(temp.path(), &plan, &sources)?;

        assert_eq!(report.status, ImportPlanStatus::Verified);
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"first complete bytes"
        );
        assert_eq!(
            fs::read(bundle.join("staged/memory.md"))?,
            b"first complete bytes"
        );
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_resumes_after_prepared_without_publishing_twice() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"resume secret".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(if matches!(step, ApplyStep::Prepared) {
                ApplyHookAction::Interrupt
            } else {
                ApplyHookAction::Continue
            })
        });

        let interrupted = apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the test hook must interrupt after durable preparation");
        assert!(interrupted.to_string().contains("interrupted"));
        assert!(!temp.path().join("memory/sage/memory.md").exists());

        let resumed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let report = apply_import_plan(temp.path(), &resumed_plan, &sources)?;
        assert_eq!(report.status, ImportPlanStatus::Verified);
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"resume secret"
        );
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_resumes_an_entry_prepared_before_publication() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"prepared entry".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(
                if matches!(step, ApplyStep::BeforePublish(name) if name == "memory.md") {
                    ApplyHookAction::Interrupt
                } else {
                    ApplyHookAction::Continue
                },
            )
        });

        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the entry must be interrupted after durable preparation");
        assert!(!temp.path().join("memory/sage/memory.md").exists());

        let resumed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let report = apply_import_plan(temp.path(), &resumed_plan, &sources)?;

        assert_eq!(report.status, ImportPlanStatus::Verified);
        assert_eq!(report.create_count, 1);
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"prepared entry"
        );
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_resumes_after_candidate_sync_before_entry_prepared() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"candidate synced".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(
                if matches!(
                    step,
                    ApplyStep::AfterCandidateSyncBeforePrepared(name) if name == "memory.md"
                ) {
                    ApplyHookAction::Interrupt
                } else {
                    ApplyHookAction::Continue
                },
            )
        });

        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the candidate-to-prepared window must be interrupted");
        assert!(!temp.path().join("memory/sage/memory.md").exists());

        let resumed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let report = apply_import_plan(temp.path(), &resumed_plan, &sources)?;

        assert_eq!(report.status, ImportPlanStatus::Verified);
        assert_eq!(report.create_count, 1);
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"candidate synced"
        );
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_rolls_back_current_run_entries_in_a_resumed_bundle() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut prepare_hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(if matches!(step, ApplyStep::Prepared) {
                ApplyHookAction::Interrupt
            } else {
                ApplyHookAction::Continue
            })
        });
        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut prepare_hook)
            .expect_err("the bundle must be interrupted after preparation");

        let conflict = temp.path().join("memory/sage/notes-second.md");
        let mut conflict_hook = TestApplyHook::new(|step: &ApplyStep| {
            if matches!(step, ApplyStep::BeforePublish(name) if name == "notes-second.md") {
                fs::write(&conflict, b"conflict")?;
            }
            Ok(ApplyHookAction::Continue)
        });
        let resumed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;

        let error =
            apply_import_plan_with_hook(temp.path(), &resumed_plan, &sources, &mut conflict_hook)
                .expect_err("the later conflict must roll back current-run publications");

        assert!(!error.to_string().contains("manual recovery"), "{error:#}");
        assert!(!temp.path().join("memory/sage/memory.md").exists());
        assert_eq!(fs::read(conflict)?, b"conflict");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_reconciles_prejournal_publication_as_unchanged_after_restart() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut interrupted_once = false;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if !interrupted_once
                && matches!(
                    step,
                    ApplyStep::AfterPublishBeforeJournal(name) if name == "memory.md"
                )
            {
                interrupted_once = true;
                Ok(ApplyHookAction::Interrupt)
            } else {
                Ok(ApplyHookAction::Continue)
            }
        });

        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the test hook must interrupt after the first no-replace publish");
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"first"
        );
        assert!(!temp.path().join("memory/sage/notes-second.md").exists());

        let resumed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let report = apply_import_plan(temp.path(), &resumed_plan, &sources)?;
        assert_eq!(report.status, ImportPlanStatus::Verified);
        assert_eq!(report.create_count, 1);
        assert_eq!(report.unchanged_count, 1);
        assert_eq!(
            fs::read(temp.path().join("memory/sage/notes-second.md"))?,
            b"second"
        );
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        let journal = fs::read_to_string(bundle.join(JOURNAL_FILE))?;
        assert_eq!(
            journal
                .lines()
                .filter(|line| {
                    line.contains("\"target_name\":\"memory.md\"")
                        && line.contains("\"state\":\"published\"")
                })
                .count(),
            0
        );
        assert!(journal.lines().any(|line| {
            line.contains("\"target_name\":\"memory.md\"")
                && line.contains("\"state\":\"verified\"")
                && line.contains("\"outcome\":\"unchanged\"")
        }));
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_reconciles_a_prejournal_target_with_missing_candidate_as_unchanged() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"published bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(
                if matches!(step, ApplyStep::AfterPublishBeforeJournal(name) if name == "memory.md")
                {
                    ApplyHookAction::Interrupt
                } else {
                    ApplyHookAction::Continue
                },
            )
        });
        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("publication must be interrupted before its journal record");
        let work = temp
            .path()
            .join("memory/sage")
            .join(TARGET_WORK_DIRECTORY)
            .join(&plan.bundle_id);
        fs::remove_file(work.join("memory.md.candidate"))?;

        let report = apply_import_plan(temp.path(), &plan, &sources)?;

        assert_eq!(report.status, ImportPlanStatus::Verified);
        assert_eq!(report.create_count, 0);
        assert_eq!(report.unchanged_count, 1);
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"published bytes"
        );
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_requires_manual_recovery_when_a_published_target_is_replaced() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"published bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut published_hook_ran = false;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if matches!(step, ApplyStep::AfterPublishedJournalSync(name) if name == "memory.md") {
                published_hook_ran = true;
                return Ok(ApplyHookAction::Interrupt);
            }
            Ok(ApplyHookAction::Continue)
        });
        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the bundle must stop after the published record is durable");
        assert!(published_hook_ran);
        let target = temp.path().join("memory/sage/memory.md");
        fs::remove_file(&target)?;
        fs::write(&target, b"user replacement")?;

        let error = apply_import_plan(temp.path(), &plan, &sources)
            .expect_err("a replaced published target must require manual recovery");

        assert!(error.to_string().contains("manual recovery"), "{error:#}");
        assert_eq!(fs::read(target)?, b"user replacement");
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        let journal = fs::read_to_string(bundle.join(JOURNAL_FILE))?;
        assert!(!journal.contains("\"outcome\":\"not_published\""));
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_restart_never_republishes_a_missing_durable_created_target() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"published bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(
                if matches!(step, ApplyStep::AfterPublishedJournalSync(name) if name == "memory.md")
                {
                    ApplyHookAction::Interrupt
                } else {
                    ApplyHookAction::Continue
                },
            )
        });
        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the bundle must stop after durable publication");
        let target = temp.path().join("memory/sage/memory.md");
        fs::remove_file(&target)?;

        let error = apply_import_plan(temp.path(), &plan, &sources)
            .expect_err("a missing durable target must require manual recovery");

        assert!(error.to_string().contains("manual recovery"), "{error:#}");
        assert!(!target.exists());
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_resumes_a_not_published_rollback_without_touching_the_conflict() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"import bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let target = temp.path().join("memory/sage/memory.md");
        let mut interrupted = false;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if matches!(step, ApplyStep::BeforePublish(name) if name == "memory.md") {
                fs::write(&target, b"user conflict")?;
            }
            if !interrupted
                && matches!(
                    step,
                    ApplyStep::AfterNotPublishedRollingBack(name) if name == "memory.md"
                )
            {
                interrupted = true;
                return Ok(ApplyHookAction::Interrupt);
            }
            Ok(ApplyHookAction::Continue)
        });
        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the not-published rollback must be interrupted");
        assert!(interrupted);

        let resumed = apply_import_plan(temp.path(), &plan, &sources)
            .expect_err("the cleanly rolled-back bundle remains terminal");

        assert!(resumed.to_string().contains("rolled back"), "{resumed:#}");
        assert!(!resumed.to_string().contains("manual recovery"));
        assert_eq!(fs::read(target)?, b"user conflict");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_rejects_corrupt_staging_before_publication() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"expected".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(if matches!(step, ApplyStep::Prepared) {
                ApplyHookAction::Interrupt
            } else {
                ApplyHookAction::Continue
            })
        });
        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("preparation must be interruptible");
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        fs::write(bundle.join("staged/memory.md"), b"corrupt!")?;

        let error = apply_import_plan(temp.path(), &plan, &sources)
            .expect_err("corrupt staged bytes must fail before publication");
        assert!(
            error.to_string().contains("digest verification"),
            "{error:#}"
        );
        assert!(!temp.path().join("memory/sage/memory.md").exists());
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_target_race_rolls_back_only_this_bundle_creations() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let target = temp.path().join("memory/sage/notes-second.md");
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if matches!(step, ApplyStep::BeforePublish(name) if name == "notes-second.md") {
                fs::write(&target, b"racing writer")?;
            }
            Ok(ApplyHookAction::Continue)
        });

        let error = apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("a divergent target race must abort the batch");
        assert!(
            error.to_string().contains("changed immediately"),
            "{error:#}"
        );
        assert!(!temp.path().join("memory/sage/memory.md").exists());
        assert_eq!(fs::read(target)?, b"racing writer");
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        assert!(
            fs::read_to_string(bundle.join(JOURNAL_FILE))?.contains("\"state\":\"rolled_back\"")
        );
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_atomic_publication_conflict_uses_not_published_rollback() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let conflict = temp.path().join("memory/sage/notes-second.md");
        let mut raced = false;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if !raced
                && matches!(step, ApplyStep::BeforeAtomicPublish(name) if name == "notes-second.md")
            {
                raced = true;
                fs::write(&conflict, b"atomic conflict")?;
            }
            Ok(ApplyHookAction::Continue)
        });

        let error = apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("an atomic no-replace conflict must abort the batch");

        assert!(raced, "the atomic publication race hook did not run");
        assert!(!error.to_string().contains("manual recovery"), "{error:#}");
        assert!(!temp.path().join("memory/sage/memory.md").exists());
        assert_eq!(fs::read(conflict)?, b"atomic conflict");
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        let journal = fs::read_to_string(bundle.join(JOURNAL_FILE))?;
        assert!(journal.contains("\"outcome\":\"not_published\""));
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_rollback_refuses_an_edited_published_target() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let first_target = temp.path().join("memory/sage/memory.md");
        let second_target = temp.path().join("memory/sage/notes-second.md");
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if matches!(step, ApplyStep::BeforePublish(name) if name == "notes-second.md") {
                fs::write(&first_target, b"user edit")?;
                fs::write(&second_target, b"conflict")?;
            }
            Ok(ApplyHookAction::Continue)
        });

        let error = apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("rollback must fail closed around an edited target");
        assert!(error.to_string().contains("manual recovery"), "{error:#}");
        assert_eq!(fs::read(&first_target)?, b"user edit");
        assert_eq!(fs::read(second_target)?, b"conflict");
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        assert!(fs::read_to_string(bundle.join(JOURNAL_FILE))?
            .contains("\"outcome\":\"manual_recovery\""));

        let same_bundle = apply_import_plan(temp.path(), &plan, &sources)
            .expect_err("a terminal manual-recovery bundle must remain terminal");
        assert!(
            same_bundle.to_string().contains("manual recovery"),
            "{same_bundle:#}"
        );

        let changed_sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"changed source".to_vec(),
        }];
        let changed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &changed_sources,
        )?;
        let changed = apply_import_plan(temp.path(), &changed_plan, &changed_sources)
            .expect_err("an older manual-recovery bundle must block a new bundle");
        assert!(
            changed.to_string().contains("manual recovery"),
            "{changed:#}"
        );
        assert_eq!(fs::read(first_target)?, b"user edit");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_verified_rerun_is_idempotent_and_reports_unchanged() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"same".to_vec(),
        }];
        let first_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &first_plan, &sources)?;
        let target_metadata_before = fs::metadata(temp.path().join("memory/sage/memory.md"))?;

        let second_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        assert_eq!(second_plan.entries[0].status, PlanEntryStatus::Unchanged);
        let report = apply_import_plan(temp.path(), &second_plan, &sources)?;

        assert_eq!(report.status, ImportPlanStatus::Verified);
        assert_eq!(report.create_count, 0);
        assert_eq!(report.unchanged_count, 1);
        let target_metadata_after = fs::metadata(temp.path().join("memory/sage/memory.md"))?;
        assert!(opened_metadata_stable_std(
            &target_metadata_before,
            &target_metadata_after
        ));
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn restore_logically_hides_only_verified_created_targets() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"created".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"preexisting".to_vec(),
            },
        ];
        let existing = temp.path().join("memory/sage/notes-second.md");
        fs::create_dir_all(existing.parent().expect("target has parent"))?;
        fs::write(&existing, b"preexisting")?;
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let applied = apply_import_plan(temp.path(), &plan, &sources)?;
        assert_eq!(applied.create_count, 1);
        assert_eq!(applied.unchanged_count, 1);

        let restored = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;

        assert_eq!(restored.status, ImportPlanStatus::Restored);
        assert_eq!(restored.restored_count, 1);
        assert_eq!(restored.unchanged_count, 1);
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"created"
        );
        assert_eq!(fs::read(existing)?, b"preexisting");
        let visible = crate::cockpit_sources::scan_memory(temp.path())?;
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].path, "sage/notes-second.md");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn partial_restore_retry_keeps_suppressed_targets_hidden() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"created".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        let conflict = temp.path().join("memory/sage/notes-second.md");
        fs::write(&conflict, b"user edit")?;

        let first = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;
        let second = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;
        let visible = crate::cockpit_sources::scan_memory(temp.path())?;

        assert_eq!(first.status, ImportPlanStatus::ManualRecovery);
        assert_eq!(first.restored_count, 1);
        assert_eq!(first.conflict_count, 1);
        assert_eq!(second.status, ImportPlanStatus::ManualRecovery);
        assert_eq!(second.restored_count, 1);
        assert_eq!(second.conflict_count, 1);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].path, "sage/notes-second.md");
        assert_eq!(fs::read(conflict)?, b"user edit");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn restore_retains_a_same_byte_replacement() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"same bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        let target = temp.path().join("memory/sage/memory.md");
        fs::remove_file(&target)?;
        fs::write(&target, b"same bytes")?;

        let restored = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;

        assert_eq!(restored.status, ImportPlanStatus::ManualRecovery);
        assert_eq!(restored.restored_count, 0);
        assert_eq!(restored.conflict_count, 1);
        assert_eq!(fs::read(target)?, b"same bytes");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn invalidated_restore_cannot_reactivate_a_same_byte_replacement() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"same bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;
        let target = temp.path().join("memory/sage/memory.md");
        fs::remove_file(&target)?;
        fs::write(&target, b"same bytes")?;
        let invalidated = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;
        assert_eq!(invalidated.status, ImportPlanStatus::ManualRecovery);
        let reactivation_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;

        let error = apply_import_plan(temp.path(), &reactivation_plan, &sources)
            .expect_err("invalidated restore provenance must block reactivation");

        assert!(error.to_string().contains("manual recovery"), "{error:#}");
        assert_eq!(fs::read(target)?, b"same bytes");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn reactivation_revalidates_restore_provenance_before_transition() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"same bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;
        let target = temp.path().join("memory/sage/memory.md");
        fs::remove_file(&target)?;
        fs::write(&target, b"same bytes")?;
        let reactivation_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;

        let error = apply_import_plan(temp.path(), &reactivation_plan, &sources)
            .expect_err("reactivation must revalidate retained restore evidence");

        assert!(error.to_string().contains("manual recovery"), "{error:#}");
        let restored = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;
        assert_eq!(restored.status, ImportPlanStatus::ManualRecovery);
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn restore_resumes_after_marker_before_journal_append() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"remove once".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        let mut interrupted = false;
        let mut hook = TestRestoreHook::new(|step: &RestoreStep| {
            if !interrupted
                && matches!(
                    step,
                    RestoreStep::AfterVerifyBeforeJournal(name) if name == "memory.md"
                )
            {
                interrupted = true;
                return Ok(RestoreHookAction::Interrupt);
            }
            Ok(RestoreHookAction::Continue)
        });

        restore_import_bundle_with_hook(temp.path(), "sage", &plan.bundle_id, &mut hook)
            .expect_err("restore must interrupt after removal");
        assert!(interrupted);
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"remove once"
        );

        let restored = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;

        assert_eq!(restored.status, ImportPlanStatus::Restored);
        assert_eq!(restored.restored_count, 1);
        assert_eq!(restored.conflict_count, 0);
        assert_eq!(crate::cockpit_sources::scan_memory(temp.path())?.len(), 0);
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn restore_preserves_a_replacement_raced_before_marker() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"same bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        let target = temp.path().join("memory/sage/memory.md");
        let mut raced = false;
        let mut hook = TestRestoreHook::new(|step: &RestoreStep| {
            if !raced && matches!(step, RestoreStep::BeforeMarker(name) if name == "memory.md") {
                raced = true;
                fs::remove_file(&target)?;
                fs::write(&target, b"same bytes")?;
            }
            Ok(RestoreHookAction::Continue)
        });

        let restored =
            restore_import_bundle_with_hook(temp.path(), "sage", &plan.bundle_id, &mut hook)?;

        assert!(raced);
        assert_eq!(restored.status, ImportPlanStatus::ManualRecovery);
        assert_eq!(restored.conflict_count, 1);
        assert_eq!(fs::read(target)?, b"same bytes");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn restore_reports_manual_recovery_when_target_disappears_before_marker() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"remove race".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        let target = temp.path().join("memory/sage/memory.md");
        let mut raced = false;
        let mut hook = TestRestoreHook::new(|step: &RestoreStep| {
            if !raced && matches!(step, RestoreStep::BeforeMarker(name) if name == "memory.md") {
                raced = true;
                fs::remove_file(&target)?;
            }
            Ok(RestoreHookAction::Continue)
        });

        let restored =
            restore_import_bundle_with_hook(temp.path(), "sage", &plan.bundle_id, &mut hook)?;

        assert!(raced);
        assert_eq!(restored.status, ImportPlanStatus::ManualRecovery);
        assert_eq!(restored.conflict_count, 1);
        assert!(!target.exists());
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn restore_reports_manual_recovery_when_target_is_edited_after_marker() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"original".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        let target = temp.path().join("memory/sage/memory.md");
        let mut raced = false;
        let mut hook = TestRestoreHook::new(|step: &RestoreStep| {
            if !raced
                && matches!(
                    step,
                    RestoreStep::AfterMarkerBeforeVerify(name) if name == "memory.md"
                )
            {
                raced = true;
                fs::write(&target, b"replacement")?;
            }
            Ok(RestoreHookAction::Continue)
        });

        let restored =
            restore_import_bundle_with_hook(temp.path(), "sage", &plan.bundle_id, &mut hook)?;

        assert!(raced);
        assert_eq!(restored.status, ImportPlanStatus::ManualRecovery);
        assert_eq!(restored.conflict_count, 1);
        assert_eq!(fs::read(target)?, b"replacement");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn restore_revalidates_candidate_at_the_prejournal_boundary() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"candidate race".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        let candidate = temp
            .path()
            .join("memory/sage")
            .join(TARGET_WORK_DIRECTORY)
            .join(bundle_component(&plan.bundle_id)?)
            .join("memory.md.candidate");
        let mut raced = false;
        let mut hook = TestRestoreHook::new(|step: &RestoreStep| {
            if !raced
                && matches!(
                    step,
                    RestoreStep::AfterVerifyBeforeJournal(name) if name == "memory.md"
                )
            {
                raced = true;
                fs::remove_file(&candidate)?;
            }
            Ok(RestoreHookAction::Continue)
        });

        let restored =
            restore_import_bundle_with_hook(temp.path(), "sage", &plan.bundle_id, &mut hook)?;

        assert!(raced);
        assert_eq!(restored.status, ImportPlanStatus::ManualRecovery);
        assert_eq!(restored.conflict_count, 1);
        let visible = crate::cockpit_sources::scan_memory(temp.path())?;
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].path, "sage/memory.md");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn restore_reports_and_retains_an_edited_target() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"original".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        let target = temp.path().join("memory/sage/memory.md");
        fs::write(&target, b"user edit")?;

        let restored = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;

        assert_eq!(restored.status, ImportPlanStatus::ManualRecovery);
        assert_eq!(restored.conflict_count, 1);
        assert_eq!(restored.entries[0].status, PlanEntryStatus::Conflict);
        assert_eq!(fs::read(target)?, b"user edit");
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn restore_reports_and_retains_missing_symlinked_and_nonregular_targets() -> Result<()> {
        use std::os::unix::fs::symlink;

        for case in ["missing", "symlink", "directory"] {
            let temp = trusted_tempdir()?;
            let workspace = temp.path().join("workspace");
            write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
            let sources = vec![DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"original".to_vec(),
            }];
            let plan = build_import_plan(
                temp.path(),
                "sage",
                MemoryImportSourceKind::Native,
                &sources,
            )?;
            apply_import_plan(temp.path(), &plan, &sources)?;
            let target = temp.path().join("memory/sage/memory.md");
            fs::remove_file(&target)?;
            match case {
                "missing" => {}
                "symlink" => {
                    let external = temp.path().join("external");
                    fs::write(&external, b"external")?;
                    symlink(&external, &target)?;
                }
                "directory" => {
                    fs::create_dir(&target)?;
                    fs::write(target.join("sentinel"), b"directory")?;
                }
                _ => unreachable!(),
            }

            let restored = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;

            assert_eq!(restored.status, ImportPlanStatus::ManualRecovery);
            assert_eq!(restored.conflict_count, 1);
            match case {
                "missing" => assert!(!target.exists()),
                "symlink" => assert!(fs::symlink_metadata(&target)?.file_type().is_symlink()),
                "directory" => {
                    assert_eq!(fs::read(target.join("sentinel"))?, b"directory");
                }
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn restore_is_idempotent() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"restore once".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;

        let first = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;
        let second = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;

        assert_eq!(first.status, ImportPlanStatus::Restored);
        assert_eq!(second.status, ImportPlanStatus::Restored);
        assert_eq!(second.restored_count, 1);
        assert_eq!(
            fs::read(temp.path().join("memory/sage/memory.md"))?,
            b"restore once"
        );
        assert_eq!(crate::cockpit_sources::scan_memory(temp.path())?.len(), 0);
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn restored_bundle_can_be_reactivated_and_restored_again() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"toggle".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;
        assert!(crate::cockpit_sources::scan_memory(temp.path())?.is_empty());
        let reactivation_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;

        let reactivated = apply_import_plan(temp.path(), &reactivation_plan, &sources)?;

        assert_eq!(reactivated.status, ImportPlanStatus::Verified);
        assert_eq!(reactivated.create_count, 0);
        assert_eq!(reactivated.unchanged_count, 1);
        assert_eq!(crate::cockpit_sources::scan_memory(temp.path())?.len(), 1);

        let restored = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;

        assert_eq!(restored.status, ImportPlanStatus::Restored);
        assert_eq!(restored.restored_count, 1);
        assert!(crate::cockpit_sources::scan_memory(temp.path())?.is_empty());
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn reactivation_resumes_after_bundle_transition() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"resume toggle".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;
        {
            let migration = open_locked_migration_familiar(temp.path(), "sage")?;
            let bundle = open_existing_bundle(&migration, &plan.bundle_id)?;
            let mut journal = open_or_create_journal(&bundle)?;
            append_journal(
                &bundle,
                &mut journal,
                JournalScope::Bundle,
                JournalState::Reactivating,
                None,
                None,
            )?;
        }
        let reactivation_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;

        let reactivated = apply_import_plan(temp.path(), &reactivation_plan, &sources)?;

        assert_eq!(reactivated.status, ImportPlanStatus::Verified);
        assert_eq!(reactivated.unchanged_count, 1);
        assert_eq!(crate::cockpit_sources::scan_memory(temp.path())?.len(), 1);
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn resumed_reactivation_invalidates_changed_restore_provenance() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"resume replacement".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;
        {
            let migration = open_locked_migration_familiar(temp.path(), "sage")?;
            let bundle = open_existing_bundle(&migration, &plan.bundle_id)?;
            let mut journal = open_or_create_journal(&bundle)?;
            append_journal(
                &bundle,
                &mut journal,
                JournalScope::Bundle,
                JournalState::Reactivating,
                None,
                None,
            )?;
        }
        let target = temp.path().join("memory/sage/memory.md");
        fs::remove_file(&target)?;
        fs::write(&target, b"resume replacement")?;
        let reactivation_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;

        let error = apply_import_plan(temp.path(), &reactivation_plan, &sources)
            .expect_err("resumed reactivation must revalidate restore provenance");

        assert!(error.to_string().contains("manual recovery"), "{error:#}");
        assert_eq!(fs::read(target)?, b"resume replacement");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn restored_report_revalidates_marker_digest() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"created".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        let target = temp.path().join("memory/sage/memory.md");
        let first = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;
        assert_eq!(first.status, ImportPlanStatus::Restored);
        fs::write(&target, b"replacement")?;

        let second = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;

        assert_eq!(second.status, ImportPlanStatus::ManualRecovery);
        assert_eq!(second.restored_count, 0);
        assert_eq!(second.conflict_count, 1);
        assert_eq!(fs::read(target)?, b"replacement");
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn canonical_reader_requires_target_bound_restore_provenance() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"same bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;
        let restored = fs::File::open(temp.path().join("memory/sage/memory.md"))?;
        let mut marker = vec![0_u8; 1024];
        let marker_len = rustix::fs::fgetxattr(&restored, RESTORED_XATTR, &mut marker)?;
        marker.truncate(marker_len);
        let unrelated_path = temp.path().join("memory/sage/unrelated.md");
        fs::write(&unrelated_path, b"same bytes")?;
        let unrelated = fs::File::open(&unrelated_path)?;
        rustix::fs::fsetxattr(
            &unrelated,
            RESTORED_XATTR,
            &marker,
            rustix::fs::XattrFlags::CREATE,
        )?;

        let visible = crate::cockpit_sources::scan_memory(temp.path())?;

        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].path, "sage/unrelated.md");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn restore_is_isolated_to_one_familiar() -> Result<()> {
        let temp = trusted_tempdir()?;
        let sage_workspace = temp.path().join("sage-workspace");
        let cody_workspace = temp.path().join("cody-workspace");
        write_registered_familiars(
            temp.path(),
            &[("sage", &sage_workspace), ("cody", &cody_workspace)],
        )?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"sage".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        let cody = temp.path().join("memory/cody/sentinel.md");
        fs::create_dir_all(cody.parent().expect("sentinel has parent"))?;
        fs::write(&cody, b"cody")?;

        restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;

        assert_eq!(fs::read(cody)?, b"cody");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn restore_resumes_after_interruption_after_marker_sync() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"restore bytes".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        apply_import_plan(temp.path(), &plan, &sources)?;
        let target = temp.path().join("memory/sage/memory.md");
        let mut interrupted = false;
        let mut hook = TestRestoreHook::new(|step: &RestoreStep| {
            if !interrupted
                && matches!(
                    step,
                    RestoreStep::AfterMarkerBeforeVerify(name) if name == "memory.md"
                )
            {
                interrupted = true;
                return Ok(RestoreHookAction::Interrupt);
            }
            Ok(RestoreHookAction::Continue)
        });

        restore_import_bundle_with_hook(temp.path(), "sage", &plan.bundle_id, &mut hook)
            .expect_err("restore must interrupt after marker sync");
        assert!(interrupted);
        assert_eq!(fs::read(&target)?, b"restore bytes");

        let restored = restore_import_bundle(temp.path(), "sage", &plan.bundle_id)?;

        assert_eq!(restored.status, ImportPlanStatus::Restored);
        assert_eq!(restored.restored_count, 1);
        assert_eq!(restored.conflict_count, 0);
        assert_eq!(crate::cockpit_sources::scan_memory(temp.path())?.len(), 0);
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_requires_manual_recovery_for_an_interrupted_destructive_rollback() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let conflict = temp.path().join("memory/sage/notes-second.md");
        let mut rollback_interrupted = false;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if matches!(step, ApplyStep::BeforePublish(name) if name == "notes-second.md") {
                fs::write(&conflict, b"conflict")?;
            }
            if !rollback_interrupted && matches!(step, ApplyStep::DuringRollback(_)) {
                rollback_interrupted = true;
                return Ok(ApplyHookAction::Interrupt);
            }
            Ok(ApplyHookAction::Continue)
        });

        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("the first rollback attempt must be interrupted");
        assert!(temp.path().join("memory/sage/memory.md").exists());

        let resumed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let resumed = apply_import_plan(temp.path(), &resumed_plan, &sources)
            .expect_err("a restarted destructive rollback must not trust disk provenance");
        assert!(
            resumed.to_string().contains("manual recovery"),
            "{resumed:#}"
        );
        assert!(temp.path().join("memory/sage/memory.md").exists());
        assert_eq!(fs::read(conflict)?, b"conflict");

        let terminal = apply_import_plan(temp.path(), &resumed_plan, &sources)
            .expect_err("a manual-recovery bundle must remain terminal");
        assert!(
            terminal.to_string().contains("manual recovery"),
            "{terminal:#}"
        );
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_restores_a_quarantine_interrupted_before_directory_sync() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let first = temp.path().join("memory/sage/memory.md");
        let conflict = temp.path().join("memory/sage/notes-second.md");
        let mut interrupted = false;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if matches!(step, ApplyStep::BeforePublish(name) if name == "notes-second.md") {
                fs::write(&conflict, b"conflict")?;
            }
            if !interrupted
                && matches!(
                    step,
                    ApplyStep::AfterRollbackQuarantineBeforeSync(name) if name == "memory.md"
                )
            {
                interrupted = true;
                return Ok(ApplyHookAction::Interrupt);
            }
            Ok(ApplyHookAction::Continue)
        });

        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("rollback must interrupt after the quarantine rename");
        assert!(interrupted, "the quarantine interruption hook did not run");
        assert!(!first.exists());

        let resumed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let error = apply_import_plan(temp.path(), &resumed_plan, &sources)
            .expect_err("restarted rollback must require manual recovery");

        assert!(error.to_string().contains("manual recovery"), "{error:#}");
        assert_eq!(fs::read(first)?, b"first");
        assert_eq!(fs::read(conflict)?, b"conflict");
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_marks_an_interrupted_destructive_rollback_for_manual_recovery() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let conflict = temp.path().join("memory/sage/notes-second.md");
        let mut interrupted = false;
        {
            let mut hook = TestApplyHook::new(|step: &ApplyStep| {
                if matches!(step, ApplyStep::BeforePublish(name) if name == "notes-second.md") {
                    fs::write(&conflict, b"conflict")?;
                }
                if !interrupted && matches!(step, ApplyStep::AfterRollbackRemoveBeforeJournal(_)) {
                    interrupted = true;
                    return Ok(ApplyHookAction::Interrupt);
                }
                Ok(ApplyHookAction::Continue)
            });

            apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
                .expect_err("rollback must interrupt after removing its first target");
        }
        assert!(interrupted, "the rollback interruption hook did not run");
        assert!(!temp.path().join("memory/sage/memory.md").exists());

        let resumed_plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let resumed = apply_import_plan(temp.path(), &resumed_plan, &sources)
            .expect_err("a restarted rollback cannot trust an on-disk ownership claim");
        assert!(
            resumed.to_string().contains("manual recovery"),
            "{resumed:#}"
        );
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        let journal = fs::read_to_string(bundle.join(JOURNAL_FILE))?;
        assert!(journal.contains("\"state\":\"rolling_back\""));
        assert!(journal.contains("\"outcome\":\"manual_recovery\""));
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_rollback_marks_a_missing_published_target_for_manual_recovery() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![
            DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"first".to_vec(),
            },
            DiscoveredSource {
                source_label: "notes/second.md".to_owned(),
                bytes: b"second".to_vec(),
            },
        ];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let first = temp.path().join("memory/sage/memory.md");
        let second = temp.path().join("memory/sage/notes-second.md");
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            if matches!(step, ApplyStep::BeforePublish(name) if name == "notes-second.md") {
                fs::remove_file(&first)?;
                fs::write(&second, b"conflict")?;
            }
            Ok(ApplyHookAction::Continue)
        });

        let error = apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("a missing journaled publication needs manual recovery");
        assert!(error.to_string().contains("manual recovery"), "{error:#}");
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        assert!(fs::read_to_string(bundle.join(JOURNAL_FILE))?
            .contains("\"outcome\":\"manual_recovery\""));
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_rejects_tampered_manifest_and_symlinked_bundle_ancestors() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"secret".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(if matches!(step, ApplyStep::Prepared) {
                ApplyHookAction::Interrupt
            } else {
                ApplyHookAction::Continue
            })
        });
        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("preparation must be interruptible");
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(bundle.join(MANIFEST_FILE))?)?;
        manifest["familiar_id"] = serde_json::Value::String("other".to_owned());
        fs::write(bundle.join(MANIFEST_FILE), canonical_json_line(&manifest)?)?;
        assert!(apply_import_plan(temp.path(), &plan, &sources)
            .expect_err("a changed manifest must fail closed")
            .to_string()
            .contains("does not match"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let linked_home = trusted_tempdir()?;
            let linked_workspace = linked_home.path().join("workspace");
            write_registered_familiars(linked_home.path(), &[("sage", &linked_workspace)])?;
            let outside = linked_home.path().join("outside");
            fs::create_dir(&outside)?;
            symlink(&outside, linked_home.path().join(MIGRATIONS_DIRECTORY))?;
            let linked_plan = build_import_plan(
                linked_home.path(),
                "sage",
                MemoryImportSourceKind::Native,
                &sources,
            )?;
            let error = apply_import_plan(linked_home.path(), &linked_plan, &sources)
                .expect_err("a symlinked migration root must fail closed");
            assert!(error.to_string().contains("real directory"), "{error:#}");
            assert!(fs::read_dir(outside)?.next().is_none());
        }
        Ok(())
    }

    #[test]
    #[cfg(not(windows))]
    fn apply_rejects_journal_entries_not_bound_to_the_manifest() -> Result<()> {
        let temp = trusted_tempdir()?;
        let workspace = temp.path().join("workspace");
        write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
        let sources = vec![DiscoveredSource {
            source_label: "MEMORY.md".to_owned(),
            bytes: b"secret".to_vec(),
        }];
        let plan = build_import_plan(
            temp.path(),
            "sage",
            MemoryImportSourceKind::Native,
            &sources,
        )?;
        let mut hook = TestApplyHook::new(|step: &ApplyStep| {
            Ok(if matches!(step, ApplyStep::Prepared) {
                ApplyHookAction::Interrupt
            } else {
                ApplyHookAction::Continue
            })
        });
        apply_import_plan_with_hook(temp.path(), &plan, &sources, &mut hook)
            .expect_err("preparation must be interruptible");
        let bundle = bundle_path(temp.path(), "sage", &plan.bundle_id)?;
        let forged = JournalRecord {
            protocol_version: IMPORT_PROTOCOL_VERSION,
            sequence: 1,
            scope: JournalScope::Entry,
            state: JournalState::Prepared,
            target_name: Some("foreign.md".to_owned()),
            digest: Some(blake3_digest(b"foreign")),
            byte_length: Some(7),
            outcome: None,
        };
        let mut journal = fs::OpenOptions::new()
            .append(true)
            .open(bundle.join(JOURNAL_FILE))?;
        journal.write_all(&canonical_json_line(&forged)?)?;
        journal.sync_all()?;

        let error = apply_import_plan(temp.path(), &plan, &sources)
            .expect_err("journal entries outside the manifest must fail closed");
        assert!(error.to_string().contains("journal"), "{error:#}");
        assert!(!temp.path().join("memory/sage/memory.md").exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn plans_unsafe_target_roots_mark_every_entry_conflict_without_creation() -> Result<()> {
        use std::os::unix::fs::symlink;

        for unsafe_kind in ["memory-symlink", "familiar-symlink", "memory-file"] {
            let temp = trusted_tempdir()?;
            let workspace = temp.path().join("workspace");
            write_registered_familiars(temp.path(), &[("sage", &workspace)])?;
            let outside = temp.path().join("outside");
            fs::create_dir_all(&outside)?;
            match unsafe_kind {
                "memory-symlink" => symlink(&outside, temp.path().join("memory"))?,
                "familiar-symlink" => {
                    fs::create_dir_all(temp.path().join("memory"))?;
                    symlink(&outside, temp.path().join("memory/sage"))?;
                }
                "memory-file" => write_file(&temp.path().join("memory"), b"not a directory")?,
                _ => unreachable!(),
            }
            let sources = vec![DiscoveredSource {
                source_label: "MEMORY.md".to_owned(),
                bytes: b"secret".to_vec(),
            }];

            let plan = build_import_plan(
                temp.path(),
                "sage",
                MemoryImportSourceKind::Native,
                &sources,
            )?;

            assert_eq!(plan.status, ImportPlanStatus::Conflict);
            assert!(!plan.apply_eligible);
            assert_eq!(plan.conflict_count, 1);
            assert_eq!(plan.entries[0].status, PlanEntryStatus::Conflict);
            assert!(!outside.join("memory.md").exists());
            assert!(!temp.path().join("memory-import").exists());
        }
        Ok(())
    }

    fn assert_portable_target_name(name: &str) {
        assert!(name.is_ascii());
        assert_eq!(Path::new(name).components().count(), 1);
        assert_ne!(name, ".");
        assert_ne!(name, "..");
        assert!(name.ends_with(".md"));
        assert!(!name.ends_with(['.', ' ']));
        assert!(!name.chars().any(char::is_control));
        assert!(!name.contains(['/', '\\', ':']));
        let stem = name.trim_end_matches(".md").to_ascii_uppercase();
        assert!(
            !matches!(
                stem.as_str(),
                "CON"
                    | "PRN"
                    | "AUX"
                    | "NUL"
                    | "COM1"
                    | "COM2"
                    | "COM3"
                    | "COM4"
                    | "COM5"
                    | "COM6"
                    | "COM7"
                    | "COM8"
                    | "COM9"
                    | "LPT1"
                    | "LPT2"
                    | "LPT3"
                    | "LPT4"
                    | "LPT5"
                    | "LPT6"
                    | "LPT7"
                    | "LPT8"
                    | "LPT9"
            ),
            "Windows device name leaked into target: {name}"
        );
    }

    struct TestApplyHook<F> {
        callback: F,
    }

    impl<F> TestApplyHook<F> {
        fn new(callback: F) -> Self {
            Self { callback }
        }
    }

    impl<F> ApplyHook for TestApplyHook<F>
    where
        F: FnMut(&ApplyStep) -> Result<ApplyHookAction>,
    {
        fn step(&mut self, step: &ApplyStep) -> Result<ApplyHookAction> {
            (self.callback)(step)
        }
    }

    #[cfg(unix)]
    fn opened_metadata_stable_std(before: &fs::Metadata, after: &fs::Metadata) -> bool {
        use std::os::unix::fs::MetadataExt;

        before.dev() == after.dev() && before.ino() == after.ino()
    }

    #[cfg(not(any(unix, windows)))]
    fn opened_metadata_stable_std(before: &fs::Metadata, after: &fs::Metadata) -> bool {
        before.len() == after.len() && before.modified().ok() == after.modified().ok()
    }

    fn labels(sources: &[DiscoveredSource]) -> Vec<&str> {
        sources
            .iter()
            .map(|source| source.source_label.as_str())
            .collect()
    }

    fn trusted_tempdir() -> Result<tempfile::TempDir> {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let worktree = manifest_dir
            .parent()
            .and_then(Path::parent)
            .expect("coven-cli manifest must be inside the repository");
        let repository = worktree
            .parent()
            .filter(|parent| parent.file_name() == Some(std::ffi::OsStr::new(".worktrees")))
            .and_then(Path::parent)
            .unwrap_or(worktree);
        let test_root = repository.join("target/m");
        fs::create_dir_all(&test_root)?;
        Ok(tempfile::Builder::new().prefix("m").tempdir_in(test_root)?)
    }

    fn write_registered_familiars(coven_home: &Path, familiars: &[(&str, &Path)]) -> Result<()> {
        let mut config = String::new();
        for (id, workspace) in familiars {
            let workspace = serde_json::to_string(&workspace.to_string_lossy())?;
            config.push_str(&format!(
                r#"
[[familiar]]
id = "{id}"
display_name = "{id}"
role = "test"
description = "test"
workspace = {workspace}
"#
            ));
        }
        fs::write(coven_home.join("familiars.toml"), config)?;
        Ok(())
    }

    fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)?;
        Ok(())
    }

    fn add_excluded_traps(root: &Path, native: bool) -> Result<()> {
        for root_file in ["AGENTS.md", "USER.md", "SOUL.md", "TOOLS.md"] {
            let path = root.join(root_file);
            write_file(&path, format!("sentinel root {root_file}").as_bytes())?;
            make_unreadable(&path)?;
        }
        if native {
            write_file(&root.join("DREAMS.md"), b"native must not read dreams")?;
        } else {
            write_file(
                &root.join("notes/native.md"),
                b"OpenClaw must not read notes",
            )?;
        }
        for directory in [
            "config",
            "auth",
            "credentials",
            "sessions",
            "transcripts",
            "logs",
        ] {
            let path = root.join(directory).join("sentinel.md");
            write_file(&path, format!("sentinel {directory}").as_bytes())?;
            make_unreadable(&path)?;
        }
        write_file(&root.join("memory/.hidden.md"), b"hidden")?;
        write_file(&root.join("memory/.hidden/topic.md"), b"hidden tree")?;
        write_file(&root.join("memory/not-markdown.txt"), b"not markdown")?;
        write_file(&root.join("memory/invalid.md"), &[0xff, 0xfe])?;
        write_file(
            &root.join("memory/oversize.md"),
            &vec![b'x'; crate::cockpit_sources::MEMORY_CONTENT_MAX_BYTES as usize + 1],
        )?;

        #[cfg(unix)]
        {
            use std::ffi::OsString;
            use std::os::unix::ffi::OsStringExt;
            use std::os::unix::net::UnixListener;

            let non_utf8 = OsString::from_vec(b"non-utf8-\xff.md".to_vec());
            if let Err(error) = write_file(&root.join("memory").join(non_utf8), b"bad name") {
                if error
                    .downcast_ref::<io::Error>()
                    .and_then(io::Error::raw_os_error)
                    != Some(92)
                {
                    return Err(error);
                }
            }
            let socket_path = root.join("memory/special.md");
            let listener = UnixListener::bind(&socket_path)?;
            drop(listener);
        }
        Ok(())
    }

    fn make_unreadable(path: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(path, fs::Permissions::from_mode(0o000))?;
        }
        Ok(())
    }
}
