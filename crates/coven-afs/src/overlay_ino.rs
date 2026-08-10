//! Inode-addressed access to a copy-on-write overlay.
//!
//! [`crate::overlay`] resolves paths; a mount backend cannot use it, for the
//! reason [`crate::ino`] gives: NFSv3 and FUSE address objects by a 64-bit file
//! id and a `(parent, name)` pair, never by path. Serving the *merged* view
//! therefore needs the overlay's delta-then-base rules expressed against inode
//! numbers, which is what this module is.
//!
//! # The identity problem
//!
//! Both layers allocate `fs_inode.ino` as an `AUTOINCREMENT` starting at
//! [`ROOT_INO`], so base inode 5 and delta inode 5 are unrelated files. A
//! union of the two id spaces collides on almost every id.
//!
//! Base inodes are therefore exposed with [`BASE_TAG`] set and delta inodes
//! exposed as they are. The tag is a high bit rather than an offset because
//! `readdir_ino` paginates with `WHERE ino > start_after`: a high bit keeps the
//! merged sequence monotonic, so a cursor still resumes exactly, walking the
//! delta's entries and then the base's.
//!
//! # Handle stability across copy-up
//!
//! A client holding a handle to a base-only file must keep it working after
//! the first write, which moves that file into the delta and gives it a new
//! delta inode. The exposed id cannot change underneath the client, so a
//! tagged base id stays the file's identity for the export's lifetime and
//! resolution redirects it to the delta inode once one exists.
//!
//! `fs_origin` records `delta_ino -> base_ino` but is keyed by `delta_ino`, so
//! the reverse lookup this needs has no index. Rather than touch a SPEC table
//! (DESIGN.md E1 forbids it), the redirect lives in memory: it is seeded from
//! `fs_origin` at open and updated on each copy-up. The export owns the only
//! writer to its delta, so the map cannot go stale beneath it.

use std::collections::HashMap;

use crate::fs::{Metadata, ROOT_INO};
use crate::ino::DirEntry;
use crate::overlay::OverlayFs;
use crate::path::normalize;
use crate::{AgentFs, Error, Result};

/// Marks an exposed id as belonging to the base layer.
///
/// Bit 62: the ids stay positive in `i64`, and no realistic delta allocates
/// enough inodes to reach it.
pub const BASE_TAG: i64 = 1 << 62;

/// Which layer an exposed id resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layer {
    Delta(i64),
    Base(i64),
}

/// An overlay presented as a single inode-addressed filesystem.
pub struct OverlayExport {
    fs: OverlayFs,
    /// `base_ino -> delta_ino` for files that have been copied up. The reverse
    /// of `fs_origin`, which is keyed the other way.
    redirect: HashMap<i64, i64>,
    /// Exposed id to absolute path.
    ///
    /// Whiteouts are keyed by path (`fs_whiteout.path`), so merging a
    /// directory or resolving a name needs its parent's path. Clients walk
    /// down from the root, so the path is known on the way in and recorded
    /// here; [`Self::path_of`] falls back to a walk for a handle this process
    /// has not seen, which is what happens after a restart.
    paths: HashMap<i64, String>,
}

impl OverlayExport {
    /// Wrap an overlay for inode-addressed access.
    pub fn new(fs: OverlayFs) -> Result<Self> {
        let redirect = fs.origin_pairs()?.into_iter().collect();
        let mut paths = HashMap::new();
        paths.insert(ROOT_INO, "/".to_string());
        Ok(Self {
            fs,
            redirect,
            paths,
        })
    }

    /// The overlay's root. The delta's root is the merged root: a delta always
    /// exists, and the base's root is reached through it rather than beside it.
    pub fn root(&self) -> i64 {
        ROOT_INO
    }

    /// The underlying overlay.
    pub fn overlay(&self) -> &OverlayFs {
        &self.fs
    }

    /// The underlying overlay, mutably.
    pub fn overlay_mut(&mut self) -> &mut OverlayFs {
        &mut self.fs
    }

    fn resolve(&self, id: i64) -> Layer {
        if id & BASE_TAG == 0 {
            return Layer::Delta(id);
        }
        let base = id & !BASE_TAG;
        match self.redirect.get(&base) {
            Some(delta) => Layer::Delta(*delta),
            None => Layer::Base(base),
        }
    }

    /// The path for an exposed id.
    ///
    /// Cheap for anything reached by walking from the root, which is every
    /// handle a client obtained in this process. The fallback walk exists for
    /// handles that outlived the cache.
    fn path_of(&self, id: i64) -> Result<String> {
        if let Some(path) = self.paths.get(&id) {
            return Ok(path.clone());
        }
        let (fs, ino) = match self.resolve(id) {
            Layer::Delta(ino) => (self.fs.delta(), ino),
            Layer::Base(ino) => (self.fs.base(), ino),
        };
        walk_to_path(fs, ino)
    }

