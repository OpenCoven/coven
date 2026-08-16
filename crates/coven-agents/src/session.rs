use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;

use crate::{BoxError, RunItem};

#[async_trait]
/// Conversation history storage for completed runs.
///
/// The runner performs separate load and append operations. Implementations
/// must enforce a single writer per session id or provide their own optimistic
/// concurrency control when concurrent runs can target the same session.
pub trait SessionStore: Send + Sync {
    async fn load(&self, session_id: &str) -> Result<Vec<RunItem>, BoxError>;

    async fn append(&self, session_id: &str, items: &[RunItem]) -> Result<(), BoxError>;
}

#[derive(Debug, Clone, Default)]
pub struct InMemorySession {
    items: Arc<Mutex<BTreeMap<String, Vec<RunItem>>>>,
}

impl InMemorySession {
    pub async fn items(&self, session_id: &str) -> Result<Vec<RunItem>, BoxError> {
        self.load(session_id).await
    }
}

#[async_trait]
impl SessionStore for InMemorySession {
    async fn load(&self, session_id: &str) -> Result<Vec<RunItem>, BoxError> {
        let sessions = self.items.lock().map_err(|_| {
            Box::new(io::Error::other("in-memory session lock poisoned")) as BoxError
        })?;
        Ok(sessions.get(session_id).cloned().unwrap_or_default())
    }

    async fn append(&self, session_id: &str, items: &[RunItem]) -> Result<(), BoxError> {
        let mut sessions = self.items.lock().map_err(|_| {
            Box::new(io::Error::other("in-memory session lock poisoned")) as BoxError
        })?;
        sessions
            .entry(session_id.to_owned())
            .or_default()
            .extend_from_slice(items);
        Ok(())
    }
}
