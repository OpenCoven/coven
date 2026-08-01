use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::ambient_authority;
#[cfg(windows)]
use cap_std::fs::MetadataExt;
#[cfg(unix)]
use cap_std::fs::OpenOptionsExt;
use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};

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
#[allow(dead_code)]
pub(crate) enum MemoryImportStatus {
    Preview,
    Applied,
    Restored,
    Conflict,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub(crate) enum MemoryImportEntryStatus {
    Planned,
    Created,
    Unchanged,
    Restored,
    Conflict,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[allow(dead_code)]
pub(crate) struct MemoryImportEntry {
    pub(crate) source_label: String,
    pub(crate) target_name: String,
    pub(crate) digest: String,
    pub(crate) status: MemoryImportEntryStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[allow(dead_code)]
pub(crate) struct MemoryImportReport {
    pub(crate) familiar_id: String,
    pub(crate) source_kind: MemoryImportSourceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bundle_id: Option<String>,
    pub(crate) status: MemoryImportStatus,
    pub(crate) file_count: usize,
    pub(crate) created_count: usize,
    pub(crate) unchanged_count: usize,
    pub(crate) restored_count: usize,
    pub(crate) conflict_count: usize,
    pub(crate) entries: Vec<MemoryImportEntry>,
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

#[derive(Serialize)]
struct DiscoveryReport<'a> {
    familiar_id: &'a str,
    source_kind: MemoryImportSourceKind,
    status: &'static str,
    file_count: usize,
    entries: Vec<DiscoveryReportEntry<'a>>,
}

#[derive(Serialize)]
struct DiscoveryReportEntry<'a> {
    source_label: &'a str,
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
    let report = DiscoveryReport {
        familiar_id: familiar,
        source_kind: source,
        status: "discovered",
        file_count: discovered.len(),
        entries: discovered
            .iter()
            .map(|source| DiscoveryReportEntry {
                source_label: &source.source_label,
            })
            .collect(),
    };

    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        println!(
            "Discovered {} source file(s) for familiar `{familiar}`.",
            report.file_count
        );
        for entry in &report.entries {
            println!("- {}", entry.source_label);
        }
        println!("Discovery only; import planning and filesystem changes are not implemented.");
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
    fn memory_import_report_json_is_stable_and_redacted() {
        let report = MemoryImportReport {
            familiar_id: "sage".to_owned(),
            source_kind: MemoryImportSourceKind::Openclaw,
            bundle_id: Some("bundle-1".to_owned()),
            status: MemoryImportStatus::Preview,
            file_count: 1,
            created_count: 0,
            unchanged_count: 0,
            restored_count: 0,
            conflict_count: 0,
            entries: vec![MemoryImportEntry {
                source_label: "memory/notes.md".to_owned(),
                target_name: "openclaw-notes.md".to_owned(),
                digest: "blake3:abc123".to_owned(),
                status: MemoryImportEntryStatus::Planned,
            }],
        };

        let value = serde_json::to_value(&report).expect("report must serialize");
        assert_eq!(value["familiar_id"], "sage");
        assert_eq!(value["source_kind"], "openclaw");
        assert_eq!(value["status"], "preview");
        assert_eq!(value["entries"][0]["status"], "planned");

        let json = serde_json::to_string(&report).expect("report must serialize");
        for forbidden in ["content", "source_path", "absolute_path"] {
            assert!(
                !json.contains(forbidden),
                "serialized report leaked forbidden value {forbidden:?}: {json}"
            );
        }

        let decoded: MemoryImportReport =
            serde_json::from_str(&json).expect("report must deserialize");
        assert_eq!(decoded, report);
    }

    #[test]
    fn memory_import_report_json_omits_absent_bundle_id_without_path_fields() {
        let report = MemoryImportReport {
            familiar_id: "sage".to_owned(),
            source_kind: MemoryImportSourceKind::Native,
            bundle_id: None,
            status: MemoryImportStatus::Preview,
            file_count: 0,
            created_count: 0,
            unchanged_count: 0,
            restored_count: 0,
            conflict_count: 0,
            entries: Vec::new(),
        };

        let value = serde_json::to_value(report).expect("report must serialize");
        let object = value
            .as_object()
            .expect("report must serialize as an object");
        assert!(!object.contains_key("bundle_id"));
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
