//! TurboVec IdMapIndex wrapper — persistent 4-bit compressed ANN index with stable ids

use crate::{embed::DIM, fs_security};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use turbovec::IdMapIndex;

pub const BIT_WIDTH: usize = 4;

pub struct VecIndex {
    inner: IdMapIndex,
    path: PathBuf,
}

impl VecIndex {
    /// Load from disk if it exists, otherwise create a fresh index
    pub fn open(path: &Path) -> Result<Self> {
        fs_security::ensure_private_parent(path)?;
        fs_security::validate_existing_private_file(path)?;
        let inner = if path.exists() {
            IdMapIndex::load(path)
                .with_context(|| format!("loading index from {}", path.display()))?
        } else {
            IdMapIndex::new(DIM, BIT_WIDTH)
                .map_err(|e| anyhow::anyhow!("creating index: {:?}", e))?
        };
        Ok(Self {
            inner,
            path: path.to_owned(),
        })
    }

    /// Add a batch of (id, embedding) pairs (flat row-major: each embedding is DIM f32s)
    pub fn add(&mut self, ids: &[u64], vectors: &[Vec<f32>]) -> Result<()> {
        let flat: Vec<f32> = vectors.iter().flat_map(|v| v.iter().cloned()).collect();
        self.inner
            .add_with_ids(&flat, ids)
            .map_err(|e| anyhow::anyhow!("adding to turbovec: {:?}", e))?;
        Ok(())
    }

    /// Search — returns (scores, ids) ordered by score descending
    pub fn search(&self, query: &[f32], k: usize) -> Result<(Vec<f32>, Vec<u64>)> {
        let (scores, ids) = self.inner.search(query, k);
        Ok((scores, ids))
    }

    /// Search restricted to an allowlist of ids (Ward / familiar scoping)
    pub fn search_filtered(
        &self,
        query: &[f32],
        k: usize,
        allowlist: &[u64],
    ) -> Result<(Vec<f32>, Vec<u64>)> {
        let (scores, ids) = self.inner.search_with_allowlist(query, k, Some(allowlist));
        Ok((scores, ids))
    }

    /// Remove a doc by id (O(1) in turbovec)
    pub fn remove(&mut self, id: u64) {
        self.inner.remove(id);
    }

    /// Persist to disk
    pub fn save(&self) -> Result<()> {
        self.inner
            .write(&self.path)
            .with_context(|| format!("saving index to {}", self.path.display()))?;
        fs_security::set_private_file(&self.path)?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.len() == 0
    }
}
