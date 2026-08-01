use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::ambient_authority;
#[cfg(any(unix, windows))]
use cap_std::fs::MetadataExt;
#[cfg(unix)]
use cap_std::fs::OpenOptionsExt;
use cap_std::fs::{Dir, OpenOptions};
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
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlanEntryStatus {
    Create,
    Unchanged,
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
    format!("blake3:{}", hasher.finalize().to_hex())
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

pub(crate) fn run_import(
    familiar: &str,
    source: MemoryImportSourceKind,
    openclaw_root: Option<&Path>,
    apply: bool,
    json: bool,
) -> Result<()> {
    if apply {
        bail!("coven memory import apply is not implemented yet");
    }
    let coven_home = crate::paths::coven_home_dir()?;
    let discovered = discover_sources(
        &coven_home,
        DiscoverSourcesRequest {
            familiar,
            source,
            openclaw_root,
        },
    )?;
    let plan = build_import_plan(&coven_home, familiar, source, &discovered)?;

    if json {
        println!("{}", serde_json::to_string(&plan)?);
    } else {
        println!(
            "Preview for familiar `{familiar}`: {} file(s), {} create, {} unchanged, {} conflict.",
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
                    PlanEntryStatus::Conflict => "conflict",
                }
            );
        }
        if plan.apply_eligible {
            println!("Plan is apply-eligible; no files or directories were created.");
        } else {
            println!("Plan is not apply-eligible; no files or directories were created.");
        }
    }
    Ok(())
}

pub(crate) fn run_restore(_familiar: &str, _bundle: &str, _json: bool) -> Result<()> {
    bail!("coven memory restore is not implemented yet")
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
        assert!(same.bundle_id.starts_with("blake3:"));
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
