//! The filesystem half of the scoped read contract: reading one authorized
//! file beneath a bound root.
//!
//! A root is the directory opened when the reader is built. A request is
//! served only while the root's path still names that directory, so a root
//! renamed away or replaced refuses every read. The walk opens one component
//! at a time from the root's handle and never follows a link, so no spelling
//! can leave the root. Each component's identity, checked on the opened handle
//! and not only on its name, must stay on the root's filesystem and must not
//! be a protected resource. A protected resource is held by identity, so a
//! case or Unicode alias of its name on a filesystem that equates them still
//! matches.
//!
//! After the open, the walk is repeated from the root by name. Every directory
//! and the file must still be what the root's own names lead to; a directory
//! renamed out of the root after the walk entered it fails here, before any
//! content is read.
//!
//! Mount crossings are refused by device, and on Linux also by mount id, which
//! catches a bind mount from the same filesystem. Only Unix platforms have the
//! reviewed walk; elsewhere every read is refused.

use std::path::Path;

use serde_json::Value;

use super::scope::{Grants, ReadRequest, RootKind};
use super::{ReadPolicy, Refusal};
use crate::automations::contract::canonical_json::sha256_hex;

/// A filesystem object's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(unix), allow(dead_code))]
struct Identity {
    device: u64,
    inode: u64,
}

/// A point in a read, reported to the reader's probe. Tests use it to change
/// the filesystem at an exact step; production passes a probe that does
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// The first filesystem access: the root is about to be rechecked.
    Root,
    /// Directory component `n` was listed and is about to be opened.
    BeforeDirectory(usize),
    /// Directory component `n` was opened.
    Directory(usize),
    /// The file was listed and is about to be opened.
    BeforeFile,
    /// The file was opened and the recheck from the root is next.
    File,
    /// The recheck and the size checks passed; the content is read next.
    Read,
}

/// File content the reader produced. It is bounded and buffered whole; the
/// service commits its digest before any of it is disclosed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileRead {
    pub content: Vec<u8>,
    /// Lowercase hex SHA-256 of `content`.
    pub digest: String,
}

/// One attempt's reader: its grant, bound roots, protected resources and the
/// budget it has used.
pub(crate) struct ScopedReader {
    grants: Grants,
    policy: ReadPolicy,
    roots: Vec<platform::BoundRoot>,
    protected: Vec<Identity>,
    calls: u32,
    disclosed: u64,
}

impl ScopedReader {
    /// Builds a reader for `scopes` over `roots`. Every protected path that
    /// exists is held by the identity of what it names. A root that is, or is
    /// inside, a protected resource refuses the reader.
    pub(crate) fn new(
        scopes: &[Value],
        roots: &[(RootKind, &Path)],
        protected: &[&Path],
        policy: ReadPolicy,
    ) -> Result<Self, Refusal> {
        let grants = Grants::from_scopes(scopes, &policy)?;
        let protected = platform::protected_identities(protected)?;
        let mut bound = Vec::with_capacity(roots.len());
        for (kind, path) in roots {
            if bound
                .iter()
                .any(|root: &platform::BoundRoot| root.kind == *kind)
            {
                return Err(Refusal::GrantUnsupported);
            }
            bound.push(platform::bind(*kind, path, &protected)?);
        }
        Ok(Self {
            grants,
            policy,
            roots: bound,
            protected,
            calls: 0,
            disclosed: 0,
        })
    }

    /// Serves one request.
    pub(crate) fn read(&mut self, request: &Value) -> Result<FileRead, Refusal> {
        self.read_with(request, &mut |_| {})
    }

    /// [`Self::read`], reporting each step to `probe`. Every call counts
    /// against the policy, refused ones included. The lexical checks all run
    /// before the first filesystem access.
    pub(crate) fn read_with(
        &mut self,
        request: &Value,
        probe: &mut dyn FnMut(Step),
    ) -> Result<FileRead, Refusal> {
        if self.calls >= self.policy.calls {
            return Err(Refusal::LimitCalls);
        }
        self.calls += 1;
        let request = ReadRequest::parse(request, &self.policy)?;
        self.grants.authorize(&request)?;
        let root = self
            .roots
            .iter()
            .find(|root| root.kind == request.root)
            .ok_or(Refusal::RootUnbound)?;
        let remaining = self.policy.attempt_bytes - self.disclosed;
        let content = platform::read_beneath(
            root,
            request.path.components(),
            &self.protected,
            self.policy.file_bytes,
            remaining,
            probe,
        )?;
        self.disclosed += content.len() as u64;
        Ok(FileRead {
            digest: sha256_hex(&content),
            content,
        })
    }
}

#[cfg(unix)]
mod platform {
    use std::io::{self, Read};
    use std::path::{Path, PathBuf};

    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::ambient_authority;
    use cap_std::fs::{Dir, Metadata, MetadataExt, OpenOptions, OpenOptionsExt};

    use super::{Identity, Refusal, RootKind, Step};

