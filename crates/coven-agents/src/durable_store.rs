//! Crash-safe, compare-and-set record storage shared by the file-backed
//! journals.
//!
//! Each key gets a directory named by the hex encoding of its bytes. Every
//! write is a new `checkpoint-<sequence>.json` file, staged, synced, renamed
//! into place and followed by a directory sync, under an exclusive advisory
//! lock. Readers take a shared lock and use the highest sequence. A crash
//! leaves either the previous checkpoint or the new one, never a torn file;
//! staging leftovers are removed before the next write.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use fs2::FileExt;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};

use crate::BoxError;

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// What a store calls itself in errors, the schema version it writes, and
/// the field each checkpoint file stores its record under.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StoreFormat {
    pub(crate) label: &'static str,
    pub(crate) schema_version: u32,
    pub(crate) record_field: &'static str,
}

#[derive(Debug, Clone)]
pub(crate) struct DurableStore {
    root: PathBuf,
    format: StoreFormat,
}

impl DurableStore {
    pub(crate) fn new(root: impl Into<PathBuf>, format: StoreFormat) -> Result<Self, io::Error> {
        let root = root.into();
        create_directory_durable(&root, format.label)?;
        Ok(Self { root, format })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn load<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, BoxError> {
        let directory = self.key_directory(key);
        if !directory.exists() {
            return Ok(None);
        }
        let lock = open_lock(&directory)?;
        FileExt::lock_shared(&lock)?;
        let record = self
            .latest(&directory)
            .map(|latest| latest.map(|(_, record)| record));
        FileExt::unlock(&lock)?;
        record
    }

    pub(crate) fn list<T: DeserializeOwned>(&self) -> Result<Vec<T>, BoxError> {
        let mut records = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let lock = open_lock(&entry.path())?;
            FileExt::lock_shared(&lock)?;
            let latest = self.latest(&entry.path());
            FileExt::unlock(&lock)?;
            if let Some((_, record)) = latest? {
                records.push(record);
            }
        }
        Ok(records)
    }

    /// Atomically replaces `expected` with `next`. Returns `false`, writing
    /// nothing, when the stored record is not `expected`.
    pub(crate) fn compare_and_set<T>(
        &self,
        key: &str,
        expected: Option<&T>,
        next: &T,
    ) -> Result<bool, BoxError>
    where
        T: Serialize + DeserializeOwned + PartialEq,
    {
        let directory = self.key_directory(key);
        create_directory_durable(&directory, self.format.label)?;
        let lock = open_lock(&directory)?;
        FileExt::lock_exclusive(&lock)?;
        let written = (|| -> Result<bool, BoxError> {
            remove_stale_staging_files(&directory)?;
            let latest = self.latest::<T>(&directory)?;
            if latest.as_ref().map(|(_, record)| record) != expected {
                return Ok(false);
            }
            let sequence = latest.map_or(1, |(sequence, _)| sequence + 1);
            self.persist(&directory, sequence, next)?;
            Ok(true)
        })();
        FileExt::unlock(&lock)?;
        written
    }

    fn key_directory(&self, key: &str) -> PathBuf {
        let encoded = key
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.root.join(encoded)
    }

    fn latest<T: DeserializeOwned>(&self, directory: &Path) -> Result<Option<(u64, T)>, BoxError> {
        let mut latest: Option<(u64, PathBuf)> = None;
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(sequence) = name
                .strip_prefix("checkpoint-")
                .and_then(|value| value.strip_suffix(".json"))
                .and_then(|value| value.parse::<u64>().ok())
            else {
                continue;
            };
            if latest
                .as_ref()
                .is_some_and(|(latest_sequence, _)| *latest_sequence >= sequence)
            {
                continue;
            }
            latest = Some((sequence, entry.path()));
        }
        let Some((sequence, path)) = latest else {
            return Ok(None);
        };
        let mut durable: Map<String, Value> = serde_json::from_slice(&fs::read(path)?)?;
        let version = durable.get("schema_version").and_then(Value::as_u64);
        if version != Some(u64::from(self.format.schema_version)) {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsupported {} schema version {}",
                    self.format.label,
                    version.map_or_else(|| "missing".to_owned(), |version| version.to_string())
                ),
            )));
        }
        let record = durable.remove(self.format.record_field).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} checkpoint has no `{}`",
                    self.format.label, self.format.record_field
                ),
            )
        })?;
        Ok(Some((sequence, serde_json::from_value(record)?)))
    }

    fn persist<T: Serialize>(
        &self,
        directory: &Path,
        sequence: u64,
        record: &T,
    ) -> Result<(), BoxError> {
        let mut durable = Map::new();
        durable.insert(
            "schema_version".to_owned(),
            Value::from(self.format.schema_version),
        );
        durable.insert(
            self.format.record_field.to_owned(),
            serde_json::to_value(record)?,
        );
        let bytes = serde_json::to_vec(&durable)?;
        let staging = directory.join(format!(
            ".checkpoint-{}-{}.tmp",
            std::process::id(),
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let final_path = directory.join(format!("checkpoint-{sequence:020}.json"));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&staging)?;
        if let Err(error) = (|| -> Result<(), io::Error> {
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&staging, &final_path)?;
            sync_directory(directory)?;
            Ok(())
        })() {
            let _ = fs::remove_file(&staging);
            return Err(Box::new(error));
        }
        Ok(())
    }
}

fn open_lock(directory: &Path) -> Result<File, io::Error> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(".lock"))
}

fn remove_stale_staging_files(directory: &Path) -> Result<(), io::Error> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(".checkpoint-")
            && name.ends_with(".tmp")
            && entry.file_type()?.is_file()
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<(), io::Error> {
    File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(directory: &Path) -> Result<(), io::Error> {
    let _ = directory;
    Ok(())
}

fn create_directory_durable(directory: &Path, label: &str) -> Result<(), io::Error> {
    if directory.exists() {
        return if directory.is_dir() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{label} path exists and is not a directory"),
            ))
        };
    }

    let mut missing = Vec::new();
    let mut cursor = directory;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{label} path has no existing ancestor"),
            )
        })?;
    }

    fs::create_dir_all(directory)?;
    for created in missing.iter().rev() {
        if let Some(parent) = created.parent() {
            sync_directory(parent)?;
        }
        sync_directory(created)?;
    }
    Ok(())
}