    fn remember(&mut self, id: i64, path: String) {
        self.paths.insert(id, path);
    }

    /// Drop cached paths for `path` and everything under it.
    ///
    /// Dropping rather than repointing: `path_of` falls back to walking
    /// `fs_dentry`, which is slower but always right, and rewriting a subtree's
    /// worth of cached strings would be the same walk done eagerly.
    fn forget_subtree(&mut self, path: &str) {
        let prefix = format!("{}/", path.trim_end_matches('/'));
        self.paths
            .retain(|_, cached| cached != path && !cached.starts_with(&prefix));
    }

    /// Join a parent path and a child name.
    fn child_path(parent: &str, name: &str) -> String {
        if parent == "/" {
            format!("/{name}")
        } else {
            format!("{parent}/{name}")
        }
    }

    /// Resolve one name in a directory, following delta -> whiteout -> base.
    pub fn lookup_ino(&mut self, parent: i64, name: &str) -> Result<Option<i64>> {
        let parent_path = self.path_of(parent)?;
        let child = Self::child_path(&parent_path, name);

        // The delta wins outright: a name present there is the merged answer,
        // whether it is new or copied up.
        if let Layer::Delta(dir) = self.resolve(parent) {
            if let Some(ino) = self.fs.delta().lookup_ino(dir, name)? {
                self.remember(ino, child);
                return Ok(Some(ino));
            }
        }

        // A whiteout hides the base entry and everything under it.
        if self.fs.has_whiteout(&child)? {
            return Ok(None);
        }

        let Some(base_dir) = self.base_dir_ino(parent, &parent_path)? else {
            return Ok(None);
        };
        let Some(base_ino) = self.fs.base().lookup_ino(base_dir, name)? else {
            return Ok(None);
        };
        // Already copied up under a different delta name? Then the delta entry
        // above would have matched; reaching here means this is still base.
        let exposed = base_ino | BASE_TAG;
        self.remember(exposed, child);
        Ok(Some(exposed))
    }

    /// The base-layer inode for a directory, if the base can still contribute
    /// entries there.
    fn base_dir_ino(&self, parent: i64, parent_path: &str) -> Result<Option<i64>> {
        match self.resolve(parent) {
            Layer::Base(ino) => Ok(Some(ino)),
            Layer::Delta(_) => {
                // A directory that exists in the delta may still have base
                // entries to merge, but only where the base remains visible.
                if !self.fs.base_visible(parent_path)? {
                    return Ok(None);
                }
                self.fs.base().resolve(parent_path)
            }
        }
    }

    /// Metadata for an exposed id, with `ino` reported as the exposed id so a
    /// client sees one stable identity rather than the layer's private one.
    pub fn stat_ino(&self, id: i64) -> Result<Metadata> {
        let mut meta = match self.resolve(id) {
            Layer::Delta(ino) => self.fs.delta().stat_ino(ino)?,
            Layer::Base(ino) => self.fs.base().stat_ino(ino)?,
        };
        meta.ino = id;
        Ok(meta)
    }

    /// Read from a file by exposed id.
    pub fn read_ino_at(&self, id: i64, offset: u64, count: usize) -> Result<(Vec<u8>, bool)> {
        match self.resolve(id) {
            Layer::Delta(ino) => self.fs.delta().read_ino_at(ino, offset, count),
            Layer::Base(ino) => self.fs.base().read_ino_at(ino, offset, count),
        }
    }

    /// Read a symlink target by exposed id.
    pub fn readlink_ino(&self, id: i64) -> Result<String> {
        match self.resolve(id) {
            Layer::Delta(ino) => self.fs.delta().readlink_ino(ino),
            Layer::Base(ino) => self.fs.base().readlink_ino(ino),
        }
    }

