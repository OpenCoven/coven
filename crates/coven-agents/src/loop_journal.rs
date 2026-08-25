use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use async_trait::async_trait;
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{loop_runner::validate_loop_id, BoxError, LoopCheckpoint, LoopJournal};

const JOURNAL_SCHEMA_VERSION: u32 = 1;
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct FileLoopJournal {
    root: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct DurableCheckpoint {
    schema_version: u32,
    checkpoint: LoopCheckpoint,
}

impl FileLoopJournal {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, io::Error> {
        let root = root.into();
        create_directory_durable(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn loop_directory(&self, loop_id: &str) -> PathBuf {
        let encoded = loop_id
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.root.join(encoded)
    }

    fn open_lock(directory: &Path) -> Result<File, io::Error> {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join(".lock"))
    }

    fn latest_checkpoint(directory: &Path) -> Result<Option<(u64, LoopCheckpoint)>, BoxError> {
        let mut latest = None;
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
            let bytes = fs::read(entry.path())?;
            let durable: DurableCheckpoint = serde_json::from_slice(&bytes)?;
            if durable.schema_version != JOURNAL_SCHEMA_VERSION {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "unsupported loop journal schema version {}",
                        durable.schema_version
                    ),
                )));
            }
            latest = Some((sequence, durable.checkpoint));
        }
        Ok(latest)
    }

    fn persist(
        directory: &Path,
        sequence: u64,
        checkpoint: &LoopCheckpoint,
    ) -> Result<(), BoxError> {
        let durable = DurableCheckpoint {
            schema_version: JOURNAL_SCHEMA_VERSION,
            checkpoint: checkpoint.clone(),
        };
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

    fn validate_key(loop_id: &str) -> Result<(), BoxError> {
        validate_loop_id(loop_id).map_err(|reason| {
            Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("loop id {reason}"),
            )) as BoxError
        })
    }
}

#[async_trait]
impl LoopJournal for FileLoopJournal {
    async fn load(&self, loop_id: &str) -> Result<Option<LoopCheckpoint>, BoxError> {
        Self::validate_key(loop_id)?;
        let directory = self.loop_directory(loop_id);
        if !directory.exists() {
            return Ok(None);
        }
        let lock = Self::open_lock(&directory)?;
        FileExt::lock_shared(&lock)?;
        let checkpoint = Self::latest_checkpoint(&directory)?.map(|(_, checkpoint)| checkpoint);
        FileExt::unlock(&lock)?;
        Ok(checkpoint)
    }

    async fn list(&self) -> Result<Vec<LoopCheckpoint>, BoxError> {
        let mut checkpoints = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let lock = Self::open_lock(&entry.path())?;
            FileExt::lock_shared(&lock)?;
            if let Some((_, checkpoint)) = Self::latest_checkpoint(&entry.path())? {
                checkpoints.push(checkpoint);
            }
            FileExt::unlock(&lock)?;
        }
        checkpoints.sort_by(|left, right| left.loop_id.cmp(&right.loop_id));
        Ok(checkpoints)
    }

    async fn compare_and_set(
        &self,
        loop_id: &str,
        expected: Option<&LoopCheckpoint>,
        next: &LoopCheckpoint,
    ) -> Result<bool, BoxError> {
        Self::validate_key(loop_id)?;
        if next.loop_id != loop_id {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "checkpoint loop id does not match journal key",
            )));
        }
        let directory = self.loop_directory(loop_id);
        create_directory_durable(&directory)?;
        let lock = Self::open_lock(&directory)?;
        FileExt::lock_exclusive(&lock)?;
        Self::remove_stale_staging_files(&directory)?;
        let latest = Self::latest_checkpoint(&directory)?;
        if latest.as_ref().map(|(_, checkpoint)| checkpoint) != expected {
            FileExt::unlock(&lock)?;
            return Ok(false);
        }
        let sequence = latest.map_or(1, |(sequence, _)| sequence + 1);
        Self::persist(&directory, sequence, next)?;
        FileExt::unlock(&lock)?;
        Ok(true)
    }
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

fn create_directory_durable(directory: &Path) -> Result<(), io::Error> {
    if directory.exists() {
        return if directory.is_dir() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "loop journal path exists and is not a directory",
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
                "loop journal path has no existing ancestor",
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
