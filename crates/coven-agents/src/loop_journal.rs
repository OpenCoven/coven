use std::{io, path::Path, path::PathBuf};

use async_trait::async_trait;

use crate::{
    durable_store::{DurableStore, StoreFormat},
    loop_runner::validate_loop_id,
    BoxError, LoopCheckpoint, LoopJournal,
};

const FORMAT: StoreFormat = StoreFormat {
    label: "loop journal",
    schema_version: 1,
    record_field: "checkpoint",
};

#[derive(Debug, Clone)]
pub struct FileLoopJournal {
    store: DurableStore,
}

impl FileLoopJournal {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, io::Error> {
        Ok(Self {
            store: DurableStore::new(root, FORMAT)?,
        })
    }

    pub fn root(&self) -> &Path {
        self.store.root()
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
        self.store.load(loop_id)
    }

    async fn list(&self) -> Result<Vec<LoopCheckpoint>, BoxError> {
        let mut checkpoints: Vec<LoopCheckpoint> = self.store.list()?;
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
        self.store.compare_and_set(loop_id, expected, next)
    }
}