    pub(super) struct BoundRoot {
        pub kind: RootKind,
        path: PathBuf,
        dir: Dir,
        identity: Identity,
        mount: Option<u64>,
    }

    fn identity(metadata: &Metadata) -> Identity {
        Identity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    fn ambient_identity(metadata: &std::fs::Metadata) -> Identity {
        use std::os::unix::fs::MetadataExt as _;
        Identity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    pub(super) fn protected_identities(paths: &[&Path]) -> Result<Vec<Identity>, Refusal> {
        let mut identities = Vec::with_capacity(paths.len());
        for path in paths {
            // Follow a link here: what must not be read is what the path names.
            match std::fs::metadata(path) {
                Ok(metadata) => identities.push(ambient_identity(&metadata)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => return Err(Refusal::ResourceUnavailable),
            }
        }
        Ok(identities)
    }

    pub(super) fn bind(
        kind: RootKind,
        path: &Path,
        protected: &[Identity],
    ) -> Result<BoundRoot, Refusal> {
        let canonical = std::fs::canonicalize(path).map_err(|_| Refusal::RootUnbound)?;
        for ancestor in canonical.ancestors() {
            let metadata = std::fs::metadata(ancestor).map_err(|_| Refusal::RootUnbound)?;
            if protected.contains(&ambient_identity(&metadata)) {
                return Err(Refusal::RootProtected);
            }
        }
        let dir = Dir::open_ambient_dir(&canonical, ambient_authority())
            .map_err(|_| Refusal::RootUnbound)?;
        let metadata = dir.dir_metadata().map_err(|_| Refusal::RootUnbound)?;
        let named = std::fs::metadata(&canonical).map_err(|_| Refusal::RootUnbound)?;
        if !metadata.is_dir() || identity(&metadata) != ambient_identity(&named) {
            return Err(Refusal::RootMoved);
        }
        Ok(BoundRoot {
            kind,
            path: canonical,
            mount: mount_id(&dir)?,
            dir,
            identity: identity(&metadata),
        })
    }

    pub(super) fn read_beneath(
        root: &BoundRoot,
        components: &[String],
        protected: &[Identity],
        file_bytes: u64,
        remaining: u64,
        probe: &mut dyn FnMut(Step),
    ) -> Result<Vec<u8>, Refusal> {
        probe(Step::Root);
        let named = Dir::open_ambient_dir(&root.path, ambient_authority())
            .map_err(|_| Refusal::RootMoved)?;
        if identity(&named.dir_metadata().map_err(unavailable)?) != root.identity {
            return Err(Refusal::RootMoved);
        }
        let (name, parents) = components
            .split_last()
            .expect("a parsed path has a component");

        let mut directory = root.dir.try_clone().map_err(unavailable)?;
        let mut held = Vec::with_capacity(parents.len());
        for (index, component) in parents.iter().enumerate() {
            let listed = list(&directory, component)?;
            if listed.file_type().is_symlink() {
                return Err(Refusal::ResourceSymlink);
            }
            if !listed.is_dir() {
                return Err(Refusal::ResourceNotFound);
            }
            admit(identity(&listed), root, protected)?;
            probe(Step::BeforeDirectory(index));
            let next = directory
                .open_dir_nofollow(component)
                .map_err(|error| classify_open(&directory, component, &error))?;
            let opened = next.dir_metadata().map_err(unavailable)?;
            if identity(&opened) != identity(&listed) {
                return Err(Refusal::ResourceMoved);
            }
            if mount_id(&next)? != root.mount {
                return Err(Refusal::ResourceMountCrossing);
            }
            held.push(identity(&opened));
            probe(Step::Directory(index));
            directory = next;
        }

        let listed = list(&directory, name)?;
        let file_type = listed.file_type();
        if file_type.is_symlink() {
            return Err(Refusal::ResourceSymlink);
        }
        if file_type.is_dir() {
            return Err(Refusal::ResourceDirectory);
        }
        if !file_type.is_file() {
            return Err(Refusal::ResourceSpecial);
        }
        admit(identity(&listed), root, protected)?;
        if listed.nlink() != 1 {
            return Err(Refusal::ResourceMultiplyLinked);
        }
        probe(Step::BeforeFile);
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        // A FIFO swapped in after the listing must not block the open, and no
        // device may become the daemon's controlling terminal.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
        let mut file = directory
            .open_with(name, &options)
            .map_err(|error| classify_open(&directory, name, &error))?;
        let opened = file.metadata().map_err(unavailable)?;
        if identity(&opened) != identity(&listed) || !opened.is_file() {
            return Err(Refusal::ResourceMoved);
        }
        if opened.nlink() != 1 {
            return Err(Refusal::ResourceMultiplyLinked);
        }
        if mount_id(&file)? != root.mount {
            return Err(Refusal::ResourceMountCrossing);
        }
        probe(Step::File);
        recheck(root, parents, &held, name, identity(&opened))?;

        if opened.len() > file_bytes {
            return Err(Refusal::LimitFileTooLarge);
        }
        if opened.len() > remaining {
            return Err(Refusal::LimitAttemptDisclosure);
        }
        probe(Step::Read);
        // The file can grow after the size check: read one byte past the
        // limit to tell, and never allocate past it.
        let limit = file_bytes.min(remaining);
        let mut content = Vec::with_capacity(opened.len().min(limit) as usize);
        (&mut file)
            .take(limit + 1)
            .read_to_end(&mut content)
            .map_err(unavailable)?;
        if content.len() as u64 > limit {
            return Err(if limit == file_bytes {
                Refusal::LimitFileTooLarge
            } else {
                Refusal::LimitAttemptDisclosure
            });
        }
        Ok(content)
    }

    /// Walks from the root by name again. Every directory the read held, and
    /// the file, must still be what the root's own names lead to.
    fn recheck(
        root: &BoundRoot,
        parents: &[String],
        held: &[Identity],
        name: &str,
        file: Identity,
    ) -> Result<(), Refusal> {
        let mut directory = root.dir.try_clone().map_err(unavailable)?;
        for (component, expected) in parents.iter().zip(held) {
            let next = directory
                .open_dir_nofollow(component)
                .map_err(|_| Refusal::ResourceMoved)?;
            if identity(&next.dir_metadata().map_err(unavailable)?) != *expected {
                return Err(Refusal::ResourceMoved);
            }
            directory = next;
        }
        let named = directory
            .symlink_metadata(name)
            .map_err(|_| Refusal::ResourceMoved)?;
        if identity(&named) != file {
            return Err(Refusal::ResourceMoved);
        }
        Ok(())
    }

    /// The component's own metadata, without following it.
    fn list(directory: &Dir, component: &str) -> Result<Metadata, Refusal> {
        directory
            .symlink_metadata(component)
            .map_err(|error| match error.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => Refusal::ResourceNotFound,
                _ => Refusal::ResourceUnavailable,
            })
    }

    fn admit(component: Identity, root: &BoundRoot, protected: &[Identity]) -> Result<(), Refusal> {
        if component.device != root.identity.device {
            return Err(Refusal::ResourceMountCrossing);
        }
        if protected.contains(&component) {
            return Err(Refusal::ResourceProtected);
        }
        Ok(())
    }

    /// Why opening a component that was just listed failed. Platforms report
    /// a no-follow open of a link differently (`ELOOP`, `EMLINK`, or `ENOTDIR`
    /// with `O_DIRECTORY` on macOS), so look at the component again.
    fn classify_open(directory: &Dir, component: &str, error: &io::Error) -> Refusal {
        match directory.symlink_metadata(component) {
            Ok(now) if now.file_type().is_symlink() => Refusal::ResourceSymlink,
            // Listed a moment ago, and now gone or something else.
            Ok(_) if matches!(error.raw_os_error(), Some(libc::ENOTDIR)) => Refusal::ResourceMoved,
            Err(now) if now.kind() == io::ErrorKind::NotFound => Refusal::ResourceMoved,
            _ => Refusal::ResourceUnavailable,
        }
    }

    fn unavailable(_error: io::Error) -> Refusal {
        Refusal::ResourceUnavailable
    }

    /// The mount an open handle is on. Only Linux reports one; there a bind
    /// mount from the root's own filesystem has the root's device, so the
    /// device check alone would miss it. A kernel that cannot say refuses.
    #[cfg(target_os = "linux")]
    fn mount_id(handle: &impl std::os::fd::AsFd) -> Result<Option<u64>, Refusal> {
        use rustix::fs::{statx, AtFlags, StatxFlags};
        let status = statx(handle, "", AtFlags::EMPTY_PATH, StatxFlags::MNT_ID)
            .map_err(|_| Refusal::PlatformUnsupported)?;
        if StatxFlags::from_bits_retain(status.stx_mask).contains(StatxFlags::MNT_ID) {
            Ok(Some(status.stx_mnt_id))
        } else {
            Err(Refusal::PlatformUnsupported)
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn mount_id(_handle: &impl std::os::fd::AsFd) -> Result<Option<u64>, Refusal> {
        Ok(None)
    }
}

#[cfg(not(unix))]
mod platform {
    use std::path::Path;

    use super::{Identity, Refusal, RootKind, Step};

    pub(super) struct BoundRoot {
        pub kind: RootKind,
    }

    pub(super) fn protected_identities(_paths: &[&Path]) -> Result<Vec<Identity>, Refusal> {
        Err(Refusal::PlatformUnsupported)
    }

    pub(super) fn bind(
        _kind: RootKind,
        _path: &Path,
        _protected: &[Identity],
    ) -> Result<BoundRoot, Refusal> {
        Err(Refusal::PlatformUnsupported)
    }

    pub(super) fn read_beneath(
        _root: &BoundRoot,
        _components: &[String],
        _protected: &[Identity],
        _file_bytes: u64,
        _remaining: u64,
        _probe: &mut dyn FnMut(Step),
    ) -> Result<Vec<u8>, Refusal> {
        Err(Refusal::PlatformUnsupported)
    }
}