    /// The merged children of a directory: the delta's entries, then the
    /// base's minus whiteouts and minus anything the delta already covers.
    ///
    /// Pagination stays exact because [`BASE_TAG`] keeps the merged sequence
    /// monotonic — a cursor below the tag is still walking the delta, above it
    /// the base.
    pub fn readdir_ino(
        &mut self,
        parent: i64,
        start_after: i64,
        limit: usize,
    ) -> Result<Vec<DirEntry>> {
        let parent_path = self.path_of(parent)?;
        let mut entries = Vec::new();

        if let Layer::Delta(dir) = self.resolve(parent) {
            if start_after & BASE_TAG == 0 {
                for entry in self.fs.delta().readdir_ino(dir, start_after, limit)? {
                    let child = Self::child_path(&parent_path, &entry.name);
                    self.remember(entry.meta.ino, child);
                    entries.push(entry);
                }
            }
            if entries.len() >= limit {
                return Ok(entries);
            }
        }

        let Some(base_dir) = self.base_dir_ino(parent, &parent_path)? else {
            return Ok(entries);
        };
        // Resuming inside the base portion starts after the untagged cursor;
        // arriving fresh from the delta portion starts at the beginning.
        let base_cursor = if start_after & BASE_TAG == 0 {
            0
        } else {
            start_after & !BASE_TAG
        };
        // Filtering happens after the query, so a batch can be consumed
        // entirely by whiteouts and delta duplicates. Keep pulling until the
        // page is full or the base is exhausted: returning a short page — in
        // the worst case an empty one — reads to a client as end-of-directory,
        // and it would stop with entries left unseen.
        let mut cursor = base_cursor;
        while entries.len() < limit {
            let batch = self.fs.base().readdir_ino(
                base_dir,
                cursor,
                limit.saturating_sub(entries.len()).max(1),
            )?;
            let Some(last) = batch.last() else {
                break;
            };
            cursor = last.meta.ino;
            for mut entry in batch {
                let child = Self::child_path(&parent_path, &entry.name);
                if self.fs.has_whiteout(&child)? {
                    continue;
                }
                // A name the delta already provides was emitted above.
                if let Layer::Delta(dir) = self.resolve(parent) {
                    if self.fs.delta().lookup_ino(dir, &entry.name)?.is_some() {
                        continue;
                    }
                }
                let exposed = entry.meta.ino | BASE_TAG;
                entry.meta.ino = exposed;
                self.remember(exposed, child);
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Write to a file by exposed id, copying it up from the base first.
    ///
    /// The exposed id does not change: the redirect makes the caller's handle
    /// point at the new delta inode, which is the whole reason the redirect
    /// exists.
    pub fn write_ino_at(&mut self, id: i64, offset: u64, data: &[u8]) -> Result<Metadata> {
        let ino = self.materialize(id)?;
        let mut meta = self.fs.delta_mut().write_ino_at(ino, offset, data)?;
        meta.ino = id;
        Ok(meta)
    }

    /// Truncate a file by exposed id, copying it up first.
    pub fn truncate_ino(&mut self, id: i64, size: u64) -> Result<Metadata> {
        let ino = self.materialize(id)?;
        let mut meta = self.fs.delta_mut().truncate_ino(ino, size)?;
        meta.ino = id;
        Ok(meta)
    }

    /// The delta inode for a directory, creating the chain if the directory so
    /// far exists only in the base.
    ///
    /// Every creation lands in the delta, so a new file under a base-only
    /// directory needs that directory to exist there first. Directories are
    /// made rather than copied: their contents keep coming from the base
    /// through the merge, which is what keeps this cheap.
    fn ensure_delta_dir(&mut self, parent: i64) -> Result<i64> {
        let base_ino = match self.resolve(parent) {
            Layer::Delta(ino) => return Ok(ino),
            Layer::Base(ino) => ino,
        };
        let path = self.path_of(parent)?;
        let ino = self.fs.delta_mut().mkdir_p(&path)?;
        // The directory now exists in both layers, and the caller still holds
        // the tagged base id. Redirect it the same way a copied-up file is
        // redirected, or that id keeps resolving to the base and everything
        // created here is invisible through it.
        self.redirect.insert(base_ino, ino);
        self.remember(ino, path.clone());
        self.remember(base_ino | BASE_TAG, path);
        Ok(ino)
    }

    /// Create a regular file.
    pub fn create_child(&mut self, parent: i64, name: &str, mode: u32) -> Result<(i64, Metadata)> {
        self.insert(parent, name, |delta, dir| {
            delta.create_child(dir, name, mode)
        })
    }

    /// Create a directory.
    pub fn mkdir_ino(&mut self, parent: i64, name: &str, mode: u32) -> Result<(i64, Metadata)> {
        self.insert(parent, name, |delta, dir| delta.mkdir_ino(dir, name, mode))
    }

    /// Create a symlink.
    pub fn symlink_ino(
        &mut self,
        parent: i64,
        name: &str,
        target: &str,
    ) -> Result<(i64, Metadata)> {
        self.insert(parent, name, |delta, dir| {
            delta.symlink_ino(dir, name, target)
        })
    }

    /// Shared shape for the three creations: materialize the parent, clear any
    /// whiteout at the child, create in the delta.
    fn insert(
        &mut self,
        parent: i64,
        name: &str,
        create: impl FnOnce(&mut AgentFs, i64) -> Result<(i64, Metadata)>,
    ) -> Result<(i64, Metadata)> {
        let parent_path = self.path_of(parent)?;
        let child = Self::child_path(&parent_path, name);
        let dir = self.ensure_delta_dir(parent)?;
        // SPEC rule: creating at a whited-out path lifts the whiteout, so the
        // new file is visible rather than hidden by its predecessor's deletion.
        self.fs.clear_whiteout(&child)?;
        let (ino, meta) = create(self.fs.delta_mut(), dir)?;
        self.remember(ino, child);
        Ok((ino, meta))
    }

    /// Remove a name from a directory.
    ///
    /// A delta entry is deleted. A name the base still provides is whited out
    /// instead — the base is read-only, so hiding is the only removal
    /// available. A copied-up file needs both.
    pub fn remove_ino(&mut self, parent: i64, name: &str) -> Result<()> {
        let parent_path = self.path_of(parent)?;
        let child = Self::child_path(&parent_path, name);

        let mut removed = false;
        if let Layer::Delta(dir) = self.resolve(parent) {
            if self.fs.delta().lookup_ino(dir, name)?.is_some() {
                self.fs.delta_mut().remove_ino(dir, name)?;
                removed = true;
            }
        }

        // Does the base still contribute this name? If so it reappears after
        // the delta entry goes, and only a whiteout actually removes it.
        let base_has = match self.base_dir_ino(parent, &parent_path)? {
            Some(base_dir) => self.fs.base().lookup_ino(base_dir, name)?.is_some(),
            None => false,
        };
        if base_has {
            self.fs.set_whiteout(&child)?;
        } else if !removed {
            return Err(Error::NotFound(child));
        }
        Ok(())
    }

    /// Move a name, materializing whatever the base still owns.
    ///
    /// Rename is the operation where the two layers genuinely fight: the base
    /// is read-only, so a name cannot move out of it. The source is copied
    /// into the delta first, moved there, and the old name whited out if the
    /// base would otherwise keep supplying it.
    pub fn rename_ino(
        &mut self,
        from_parent: i64,
        from_name: &str,
        to_parent: i64,
        to_name: &str,
    ) -> Result<()> {
        let from_dir_path = self.path_of(from_parent)?;
        let from_path = Self::child_path(&from_dir_path, from_name);
        let to_dir_path = self.path_of(to_parent)?;
        let to_path = Self::child_path(&to_dir_path, to_name);
        if from_path == to_path {
            return Ok(());
        }

        let source = self
            .lookup_ino(from_parent, from_name)?
            .ok_or_else(|| Error::NotFound(from_path.clone()))?;

        // Materialize whenever the base still contributes anything here, not
        // merely when the source id resolves to the base layer.
        //
        // Those are different conditions, and assuming otherwise loses data.
        // Creating a file under a base-only directory puts that directory in
        // the delta, so the source then resolves to Delta — while every other
        // base child beneath it exists only in the base. The rename moves the
        // delta entry and whites out the old path, and anything not
        // materialized first becomes unreachable through either layer.
        if self.fs.base_visible(&from_path)? && self.fs.base().exists(&from_path)? {
            let delta_ino = self.materialize_tree(&from_path)?;
            if let Layer::Base(base_ino) = self.resolve(source) {
                self.redirect.insert(base_ino, delta_ino);
            }
        }

        let from_dir = self.ensure_delta_dir(from_parent)?;
        let to_dir = self.ensure_delta_dir(to_parent)?;
        self.fs.clear_whiteout(&to_path)?;
        self.fs
            .delta_mut()
            .rename_ino(from_dir, from_name, to_dir, to_name)?;

        // The delta entry moved, but the base's copy of the old name did not.
        // Without a whiteout the file reappears at its original path, which
        // reads as the rename having copied rather than moved.
        let base_keeps_source = match self.base_dir_ino(from_parent, &from_dir_path)? {
            Some(base_dir) => self.fs.base().lookup_ino(base_dir, from_name)?.is_some(),
            None => false,
        };
        if base_keeps_source {
            self.fs.set_whiteout(&from_path)?;
        }

        // The renamed entry keeps its identity; the cached paths of everything
        // beneath it do not. Those entries would otherwise resolve to the
        // pre-rename path, which `copy_up` then fails on because the rename
        // whited it out.
        self.forget_subtree(&from_path);
        self.remember(source, to_path);
        Ok(())
    }

    /// Copy a base subtree into the delta, returning the delta inode.
    ///
    /// Only rename needs this. Reads and writes never do: a file copies up
    /// alone, and a directory keeps serving its children through the merge.
    /// Moving a directory out of the base is the one case where the delta has
    /// to own the whole subtree, and the cost is proportional to it.
    fn materialize_tree(&mut self, path: &str) -> Result<i64> {
        let meta = self.fs.base().stat(path)?;
        let delta_ino = if meta.is_file() {
            self.fs.copy_up(path)?
        } else if meta.is_symlink() {
            let target = self.fs.base().read_link(path)?;
            self.fs.delta_mut().symlink(&target, path)?
        } else {
            let ino = self.fs.delta_mut().mkdir_p(path)?;
            for name in self.fs.base().readdir(path)? {
                let child = Self::child_path(path, &name);
                if self.fs.has_whiteout(&child)? {
                    continue;
                }
                // Iterating the base's own listing, so a stat here always
                // resolves.
                let base_is_dir = self.fs.base().stat(&child)?.is_dir();
                if let Some(existing) = self.fs.delta().resolve(&child)? {
                    if !base_is_dir {
                        // Already in the delta and nothing to copy — but a
                        // handle still tagged base must not keep resolving to
                        // the base copy, so it is redirected all the same.
                        if let Some(base_child) = self.fs.base().resolve(&child)? {
                            self.redirect.insert(base_child, existing);
                        }
                        continue;
                    }
                    // A DIRECTORY present in both layers still has to be
                    // descended. The base can hold children the delta does
                    // not, and once the rename whites out the old path the
                    // base can no longer supply them — skipping here loses
                    // them outright. Recursing is safe: mkdir_p is idempotent.
                }
                self.materialize_tree(&child)?;
            }
            ino
        };
        // Every node, not just the root. A handle to a descendant is tagged
        // base, and without its own redirect it keeps resolving to the base
        // copy — reading content that the delta has since moved past, silently.
        // Handle stability across copy-up is the property this module exists
        // for, and it has to hold at every depth.
        self.redirect.insert(meta.ino, delta_ino);
        Ok(delta_ino)
    }

    /// Change attributes, copying the file up first if it is still base.
    pub fn setattr_ino(
        &mut self,
        id: i64,
        mode: Option<u32>,
        uid: Option<i64>,
        gid: Option<i64>,
        atime: Option<(i64, i64)>,
        mtime: Option<(i64, i64)>,
    ) -> Result<Metadata> {
        let ino = self.materialize(id)?;
        let mut meta = self
            .fs
            .delta_mut()
            .setattr_ino(ino, mode, uid, gid, atime, mtime)?;
        meta.ino = id;
        Ok(meta)
    }

    /// The directory holding an exposed id.
    pub fn parent_of(&self, id: i64) -> Result<Option<i64>> {
        match self.resolve(id) {
            Layer::Delta(ino) => self.fs.delta().parent_of(ino),
            Layer::Base(ino) => Ok(self.fs.base().parent_of(ino)?.map(|parent| {
                if parent == ROOT_INO {
                    ROOT_INO
                } else {
                    parent | BASE_TAG
                }
            })),
        }
    }

    /// Ensure an exposed id has a delta inode, copying up if it is still base,
    /// and return that delta inode.
    fn materialize(&mut self, id: i64) -> Result<i64> {
        match self.resolve(id) {
            Layer::Delta(ino) => Ok(ino),
            Layer::Base(base_ino) => {
                let path = self.path_of(id)?;
                let delta_ino = self.fs.copy_up(&path)?;
                self.redirect.insert(base_ino, delta_ino);
                Ok(delta_ino)
            }
        }
    }
}

/// Reconstruct an absolute path by walking `fs_dentry` upwards.
///
/// The cold path behind [`OverlayExport::path_of`]. `ino.rs` avoids this shape
/// on hot operations for good reason; it is here only for handles the path
/// cache never saw.
fn walk_to_path(fs: &AgentFs, ino: i64) -> Result<String> {
    let mut components = Vec::new();
    let mut current = ino;
    while current != ROOT_INO {
        let Some(parent) = fs.parent_of(current)? else {
            return Err(Error::NotFound(format!("inode {ino}")));
        };
        let name = fs.child_name(parent, current)?;
        components.push(name);
        current = parent;
        // A cycle would hang the export; `fs_dentry` should never contain one,
        // but an export must not be the thing that proves it.
        if components.len() > 4096 {
            return Err(Error::InvalidArgument(format!(
                "inode {ino} exceeds the maximum path depth"
            )));
        }
    }
    components.reverse();
    Ok(normalize(&format!("/{}", components.join("/"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A base with a small tree, and an empty delta over it.
    fn export(dir: &std::path::Path) -> OverlayExport {
        let base_path = dir.join("base.db");
        {
            let mut base = AgentFs::create(&base_path).unwrap();
            base.write_file("/keep.txt", b"unchanged").unwrap();
            base.write_file("/edit.txt", b"original").unwrap();
            base.write_file("/drop.txt", b"doomed").unwrap();
            base.write_file("/dir/nested.txt", b"nested").unwrap();
        }
        let overlay = OverlayFs::open(dir.join("delta.db"), &base_path).unwrap();
        OverlayExport::new(overlay).unwrap()
    }

    fn read_all(export: &OverlayExport, id: i64) -> Vec<u8> {
        export.read_ino_at(id, 0, 4096).unwrap().0
    }

    fn names(entries: &[DirEntry]) -> Vec<String> {
        let mut names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        names.sort();
        names
    }

    #[test]
    fn a_base_only_file_is_readable_through_the_export() {
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();

        let id = export.lookup_ino(root, "keep.txt").unwrap().expect("found");
        // Nothing is in the delta yet, so this must be a tagged base id.
        assert_ne!(id & BASE_TAG, 0);
        assert_eq!(read_all(&export, id), b"unchanged");
        // The reported ino is the exposed id, not the layer's private one.
        assert_eq!(export.stat_ino(id).unwrap().ino, id);
    }

    #[test]
    fn a_handle_survives_the_copy_up_its_first_write_causes() {
        // The property the whole design exists for: a client that opened a
        // base-only file keeps its handle working after writing to it.
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();

        let id = export.lookup_ino(root, "edit.txt").unwrap().expect("found");
        assert_ne!(id & BASE_TAG, 0);
        assert_eq!(read_all(&export, id), b"original");

        export.write_ino_at(id, 0, b"rewritten").unwrap();

        // Same handle, no re-lookup.
        assert_eq!(read_all(&export, id), b"rewritten");
        assert_eq!(export.stat_ino(id).unwrap().ino, id);
        // And it now resolves into the delta rather than the base.
        assert!(matches!(export.resolve(id), Layer::Delta(_)));
    }

    #[test]
    fn a_second_lookup_after_copy_up_returns_the_delta_id() {
        // A fresh lookup legitimately sees the delta entry, so the two ids for
        // one file coexist. Both must read the copied-up content.
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();

        let base_id = export.lookup_ino(root, "edit.txt").unwrap().unwrap();
        export.write_ino_at(base_id, 0, b"rewritten").unwrap();

        let delta_id = export.lookup_ino(root, "edit.txt").unwrap().unwrap();
        assert_eq!(delta_id & BASE_TAG, 0);
        assert_eq!(read_all(&export, delta_id), b"rewritten");
        assert_eq!(read_all(&export, base_id), b"rewritten");
    }

    #[test]
    fn readdir_merges_both_layers() {
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();
        export
            .overlay_mut()
            .write_file("/fresh.txt", b"new")
            .unwrap();

        let entries = export.readdir_ino(root, 0, 100).unwrap();
        assert_eq!(
            names(&entries),
            vec!["dir", "drop.txt", "edit.txt", "fresh.txt", "keep.txt"]
        );
    }

    #[test]
    fn readdir_hides_whiteouts() {
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();
        export.overlay_mut().remove_file("/drop.txt").unwrap();

        let entries = export.readdir_ino(root, 0, 100).unwrap();
        assert!(!names(&entries).contains(&"drop.txt".to_string()));
        // And the name no longer resolves.
        assert_eq!(export.lookup_ino(root, "drop.txt").unwrap(), None);
    }

    #[test]
    fn readdir_reports_a_copied_up_file_once() {
        // The delta and the base both hold the name after copy-up; emitting it
        // twice would make a client see a duplicate directory entry.
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();
        let id = export.lookup_ino(root, "edit.txt").unwrap().unwrap();
        export.write_ino_at(id, 0, b"rewritten").unwrap();

        let entries = export.readdir_ino(root, 0, 100).unwrap();
        let hits = entries.iter().filter(|e| e.name == "edit.txt").count();
        assert_eq!(hits, 1, "{:?}", names(&entries));
    }

    #[test]
    fn a_page_filled_entirely_by_whiteouts_still_returns_later_entries() {
        // Regression: filtering happens after the query, so whiteouts consume
        // the page budget. Returning the short page reads as end-of-directory
        // and the client stops with `survivor.txt` unseen.
        let temp = tempfile::tempdir().unwrap();
        let base_path = temp.path().join("base.db");
        {
            let mut base = AgentFs::create(&base_path).unwrap();
            for index in 0..8 {
                base.write_file(&format!("/doomed{index}.txt"), b"x")
                    .unwrap();
            }
            base.write_file("/survivor.txt", b"kept").unwrap();
        }
        let overlay = OverlayFs::open(temp.path().join("delta.db"), &base_path).unwrap();
        let mut export = OverlayExport::new(overlay).unwrap();
        for index in 0..8 {
            export
                .overlay_mut()
                .remove_file(&format!("/doomed{index}.txt"))
                .unwrap();
        }

        // A page of 2 lands entirely inside the whiteouts.
        let entries = export.readdir_ino(export.root(), 0, 2).unwrap();
        assert_eq!(names(&entries), vec!["survivor.txt"]);
    }

    #[test]
    fn nested_base_directories_are_traversable() {
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();

        let dir = export.lookup_ino(root, "dir").unwrap().expect("dir");
        let nested = export.lookup_ino(dir, "nested.txt").unwrap().expect("file");
        assert_eq!(read_all(&export, nested), b"nested");
    }

    #[test]
    fn creating_under_a_base_only_directory_works() {
        // The delta has no `/dir` until this point; a creation there has to
        // make one, while the base's existing children stay visible.
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();
        let dir = export.lookup_ino(root, "dir").unwrap().unwrap();
        assert_ne!(dir & BASE_TAG, 0, "starts out base-only");

        let (id, _) = export.create_child(dir, "made.txt", 0o644).unwrap();
        export.write_ino_at(id, 0, b"fresh").unwrap();

        let entries = export.readdir_ino(dir, 0, 100).unwrap();
        assert_eq!(names(&entries), vec!["made.txt", "nested.txt"]);
        assert_eq!(read_all(&export, id), b"fresh");
    }

    #[test]
    fn removing_a_base_only_name_whites_it_out() {
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();

        export.remove_ino(root, "keep.txt").unwrap();
        assert_eq!(export.lookup_ino(root, "keep.txt").unwrap(), None);
        assert!(
            !names(&export.readdir_ino(root, 0, 100).unwrap()).contains(&"keep.txt".to_string())
        );
    }

    #[test]
    fn removing_a_copied_up_file_does_not_resurrect_the_base_copy() {
        // Deleting only the delta entry would let the base version reappear,
        // which reads as the delete having silently failed.
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();
        let id = export.lookup_ino(root, "edit.txt").unwrap().unwrap();
        export.write_ino_at(id, 0, b"rewritten").unwrap();

        export.remove_ino(root, "edit.txt").unwrap();
        assert_eq!(export.lookup_ino(root, "edit.txt").unwrap(), None);
    }

    #[test]
    fn creating_over_a_whiteout_lifts_it() {
        // SPEC rule: a whiteout is removed when a new file is created at that
        // path. Without this the replacement stays invisible.
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();
        export.remove_ino(root, "drop.txt").unwrap();

        let (id, _) = export.create_child(root, "drop.txt", 0o644).unwrap();
        export.write_ino_at(id, 0, b"replacement").unwrap();

        let found = export
            .lookup_ino(root, "drop.txt")
            .unwrap()
            .expect("visible");
        assert_eq!(read_all(&export, found), b"replacement");
    }

    #[test]
    fn removing_a_name_that_exists_nowhere_is_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();
        assert!(matches!(
            export.remove_ino(root, "never.txt"),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn setattr_copies_a_base_file_up_and_keeps_the_handle() {
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();
        let id = export.lookup_ino(root, "keep.txt").unwrap().unwrap();

        let meta = export
            .setattr_ino(id, Some(0o600), None, None, None, None)
            .unwrap();
        assert_eq!(meta.ino, id, "the exposed id must not change");
        assert_eq!(meta.mode & 0o777, 0o600);
        // Content survived the copy-up.
        assert_eq!(read_all(&export, id), b"unchanged");
    }

    #[test]
    fn renaming_a_base_only_file_moves_it_rather_than_copying_it() {
        // Without a whiteout on the source, the base keeps supplying the old
        // name and the file appears at both paths — a copy, not a move.
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();

        export
            .rename_ino(root, "keep.txt", root, "moved.txt")
            .unwrap();

        assert_eq!(export.lookup_ino(root, "keep.txt").unwrap(), None);
        let moved = export
            .lookup_ino(root, "moved.txt")
            .unwrap()
            .expect("moved");
        assert_eq!(read_all(&export, moved), b"unchanged");
        let listed = names(&export.readdir_ino(root, 0, 100).unwrap());
        assert!(listed.contains(&"moved.txt".to_string()));
        assert!(!listed.contains(&"keep.txt".to_string()), "{listed:?}");
    }

    #[test]
    fn renaming_a_base_only_directory_carries_its_children() {
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();

        export.rename_ino(root, "dir", root, "moved").unwrap();

        assert_eq!(export.lookup_ino(root, "dir").unwrap(), None);
        let moved = export.lookup_ino(root, "moved").unwrap().expect("moved");
        let nested = export
            .lookup_ino(moved, "nested.txt")
            .unwrap()
            .expect("child");
        assert_eq!(read_all(&export, nested), b"nested");
    }

    #[test]
    fn a_handle_taken_before_a_directory_rename_still_reads_current_content() {
        // The existing rename test re-looks-up the child through the NEW
        // parent, which takes the delta path and never exercises a handle
        // held across the rename. Holding handles across a rename is exactly
        // what an NFS client does.
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();

        let dir = export.lookup_ino(root, "dir").unwrap().unwrap();
        let nested = export.lookup_ino(dir, "nested.txt").unwrap().unwrap();
        assert_ne!(nested & BASE_TAG, 0, "starts out a tagged base handle");

        export.rename_ino(root, "dir", root, "moved").unwrap();

        // Write through the NEW path, then read through the OLD handle. Before
        // the fix the handle still resolved to the base copy and returned the
        // pre-write content.
        let moved = export.lookup_ino(root, "moved").unwrap().unwrap();
        let via_new = export.lookup_ino(moved, "nested.txt").unwrap().unwrap();
        export
            .write_ino_at(via_new, 0, b"updated after the rename")
            .unwrap();

        assert_eq!(read_all(&export, nested), b"updated after the rename");
    }

    #[test]
    fn a_handle_taken_before_a_directory_rename_can_still_be_written() {
        // The compounded failure: the descendant's cached path was stale, so
        // materialize() called copy_up on the pre-rename path, which the
        // rename had whited out — the write failed NotFound.
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();

        let dir = export.lookup_ino(root, "dir").unwrap().unwrap();
        let nested = export.lookup_ino(dir, "nested.txt").unwrap().unwrap();

        export.rename_ino(root, "dir", root, "moved").unwrap();

        export
            .write_ino_at(nested, 0, b"written through the old handle")
            .expect("a handle held across a rename must stay writable");
        assert_eq!(read_all(&export, nested), b"written through the old handle");
    }

    #[test]
    fn renaming_carries_base_children_under_a_directory_the_delta_already_has() {
        // Found in review of the first fix. `materialize_tree` skipped any
        // child already present in the delta; for a directory that stranded
        // the base-only files beneath it, because the rename then whites out
        // the old path and the base can no longer supply them.
        let temp = tempfile::tempdir().unwrap();
        let base_path = temp.path().join("base.db");
        {
            let mut base = AgentFs::create(&base_path).unwrap();
            base.write_file("/dir/sub/deep.txt", b"deep in the base")
                .unwrap();
        }
        let overlay = OverlayFs::open(temp.path().join("delta.db"), &base_path).unwrap();
        let mut export = OverlayExport::new(overlay).unwrap();
        let root = export.root();

        // Put `/dir/sub` in the delta the way ordinary use would: create a
        // file inside it.
        let dir = export.lookup_ino(root, "dir").unwrap().unwrap();
        let sub = export.lookup_ino(dir, "sub").unwrap().unwrap();
        let (fresh, _) = export.create_child(sub, "added.txt", 0o644).unwrap();
        export.write_ino_at(fresh, 0, b"new").unwrap();

        export.rename_ino(root, "dir", root, "moved").unwrap();

        let moved = export.lookup_ino(root, "moved").unwrap().expect("moved");
        let moved_sub = export.lookup_ino(moved, "sub").unwrap().expect("sub");
        let deep = export
            .lookup_ino(moved_sub, "deep.txt")
            .unwrap()
            .expect("a base-only file under a delta directory must survive the rename");
        assert_eq!(read_all(&export, deep), b"deep in the base");
    }

    #[test]
    fn renaming_onto_a_whited_out_name_lifts_the_whiteout() {
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();
        export.remove_ino(root, "drop.txt").unwrap();

        export
            .rename_ino(root, "keep.txt", root, "drop.txt")
            .unwrap();

        let found = export
            .lookup_ino(root, "drop.txt")
            .unwrap()
            .expect("visible");
        assert_eq!(read_all(&export, found), b"unchanged");
    }

    #[test]
    fn a_path_is_recoverable_for_a_handle_the_cache_never_saw() {
        // Handles outlive the cache across a restart, so the walk fallback has
        // to produce the same path the cached descent would have.
        let temp = tempfile::tempdir().unwrap();
        let mut export = export(temp.path());
        let root = export.root();
        let dir = export.lookup_ino(root, "dir").unwrap().unwrap();
        let nested = export.lookup_ino(dir, "nested.txt").unwrap().unwrap();

        export.paths.clear();
        assert_eq!(export.path_of(nested).unwrap(), "/dir/nested.txt");
    }
}
